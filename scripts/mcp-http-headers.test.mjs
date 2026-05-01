import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import test from 'node:test';

const require = createRequire(import.meta.url);
const { ownerCcPidFromTree, processRows } = require('./mcp-http-headers.js');

test('ownerCcPidFromTree skips shell wrappers and returns trusted ancestor', () => {
  const tree = processRows(
    [
      { pid: 10, ppid: 9, name: 'node.exe' },
      { pid: 11, ppid: 10, name: 'cmd.exe' },
      { pid: 12, ppid: 11, name: 'node.exe' },
    ],
    'pid',
    'ppid',
    'name'
  );

  assert.equal(ownerCcPidFromTree(tree, 12), '10');
});

test('ownerCcPidFromTree fails closed when parent row is missing', () => {
  const tree = processRows(
    [{ pid: 12, ppid: 11, name: 'node.exe' }],
    'pid',
    'ppid',
    'name'
  );

  assert.equal(ownerCcPidFromTree(tree, 12), '');
});
