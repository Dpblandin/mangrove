#!/usr/bin/env node
// Tracked branches for the ahead/behind benchmark; never reuses an existing directory.
// About 100 000 commits on main and 5 000 local branches, each with an upstream:
// 80 % close to it, 15 % 1 000 to 10 000 commits behind, 2.5 % about 50 000
// behind and 2.5 % on a history without common ancestor. Octopus merges widen
// the last 20 000 commits of main. The pack is rewritten as by `git gc`;
// `--commit-graph` also writes a commit-graph.
import { execFileSync } from 'node:child_process'
import { appendFileSync, mkdirSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'

const destination = resolve(process.argv[2] ?? '.mangrove-dev/tracking')
const commitGraph = process.argv.includes('--commit-graph')
const COMMITS = 100_000
const BRANCHES = 5_000
const ORPHANS = 2_000
mkdirSync(dirname(destination), { recursive: true })
mkdirSync(destination, { recursive: false })
const env = {
  ...Object.fromEntries(
    Object.entries(process.env).filter(([key]) => !key.toUpperCase().startsWith('GIT_')),
  ),
  GIT_CONFIG_GLOBAL: process.platform === 'win32' ? 'NUL' : '/dev/null',
  GIT_CONFIG_NOSYSTEM: '1',
}
function git(args, input) {
  return execFileSync(
    'git',
    ['-C', destination, '-c', 'core.hooksPath=/dev/null', '-c', 'gc.auto=0', ...args],
    { env, input, maxBuffer: 64 * 1024 * 1024, stdio: ['pipe', 'pipe', 'pipe'] },
  )
    .toString()
    .trim()
}

// mulberry32: the same fixture on every run.
let seed = 10
function random() {
  seed = (seed + 0x6d2b79f5) | 0
  let value = Math.imul(seed ^ (seed >>> 15), 1 | seed)
  value = (value + Math.imul(value ^ (value >>> 7), 61 | value)) ^ value
  return ((value ^ (value >>> 14)) >>> 0) / 4294967296
}
const between = (low, high) => low + Math.floor(random() * (high - low + 1))

const start = 1_700_000_000
const chunks = []
let mark = 0
function commit(time, parents, message, file) {
  mark += 1
  const identity = 'Tracking Fixture <fixture@example.test> ' + time + ' +0000\n'
  // fast-import gives a commit without `from` the branch tip as parent.
  if (!parents.length) chunks.push('reset refs/heads/main\n\n')
  chunks.push('commit refs/heads/main\nmark :' + mark + '\n')
  chunks.push('author ' + identity + 'committer ' + identity)
  chunks.push('data ' + Buffer.byteLength(message) + '\n' + message + '\n')
  if (parents.length) chunks.push('from :' + parents[0] + '\n')
  for (const parent of parents.slice(1)) chunks.push('merge :' + parent + '\n')
  if (file) chunks.push('M 100644 inline README.md\ndata ' + file.length + '\n' + file + '\n')
  chunks.push('\n')
  return mark
}

const main = []
for (let index = 0; index < COMMITS; index++) {
  const time = start + index * 600
  const parents = index ? [main[index - 1]] : []
  // Every hundredth commit of the last 20 000 merges eight three-commit branches.
  if (index >= COMMITS - 20_000 && index % 100 === 0) {
    for (let side = 0; side < 8; side++) {
      let tip = main[index - 30]
      for (let step = 0; step < 3; step++) {
        tip = commit(time - 600 + side * 9 + step * 3, [tip], 'side ' + index + '.' + side)
      }
      parents.push(tip)
    }
  }
  main.push(commit(time, parents, 'main ' + index, index ? '' : 'Tracking fixture\n'))
}
const orphans = []
for (let index = 0; index < ORPHANS; index++) {
  const time = start + Math.floor((index * COMMITS * 600) / ORPHANS) + 300
  orphans.push(commit(time, index ? [orphans[index - 1]] : [], 'orphan ' + index))
}
function topic(base, baseTime, count, label) {
  let tip = base
  for (let step = 0; step < count; step++) {
    tip = commit(baseTime + 60 * (step + 1), [tip], label + ' ' + step)
  }
  return tip
}

const refs = []
const config = ['[remote "origin"]', '\turl = https://example.invalid/tracking.git']
config.push('\tfetch = +refs/heads/*:refs/remotes/origin/*')
function track(name, local, upstream) {
  refs.push('reset refs/heads/' + name + '\nfrom :' + local + '\n\n')
  refs.push('reset refs/remotes/origin/' + name + '\nfrom :' + upstream + '\n\n')
  config.push('[branch "' + name + '"]', '\tremote = origin', '\tmerge = refs/heads/' + name)
}
// main: two local commits, three behind.
const tip = COMMITS - 1
track('main', topic(main[tip - 3], start + tip * 600, 2, 'local main'), main[tip])
const categories = { close: 0, far: 0, deep: 0, unrelated: 0 }
for (let index = 0; index < BRANCHES - 1; index++) {
  const name = 'area-' + (index % 16) + '/user-' + ((index * 7) % 50) + '/topic-' + index
  const upstream = tip - between(0, 2_000)
  const draw = random()
  if (draw < 0.025) {
    categories.unrelated += 1
    const position = between(0, ORPHANS - 1)
    track(name, orphans[position], main[upstream])
    continue
  }
  let behind
  let ahead
  if (draw < 0.05) {
    categories.deep += 1
    behind = between(40_000, 60_000)
    ahead = between(0, 2)
  } else if (draw < 0.2) {
    categories.far += 1
    behind = between(1_000, 10_000)
    ahead = between(0, 3)
  } else {
    categories.close += 1
    behind = random() < 0.4 ? 0 : between(1, 40)
    ahead = random() < 0.4 ? 0 : between(1, 10)
  }
  const base = upstream - behind
  track(name, topic(main[base], start + base * 600 + 30, ahead, name), main[upstream])
}
git(['init', '-q', '-b', 'main', '--template='])
git(['fast-import', '--quiet'], chunks.join('') + refs.join(''))
appendFileSync(join(destination, '.git', 'config'), config.join('\n') + '\n')
git(['pack-refs', '--all'])
git(['reset', '-q', '--hard', 'main'])
// Like `git gc`: the pack of fast-import is larger and less compressed.
git(['repack', '-a', '-d', '-f', '-q'])
if (commitGraph) git(['commit-graph', 'write', '--reachable'])
console.log(
  JSON.stringify(
    { destination, commits: mark, tracked: BRANCHES, ...categories, commitGraph },
    null,
    2,
  ),
)
