import path from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import type { LocalServer } from '../shared/types';

const run = promisify(execFile);

/** Processes that typically run dev servers. Everything else listening (Spotify, Figma…) is ignored. */
const DEV_PROCESSES = /^(node|bun|deno|python[\d.]*|ruby|php|java|beam\.smp|dotnet|com\.docker\.backend|caddy|nginx|httpd|hugo|air|uvicorn|gunicorn)$/i;

/**
 * Finds dev servers listening on this machine (`lsof`, macOS and Linux), names each after
 * the folder its process runs in, and keeps only the ones that answer HTTP.
 */
export async function findLocalServers(): Promise<LocalServer[]> {
  let output: string;
  try {
    ({ stdout: output } = await run('lsof', ['-nP', '-iTCP', '-sTCP:LISTEN', '-Fpcn'], { timeout: 3000 }));
  } catch {
    return [];
  }

  // -F output: one field per line, `p<pid>`, `c<command>`, `n<address>`.
  const listeners = new Map<number, { pid: number; command: string }>();
  let pid = 0;
  let command = '';
  for (const line of output.split('\n')) {
    const value = line.slice(1);
    if (line[0] === 'p') pid = Number(value);
    else if (line[0] === 'c') command = value;
    else if (line[0] === 'n') {
      const port = Number(value.slice(value.lastIndexOf(':') + 1));
      if (pid !== process.pid && port >= 1024 && DEV_PROCESSES.test(command) && !listeners.has(port)) {
        listeners.set(port, { pid, command });
      }
    }
  }

  const servers = await Promise.all(
    [...listeners].map(async ([port, { pid, command }]) => {
      const url = `http://localhost:${port}/`;
      if (!(await answersHttp(url))) return undefined;
      // Docker's own folder says nothing about the container behind the port.
      const name = command === 'com.docker.backend' ? 'Docker' : ((await projectFolder(pid)) ?? command);
      return { port, url, name } satisfies LocalServer;
    }),
  );
  return servers.filter((s): s is LocalServer => !!s).sort((a, b) => a.port - b.port);
}

async function answersHttp(url: string) {
  try {
    await fetch(url, { method: 'HEAD', signal: AbortSignal.timeout(800), redirect: 'manual' });
    return true;
  } catch {
    return false;
  }
}

/** The basename of the process's working directory, usually the project's folder name. */
async function projectFolder(pid: number) {
  try {
    const { stdout } = await run('lsof', ['-a', '-p', String(pid), '-d', 'cwd', '-Fn'], { timeout: 2000 });
    const cwd = stdout.split('\n').find((l) => l.startsWith('n'))?.slice(1);
    return cwd && cwd !== '/' ? path.basename(cwd) : undefined;
  } catch {
    return undefined;
  }
}
