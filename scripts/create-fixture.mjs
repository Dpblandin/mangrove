import { execFileSync } from 'node:child_process'
import { existsSync, lstatSync, mkdirSync, readdirSync, readFileSync, realpathSync } from 'node:fs'
import { resolve } from 'node:path'

// Development fixture only. The application itself never imports or executes Git commands.
const option = process.argv.indexOf('--commits')
const count = option === -1 ? 96 : Number(process.argv[option + 1])
if (!Number.isSafeInteger(count) || count < 10 || count > 100_000) {
  throw new Error('Use --commits with a number between 10 and 100000.')
}
const fixtureRoot = resolve(realpathSync(process.cwd()), '.mangrove-dev')
const directory = resolve(fixtureRoot, 'fixture-' + count)
const gitDirectory = resolve(directory, '.git')
for (const path of [fixtureRoot, directory, gitDirectory]) {
  try {
    const info = lstatSync(path)
    if (info.isSymbolicLink() || !info.isDirectory())
      throw new Error('Fixture paths must be real directories: ' + path)
  } catch (error) {
    if (error.code !== 'ENOENT') throw error
  }
}
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
    [
      '--git-dir=' + gitDirectory,
      '--work-tree=' + directory,
      '-c',
      'core.hooksPath=' + resolve(directory, '.no-hooks'),
      ...args,
    ],
    {
      cwd: directory,
      env,
      input,
      stdio: ['pipe', 'pipe', 'pipe'],
    },
  )
    .toString()
    .trim()
}
if (existsSync(resolve(directory, '.git'))) {
  try {
    git(['rev-parse', '--verify', 'refs/heads/main'])
    if (
      !readFileSync(resolve(directory, 'README.md'), 'utf8').includes(
        'Ce dépôt Git est une fixture synthétique de développement.',
      )
    )
      throw new Error('Not a generated fixture')
  } catch {
    throw new Error(
      'Incomplete development fixture at ' +
        directory +
        '. Remove that fixture directory before retrying.',
    )
  }
  console.log(directory)
} else {
  if (existsSync(directory) && readdirSync(directory).length)
    throw new Error('Refusing to overwrite a non-empty fixture directory: ' + directory)
  mkdirSync(directory, { recursive: true })
  git(['init', '--initial-branch=main', '--template='])
  const start = 1_790_323_200
  const messages = [
    'feat(graph): keep branch lanes stable across history pages',
    'perf(search): reuse the local commit index',
    'fix(viewer): preserve unicode in commit messages',
    'feat(diff): display line numbers and changed files',
    'refactor(core): separate repository reads from the interface',
    'test(repository): verify that source files remain unchanged',
    'feat(history): load visible commits progressively',
    'fix(search): discard results from an older query',
    'perf(cache): bound the number of open history pages',
    'feat(viewer): highlight references in the commit graph',
    'docs(readonly): explain local caches and source repositories',
    'chore(ui): refine spacing in the history view',
  ]
  const authors = [
    'Alice Martin <alice@example.test>',
    'Léa Bernard <lea@example.test>',
    'Noah Dubois <noah@example.test>',
  ]
  let stream = ''
  function data(value) {
    return 'data ' + Buffer.byteLength(value) + '\n' + value + '\n'
  }
  function commit(mark, branch, parent, merges, message, path, content) {
    const time = start + mark * 600
    const author = authors[mark % authors.length]
    stream += 'commit refs/heads/' + branch + '\nmark :' + mark + '\n'
    stream += 'author ' + author + ' ' + time + ' +0200\n'
    stream += 'committer ' + author + ' ' + time + ' +0200\n'
    stream += data(message + '\n')
    if (parent) stream += 'from :' + parent + '\n'
    for (const merge of merges) stream += 'merge :' + merge + '\n'
    stream += 'M 100644 inline ' + path + '\n' + data(content) + '\n'
  }
  for (let index = 1; index <= count; index++) {
    commit(
      index,
      'main',
      index - 1,
      [],
      messages[(index - 1) % messages.length],
      'src/history.ts',
      '// Synthetic development fixture\nexport const history = {\n  pageSize: 128,\n  revision: ' +
        index +
        ',\n  readOnly: true,\n}\n',
    )
  }
  commit(
    count + 1,
    'feat/graph-layout',
    count - 7,
    [],
    'feat(graph): route merge connections through visible lanes',
    'src/graph/layout.ts',
    'export function laneSpacing(width: number, lanes: number) {\n  return Math.min(18, (width - 38) / Math.max(1, lanes - 1))\n}\n',
  )
  commit(
    count + 2,
    'feat/graph-layout',
    count + 1,
    [],
    'fix(graph): align canvas rendering with virtual rows',
    'src/graph/layout.ts',
    'export function laneSpacing(width: number, lanes: number) {\n  return Math.min(18, (width - 38) / Math.max(1, lanes - 1))\n}\n\nexport const rowHeight = 44\n',
  )
  commit(
    count + 3,
    'feat/commit-search',
    count - 4,
    [],
    'feat(search): index messages and authors locally',
    'src/search/index.ts',
    'export const searchFields = ["message", "author", "sha"]\n',
  )
  commit(
    count + 4,
    'feat/commit-search',
    count + 3,
    [],
    'perf(search): cancel stale queries before rendering results',
    'src/search/index.ts',
    'export const searchFields = ["message", "author", "sha", "reference"]\nexport const debounceMs = 160\n',
  )
  commit(
    count + 5,
    'main',
    count,
    [count + 2, count + 4],
    'feat(viewer): connect history, graph and commit details',
    'README.md',
    '# Mangrove · dépôt de démonstration\n\nCe dépôt Git est une fixture synthétique de développement.\nIl sert à vérifier le graphe, la recherche et les diffs du prototype.\n\n- Lecture seule du dépôt source\n- Historique chargé progressivement\n- Index de recherche conservé à part\n',
  )
  stream += 'reset refs/heads/feat/readonly-engine\nfrom :' + (count - 2) + '\n\n'
  stream += 'reset refs/remotes/origin/main\nfrom :' + count + '\n\n'
  stream += 'reset refs/remotes/origin/feat/commit-search\nfrom :' + (count + 4) + '\n\n'
  stream += 'tag v0.1.0\nfrom :' + Math.floor(count * 0.75) + '\n'
  stream += 'tagger Alice Martin <alice@example.test> ' + (start + count * 600) + ' +0200\n'
  stream += data('Development fixture milestone\n')
  git(['fast-import', '--quiet'], stream)
  git(['reset', '--hard', 'main'])
  console.log(directory)
}
