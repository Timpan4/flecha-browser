import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { transform } from 'esbuild';
import type { WebContents } from 'electron';
import { boostMatches, boostName, parseBoostName as parse, type BoostKind } from './boostFiles';

export type { BoostKind };

const TEMPLATES: Record<BoostKind, (host: string) => string> = {
  script: (host) => `// Boost for ${host}: runs on every page of this site once it has loaded.
// It's TypeScript; Flecha compiles it with esbuild. Save to apply (open ${host} tabs reload).
// Tip: keep this folder in git to share boosts across machines.

export {};

// document.querySelector('header')?.remove();
`,
  styles: (host) => `/* Boost for ${host}: these styles apply to every page of this site, live as you save. */

/* body { font-family: ui-sans-serif, system-ui; } */
`,
};

/**
 * Boosts are per-site scripts (TypeScript) and styles (CSS) kept as plain files in
 * ~/.flecha/boosts, named after the host: `github.com.ts`, `github.com.css`, or `all.css`
 * for every site. They apply to that host and its subdomains.
 */
export class Boosts {
  readonly dir = path.join(os.homedir(), '.flecha', 'boosts');
  private compiled = new Map<string, { mtimeMs: number; code: string }>();
  private insertedCss = new WeakMap<WebContents, string[]>();

  constructor(onChange: (file: { host: string; kind: BoostKind }) => void) {
    fs.mkdirSync(this.dir, { recursive: true });
    const timers = new Map<string, NodeJS.Timeout>();
    fs.watch(this.dir, (_event, filename) => {
      const parsed = filename ? parse(filename) : undefined;
      if (!parsed) return;
      // Editors save in several steps; react once they're done.
      clearTimeout(timers.get(filename!));
      timers.set(filename!, setTimeout(() => onChange(parsed), 150));
    });
  }

  /** Hosts with at least one boost, for the sidebar's ⚡ marker. */
  hosts(): string[] {
    return [...new Set(fs.readdirSync(this.dir).flatMap((f) => parse(f)?.host ?? []))];
  }

  matches(url: string, host: string) {
    return boostMatches(url, host);
  }

  /** Applies every matching boost to a page that just finished building its DOM. */
  async apply(wc: WebContents) {
    const url = wc.getURL();
    const files = fs.readdirSync(this.dir).filter((f) => {
      const parsed = parse(f);
      return parsed && this.matches(url, parsed.host);
    });
    await this.applyStyles(wc, files);
    for (const file of files.filter((f) => f.endsWith('.ts'))) {
      try {
        await wc.executeJavaScript(await this.compile(path.join(this.dir, file)));
      } catch (err) {
        report(wc, file, err);
      }
    }
  }

  /** Swaps a page's boost styles in place, so CSS edits show up without a reload. */
  async refreshStyles(wc: WebContents) {
    const url = wc.getURL();
    await this.applyStyles(
      wc,
      fs.readdirSync(this.dir).filter((f) => f.endsWith('.css') && this.matches(url, parse(f)!.host)),
    );
  }

  /** The boost file for a host, created from a template the first time. */
  fileFor(host: string, kind: BoostKind) {
    const file = path.join(this.dir, boostName(host, kind));
    if (!fs.existsSync(file)) fs.writeFileSync(file, TEMPLATES[kind](host));
    return file;
  }

  private async applyStyles(wc: WebContents, files: string[]) {
    for (const key of this.insertedCss.get(wc) ?? []) await wc.removeInsertedCSS(key).catch(() => {});
    const keys: string[] = [];
    for (const file of files.filter((f) => f.endsWith('.css'))) {
      keys.push(await wc.insertCSS(fs.readFileSync(path.join(this.dir, file), 'utf8')));
    }
    this.insertedCss.set(wc, keys);
  }

  private async compile(file: string) {
    const { mtimeMs } = fs.statSync(file);
    const cached = this.compiled.get(file);
    if (cached?.mtimeMs === mtimeMs) return cached.code;
    const { code } = await transform(fs.readFileSync(file, 'utf8'), {
      loader: 'ts',
      format: 'iife',
      target: `chrome${process.versions.chrome.split('.')[0]}`,
      sourcefile: path.basename(file),
    });
    this.compiled.set(file, { mtimeMs, code });
    return code;
  }
}

/** Boost errors go to the page's own console, where the developer is already looking. */
function report(wc: WebContents, file: string, err: unknown) {
  const message = `[Flecha boost ${file}] ${err instanceof Error ? err.message : String(err)}`;
  wc.executeJavaScript(`console.error(${JSON.stringify(message)})`).catch(() => {});
}
