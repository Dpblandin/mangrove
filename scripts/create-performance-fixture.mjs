#!/usr/bin/env node
// Isolated stress cases for issue #17; never reuse an existing directory.
import { spawnSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, resolve, join } from 'node:path';

const destination = resolve(process.argv[2] ?? '.mangrove-dev/performance-17');
mkdirSync(dirname(destination), { recursive: true });
mkdirSync(destination, { recursive: false });
const environment = {
  ...process.env,
  GIT_AUTHOR_NAME: 'Performance Fixture',
  GIT_AUTHOR_EMAIL: 'fixture@example.test',
  GIT_COMMITTER_NAME: 'Performance Fixture',
  GIT_COMMITTER_EMAIL: 'fixture@example.test',
  GIT_AUTHOR_DATE: '2026-01-01T00:00:00Z',
  GIT_COMMITTER_DATE: '2026-01-01T00:00:00Z',
};
function git(args, input) {
  const result = spawnSync('git', ['-c', 'core.hooksPath=', '-c', 'commit.gpgsign=false', '-C', destination, ...args], {
    env: environment, input, encoding: 'utf8', maxBuffer: 8 * 1024 * 1024,
  });
  if (result.status !== 0) throw new Error(result.stderr || `git ${args[0]} failed`);
  return result.stdout.trim();
}
function file(path, content) { writeFileSync(join(destination, path), content); }
function commit(message) {
  git(['add', '-A']);
  git(['commit', '-q', '--allow-empty', '-F', '-'], message);
  return git(['rev-parse', 'HEAD']);
}
function typescript(revision) {
  return Array.from({ length: 150 }, (_, hunk) =>
    [`export const value${hunk} = ${revision};`, ...Array.from({ length: 15 }, (_, line) =>
      `// stable context ${hunk}:${line}`)].join('\n')).join('\n') + '\n';
}
git(['init', '-q', '-b', 'main']);
mkdirSync(join(destination, 'old'));
for (let index = 0; index < 1_000; index++) {
  file(`old/file-${index}.txt`, `unique file ${index}\n${'unchanged content\n'.repeat(8)}`);
}
file('many-hunks.ts', typescript(1));
file('minified.ts', 'export const data = "' + 'x'.repeat(400 * 1024) + '";\n');
file('tabs.txt', '\t'.repeat(64 * 1024) + '\n');
file('large-blob.txt', '0123456789abcdef\n'.repeat(2 * 1024 * 1024));
const base = commit('fixture: initial stress files');
file('many-hunks.ts', typescript(2));
file('minified.ts', 'export const data = "' + 'y'.repeat(400 * 1024) + '";\n');
file('tabs.txt', '\t'.repeat(64 * 1024) + 'changed\n');
const syntax = commit('fixture: syntax and long lines');
git(['mv', 'old', 'renamed']);
git(['mv', 'large-blob.txt', 'large-blob-renamed.txt']);
const renames = commit('fixture: 1000 renames and a large blob');
const tree = git(['rev-parse', 'HEAD^{tree}']);
const firstParentTree = git(['rev-parse', `${syntax}^{tree}`]);
const branches = [];
for (let index = 0; index < 40; index++) {
  const parentTree = index === 0 ? firstParentTree : tree;
  const oid = git(['commit-tree', parentTree, '-p', base], `fixture: divergent lane ${index}\n`);
  branches.push(oid);
  git(['update-ref', `refs/heads/lane-${index}`, oid]);
}
const merge = git(['commit-tree', tree, ...branches.flatMap(oid => ['-p', oid])],
  'fixture: forty divergent parents\n');
git(['update-ref', 'refs/heads/main', merge]);
git(['reset', '--hard', '-q', merge]);
const longSummary = commit('fixture: ' + 'long summary '.repeat(4_000));
git(['update-ref', 'refs/heads/performance-syntax', syntax]);
git(['update-ref', 'refs/heads/performance-renames', renames]);
git(['update-ref', '--stdin'], Array.from({ length: 50_000 }, (_, index) =>
  `create refs/tags/perf-${String(index).padStart(5, '0')} ${base}`).join('\n') + '\n');
git(['pack-refs', '--all']);
console.log(JSON.stringify({ destination, base, syntax, renames, merge, longSummary,
  hunks: 150, rename_files: 1_000, graph_parents: 40, tags: 50_000 }, null, 2));
