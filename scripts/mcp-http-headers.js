#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const MAX_DEPTH = 40;
const PROC_QUERY_TIMEOUT_MS = 5000;
const SHELL_WRAPPER_NAMES = new Set(['cmd', 'sh', 'bash', 'zsh', 'pwsh', 'powershell', 'conhost']);

function projectRoot() {
  const candidates = [
    process.cwd(),
    path.resolve(__dirname, '..'),
    path.resolve(__dirname, '..', '..'),
  ];
  for (const candidate of candidates) {
    if (fs.existsSync(path.join(candidate, '.agent-teams', 'runtime', 'http-mcp.json'))) {
      return candidate;
    }
  }
  return process.cwd();
}

function runtimeInfoPath() {
  return path.join(projectRoot(), '.agent-teams', 'runtime', 'http-mcp.json');
}

function snapshotProcessTree() {
  if (process.platform === 'win32') {
    const out = execSync(
      'powershell -NoProfile -NonInteractive -Command ' +
      '"Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,Name | ConvertTo-Json -Compress"',
      {
        encoding: 'utf8',
        timeout: PROC_QUERY_TIMEOUT_MS,
        windowsHide: true,
        maxBuffer: 8 * 1024 * 1024,
      }
    );
    const parsed = JSON.parse(out);
    return processRows(Array.isArray(parsed) ? parsed : [parsed], 'ProcessId', 'ParentProcessId', 'Name');
  }
  const out = execSync('ps -eo pid=,ppid=,comm=', {
    encoding: 'utf8',
    timeout: PROC_QUERY_TIMEOUT_MS,
  });
  const rows = out
    .split('\n')
    .map((line) => line.trim().match(/^(\d+)\s+(\d+)\s+(.+)$/))
    .filter(Boolean)
    .map((m) => ({ pid: Number(m[1]), ppid: Number(m[2]), name: m[3] }));
  return processRows(rows, 'pid', 'ppid', 'name');
}

function processRows(rows, pidKey, ppidKey, nameKey) {
  const map = new Map();
  for (const row of rows) {
    const pid = Number(row && row[pidKey]);
    const ppid = Number(row && row[ppidKey]);
    if (Number.isFinite(pid) && Number.isFinite(ppid)) {
      map.set(pid, { ppid, name: String(row[nameKey] || '') });
    }
  }
  return map;
}

function ownerCcPid() {
  try {
    const tree = snapshotProcessTree();
    return ownerCcPidFromTree(tree, process.pid);
  } catch (err) {
    console.error(`[team-mode headers] ancestor walk failed: ${err.message || err}`);
    return '';
  }
}

function ownerCcPidFromTree(tree, startPid) {
    const seen = new Set();
    let pid = startPid;
    let depth = 0;
    while (Number.isFinite(pid) && pid > 0 && !seen.has(pid) && depth < MAX_DEPTH) {
      depth += 1;
      seen.add(pid);
      const current = tree.get(pid);
      if (!current) break;
      const parent = tree.get(current.ppid);
      if (!parent) return '';
      const stem = String(parent.name || '').toLowerCase().replace(/\.exe$/, '');
      if (!SHELL_WRAPPER_NAMES.has(stem)) {
        return String(current.ppid);
      }
      pid = current.ppid;
    }
    return '';
}

function main() {
  const root = projectRoot();
  const info = JSON.parse(fs.readFileSync(runtimeInfoPath(), 'utf8'));
  const tokenFile = path.resolve(root, info.token_file || info.tokenFile);
  const token = fs.readFileSync(tokenFile, 'utf8').trim();
  const headers = {
    Authorization: `Bearer ${token}`,
  };
  const owner = ownerCcPid();
  if (owner) headers['X-Team-Mode-Owner-CC-Pid'] = owner;
  process.stdout.write(JSON.stringify(headers));
}

if (require.main === module) {
  main();
}

module.exports = {
  ownerCcPidFromTree,
  processRows,
};
