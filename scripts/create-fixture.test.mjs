import assert from 'node:assert/strict'
import { execFileSync, spawnSync } from 'node:child_process'
import {
  mkdtempSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { test } from 'node:test'

const generator = fileURLToPath(new URL('./create-fixture.mjs', import.meta.url))
const cleanEnv = Object.fromEntries(
  Object.entries(process.env).filter(([key]) => !key.toUpperCase().startsWith('GIT_')),
)
function temporary(t) {
  const root = mkdtempSync(join(tmpdir(), 'mangrove-fixture-test-'))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  return root
}
function git(path, ...args) {
  return execFileSync(
    'git',
    [
      '-C',
      path,
      '-c',
      'user.name=Fixture test',
      '-c',
      'user.email=fixture@example.test',
      '-c',
      'core.hooksPath=/dev/null',
      ...args,
    ],
    {
      env: {
        ...cleanEnv,
        GIT_CONFIG_GLOBAL: process.platform === 'win32' ? 'NUL' : '/dev/null',
        GIT_CONFIG_NOSYSTEM: '1',
      },
    },
  )
    .toString()
    .trim()
}
function snapshot(directory) {
  const files = {}
  function visit(path, prefix = '') {
    for (const entry of readdirSync(path, { withFileTypes: true })) {
      const relative = join(prefix, entry.name)
      const full = join(path, entry.name)
      if (entry.isDirectory()) visit(full, relative)
      else files[relative] = [readFileSync(full).toString('base64'), statSync(full).mtimeMs]
    }
  }
  visit(directory)
  return files
}
function generate(cwd, env = cleanEnv) {
  return spawnSync(process.execPath, [generator, '--commits', '10'], { cwd, env, encoding: 'utf8' })
}

test('inherited Git location and configuration cannot redirect fixture writes', (t) => {
  const root = temporary(t)
  const source = join(root, 'unrelated-source')
  const workspace = join(root, 'workspace')
  mkdirSync(source)
  mkdirSync(workspace)
  git(source, 'init', '--initial-branch=develop', '--template=')
  writeFileSync(join(source, 'original.txt'), 'tracked source\n')
  git(source, 'add', 'original.txt')
  git(source, 'commit', '-m', 'source commit')
  writeFileSync(join(source, 'original.txt'), 'uncommitted work that must survive\n')
  const before = snapshot(source)
  const sourceGit = join(source, '.git')
  const result = generate(workspace, {
    ...cleanEnv,
    GIT_DIR: sourceGit,
    GIT_WORK_TREE: source,
    GIT_COMMON_DIR: sourceGit,
    GIT_INDEX_FILE: join(sourceGit, 'index'),
    GIT_OBJECT_DIRECTORY: join(sourceGit, 'objects'),
    GIT_ALTERNATE_OBJECT_DIRECTORIES: join(sourceGit, 'objects'),
    GIT_CONFIG_COUNT: '1',
    GIT_CONFIG_KEY_0: 'core.worktree',
    GIT_CONFIG_VALUE_0: source,
  })
  assert.equal(result.status, 0, result.stderr)
  assert.deepEqual(snapshot(source), before)
  const fixture = join(workspace, '.mangrove-dev', 'fixture-10')
  assert.equal(git(fixture, 'rev-list', '--count', '--all'), '15')
  const fixtureBefore = snapshot(fixture)
  assert.equal(generate(workspace).status, 0)
  assert.deepEqual(snapshot(fixture), fixtureBefore, 'reusing a generated fixture is read-only')
})

test('fixture generation refuses unknown files instead of overwriting them', (t) => {
  const root = temporary(t)
  const directory = join(root, '.mangrove-dev', 'fixture-10')
  mkdirSync(directory, { recursive: true })
  writeFileSync(join(directory, 'important.txt'), 'keep me\n')
  const before = snapshot(directory)
  assert.notEqual(generate(root).status, 0)
  assert.deepEqual(snapshot(directory), before)
})

test(
  'fixture generation refuses directory and git-directory symlinks',
  {
    skip: process.platform === 'win32',
  },
  async (t) => {
    for (const target of ['root', 'fixture', 'git']) {
      await t.test(target, (caseTest) => {
        const root = temporary(caseTest)
        const source = join(root, 'unrelated-source')
        const workspace = join(root, 'workspace')
        mkdirSync(source)
        mkdirSync(workspace)
        writeFileSync(join(source, 'important.txt'), 'keep me\n')
        const base = join(workspace, '.mangrove-dev')
        const fixture = join(base, 'fixture-10')
        if (target === 'root') symlinkSync(source, base, 'dir')
        else if (target === 'fixture') {
          mkdirSync(base)
          symlinkSync(source, fixture, 'dir')
        } else {
          mkdirSync(fixture, { recursive: true })
          symlinkSync(source, join(fixture, '.git'), 'dir')
        }
        const before = snapshot(source)
        assert.notEqual(generate(workspace).status, 0)
        assert.deepEqual(snapshot(source), before)
      })
    }
  },
)
