import assert from 'node:assert/strict';
import { readFileSync, readdirSync, statSync } from 'node:fs';

const forbidden = ['SSH_AUTH_SOCK', 'AT_SPI_BUS_ADDRESS', 'PULSE_SERVER', 'XAUTHORITY'];

function vanished(pid) {
  try {
    statSync(`/proc/${pid}`);

    return false;
  } catch (error) {
    if (error.code === 'ENOENT' || error.code === 'ESRCH') return true;

    throw error;
  }
}

function readProcess(pid, file) {
  try {
    return readFileSync(`/proc/${pid}/${file}`, 'utf8');
  } catch (error) {
    if ((error.code === 'ENOENT' || error.code === 'ESRCH') && vanished(pid)) return null;

    throw new Error(`inconclusive process scan: could not read ${file} for pid ${pid} (${error.code})`);
  }
}

function identity(pid) {
  const stat = readProcess(pid, 'stat');

  if (stat === null) return null;

  const end = stat.lastIndexOf(')');
  const fields = stat.slice(end + 2).trim().split(' ');
  // Linux proc_pid_stat(5): comm is field 2, PPID is field 4, starttime is field 22.
  assert(stat.startsWith(`${pid} (`) && end > 0 && /^[A-Za-z]$/.test(fields[0])
    && /^\d+$/.test(fields[1]) && /^\d+$/.test(fields[19]), `inconclusive process scan: malformed stat for pid ${pid}`);

  return { parent: fields[1], started: fields[19], state: fields[0] };
}

export function checkProcessEnvironment(rootPid) {
  const root = String(rootPid);
  const processes = new Map();

  for (const pid of readdirSync('/proc').filter(name => /^\d+$/.test(name))) {
    const current = identity(pid);

    if (current) processes.set(pid, current);
  }

  const initialRoot = processes.get(root);
  assert(initialRoot && initialRoot.state !== 'Z', 'inconclusive process scan: missing live browser root');

  function descendant(pid) {
    const seen = new Set();

    while (processes.has(pid) && pid !== '0') {
      if (pid === root) return true;

      assert(!seen.has(pid), 'inconclusive process scan: cycle in process tree');
      seen.add(pid);
      pid = processes.get(pid).parent;
    }

    return false;
  }

  let checked = 0;
  const roles = new Map();

  for (const [pid, before] of processes) {
    if (!descendant(pid) || before.state === 'Z') continue;

    const raw = readProcess(pid, 'environ');
    const args = readProcess(pid, 'cmdline');
    const after = identity(pid);

    if (after === null) {
      assert(pid !== root, 'inconclusive process scan: browser root disappeared');
      continue;
    }

    assert.equal(after.started, before.started, 'inconclusive process scan: process identity changed');
    assert(raw?.endsWith('\0') && args?.endsWith('\0'), `inconclusive process scan: malformed data for pid ${pid}`);

    const entries = raw.split('\0').slice(0, -1);

    const keys = entries.map(entry => {
      const equals = entry.indexOf('=');
      assert(equals > 0, `inconclusive process scan: malformed environment for pid ${pid}`);

      return entry.slice(0, equals);
    });

    for (const key of forbidden) {
      assert(!keys.includes(key), `inherited forbidden environment key ${key} in pid ${pid}`);
    }

    if (pid === root) {
      assert(entries.includes('HOME=/mnt/scratch/home'));
      assert(entries.includes('FLECHA_LAUNCHER=isolated'));
    }

    const role = /(?:^|[\0 ])--type=([a-z-]+)(?:[\0 ]|$)/.exec(args)?.[1];

    roles.set(pid, role);
    checked++;
  }

  const finalRoot = identity(root);
  assert(finalRoot && finalRoot.state !== 'Z' && finalRoot.started === initialRoot.started,
    'inconclusive process scan: browser root changed');

  // Chromium forks sandboxed renderers from a zygote without exec, so
  // /proc can expose the zygote's original argv for the renderer as well.
  const hasSubprocess = [...roles].some(([pid, role]) =>
    pid !== root && (role === 'renderer' || roles.get(processes.get(pid).parent) === 'zygote'));

  assert(hasSubprocess, 'inconclusive process scan: no live CEF renderer or zygote child was checked');

  return checked;
}
