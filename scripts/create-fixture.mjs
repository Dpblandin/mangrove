import { execFileSync } from 'node:child_process'
import { existsSync, lstatSync, mkdirSync, readdirSync, realpathSync } from 'node:fs'
import { resolve } from 'node:path'

// Development fixture only. The application itself never imports or executes Git commands.
const option = process.argv.indexOf('--commits')
const count = option === -1 ? 96 : Number(process.argv[option + 1])
const largeChange = process.argv.includes('--large-change')
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
      !git(['show', 'refs/heads/main:README.md']).includes(
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
}
if (largeChange) appendLargeChange()
console.log(directory)

function appendLargeChange() {
  const tag = 'refs/tags/mangrove-demo-changes-v1'
  let existing = ''
  try {
    existing = git(['rev-parse', '--verify', tag])
  } catch {
    // The optional demonstration has not been added to this fixture yet.
  }
  if (existing) {
    git(['merge-base', '--is-ancestor', existing, 'refs/heads/main'])
    return
  }
  if (
    git(['symbolic-ref', '--short', 'HEAD']) !== 'main' ||
    git(['status', '--porcelain', '--untracked-files=all'])
  )
    throw new Error('The large-change demo requires a clean fixture on main: ' + directory)

  const parent = git(['rev-parse', 'refs/heads/main'])
  const timestamp = Number(git(['show', '-s', '--format=%ct', parent]))
  const chunks = []
  const text = (value) => chunks.push(Buffer.from(value))
  function data(value) {
    const bytes = Buffer.isBuffer(value) ? value : Buffer.from(value)
    text('data ' + bytes.length + '\n')
    chunks.push(bytes)
    text('\n')
  }
  function commit(mark, from, message) {
    text('commit refs/heads/main\nmark :' + mark + '\n')
    const identity = 'Alice Martin <alice@example.test> ' + (timestamp + mark * 600) + ' +0200\n'
    text('author ' + identity + 'committer ' + identity)
    data(message + '\n')
    text('from ' + from + '\n')
  }
  function file(path, content, mode = '100644') {
    text('M ' + mode + ' inline ' + JSON.stringify(path) + '\n')
    data(content)
  }
  function remove(path) {
    text('D ' + JSON.stringify(path) + '\n')
  }
  function service(name, readOnly) {
    return [
      '// Synthetic fixture: ' + name,
      'export class ' + name + ' {',
      '  readonly pageSize = 128',
      '  readonly cacheLimit = 12',
      '  readonly timeoutMs = 5000',
      '  readonly batchSize = 256',
      '  readonly readOnly = ' + readOnly,
      '',
      '  describe() {',
      '    return {',
      '      name: "' + name + '",',
      '      pageSize: this.pageSize,',
      '      cacheLimit: this.cacheLimit,',
      '      timeoutMs: this.timeoutMs,',
      '      batchSize: this.batchSize,',
      '      readOnly: this.readOnly,',
      '    }',
      '  }',
      '}',
      '',
    ].join('\n')
  }
  const modified = [
    [
      'apps/frontend/src/components/RepositoryHeader.vue',
      '<template>\n  <header>Repositories</header>\n</template>\n',
      '<template>\n  <header class="repository-header">\n    <h1>Repositories</h1>\n    <span>Local workspace</span>\n  </header>\n</template>\n',
    ],
    [
      'apps/frontend/src/stores/repositories.ts',
      'export const repositories = []\n',
      'export const repositories = []\nexport const pageSize = 128\nexport const readOnly = true\n',
    ],
    [
      'apps/frontend/src/styles/viewer.css',
      '.viewer {\n  background: white;\n  color: black;\n}\n',
      '.viewer {\n  background: #1e2025;\n  color: #dedee4;\n  overflow: auto;\n}\n',
    ],
    [
      'apps/frontend/src/locales/fr.json',
      '{\n  "title": "Dépôts",\n  "empty": "Vide"\n}\n',
      '{\n  "title": "Dépôts locaux",\n  "empty": "Aucun dépôt ouvert",\n  "files": "Fichiers modifiés"\n}\n',
    ],
    [
      'apps/backend/src/RepositoryReader.php',
      '<?php\nfinal class RepositoryReader\n{\n    public const PAGE_SIZE = 64;\n}\n',
      '<?php\nfinal class RepositoryReader\n{\n    public const PAGE_SIZE = 128;\n    public const READ_ONLY = true;\n}\n',
    ],
    [
      'apps/backend/tests/RepositoryReaderTest.php',
      '<?php\n// Synthetic repository reader test\nassert(64 > 0);\n',
      '<?php\n// Synthetic repository reader test\nassert(128 > 0);\nassert(true === true);\n',
    ],
    [
      'packages/core/src/cache.ts',
      'export const cache = { pages: 4, enabled: false }\n',
      'export const cache = {\n  pages: 12,\n  enabled: true,\n  strategy: "lru",\n}\n',
    ],
    [
      'packages/core/src/search.ts',
      'export const fields = ["message"]\n',
      'export const fields = ["message", "author", "sha", "reference"]\nexport const debounceMs = 160\n',
    ],
    [
      'config/viewer.yml',
      'viewer:\n  theme: light\n  page_size: 64\n',
      'viewer:\n  theme: classic\n  page_size: 128\n  diff_mode: central\n',
    ],
    [
      'config/cache.toml',
      '[cache]\npages = 4\nenabled = false\n',
      '[cache]\npages = 12\nenabled = true\nworkers = 2\n',
    ],
    [
      'config/shortcuts.xml',
      '<shortcuts>\n  <search key="F" />\n</shortcuts>\n',
      '<shortcuts>\n  <search key="K" modifier="command" />\n  <close-diff key="Escape" />\n</shortcuts>\n',
    ],
    [
      'docs/architecture.md',
      '# Architecture\n\nOne reader per repository.\n',
      '# Architecture\n\nA shared workspace owns repository readers.\n\n- Bounded history pages\n- Atomic commit and diff snapshots\n- Read-only source repositories\n',
    ],
    [
      'docs/CHANGELOG.md',
      '# Changelog\n\n## Prototype\n\n- History\n',
      '# Changelog\n\n## Viewer\n\n- Central diff view\n- Resizable panels\n- Multiple themes\n\n## Prototype\n\n- History\n',
    ],
    [
      'data/repositories.csv',
      'name,commits,provider\ndemo,96,local\n',
      'name,commits,provider\ndemo,10000,local\nfrontend,2400,github\nbackend,7200,gitlab\n',
    ],
    [
      'migrations/001_cache.sql',
      'CREATE TABLE commits (sha TEXT PRIMARY KEY);\n',
      'CREATE TABLE commits (\n  sha TEXT PRIMARY KEY,\n  message TEXT NOT NULL,\n  author TEXT NOT NULL\n);\nCREATE INDEX commits_author ON commits(author);\n',
    ],
    [
      'scripts/inspect.sh',
      '#!/bin/sh\n# Synthetic fixture; never executed by Mangrove.\nprintf "fixture\\n"\n',
      '#!/bin/sh\n# Synthetic fixture; never executed by Mangrove.\nprintf "read-only fixture\\n"\n',
      '100755',
    ],
    [
      'notes/release.txt',
      'Preview without a final newline',
      'Stable preview\nWith a final newline\n',
    ],
    [
      'assets/preview.bin',
      Buffer.from([0, 77, 71, 82, 1, 2, 3, 4]),
      Buffer.from([0, 77, 71, 82, 5, 6, 7, 8, 9]),
    ],
  ]
  const renamed = [
    [
      'apps/backend/src/LegacyCommitReader.ts',
      'apps/backend/src/readers/CommitReader.ts',
      'CommitReader',
    ],
    [
      'apps/backend/src/LegacyDiffReader.ts',
      'apps/backend/src/readers/DiffReader.ts',
      'DiffReader',
    ],
    [
      'packages/core/src/LegacyHistoryCache.ts',
      'packages/core/src/cache/HistoryCache.ts',
      'HistoryCache',
    ],
    [
      'packages/core/src/LegacySearchIndex.ts',
      'packages/core/src/search/SearchIndex.ts',
      'SearchIndex',
    ],
    [
      'apps/frontend/src/legacy/ThemeRegistry.ts',
      'apps/frontend/src/themes/ThemeRegistry.ts',
      'ThemeRegistry',
    ],
    [
      'apps/frontend/src/legacy/PanelLayout.ts',
      'apps/frontend/src/layout/PanelLayout.ts',
      'PanelLayout',
    ],
  ]
  const deleted = Array.from(
    { length: 8 },
    (_, index) => 'packages/legacy/src/ObsoleteAdapter' + (index + 1) + '.ts',
  )
  const added = [
    [
      'apps/frontend/src/components/ChangedFiles.vue',
      '<template>\n  <ul class="changed-files">\n    <li v-for="file in files" :key="file.path">{{ file.path }}</li>\n  </ul>\n</template>\n',
    ],
    [
      'apps/frontend/src/composables/useDiff.ts',
      'export function useDiff() {\n  return { mode: "central", readOnly: true }\n}\n',
    ],
    [
      'apps/backend/src/ReadOnlyWorkspace.php',
      '<?php\nfinal class ReadOnlyWorkspace\n{\n    public function capabilities(): array\n    {\n        return ["history", "details", "diff"];\n    }\n}\n',
    ],
    [
      'tests/diff-cases.json',
      '{\n  "cases": ["added", "modified", "deleted", "renamed", "binary", "typechange"]\n}\n',
    ],
    [
      'tests/fixtures/expected-diff.txt',
      '@@ -1,2 +1,3 @@\n-context\n+updated context\n+new line\n unchanged\n',
    ],
    [
      'migrations/002_search.sql',
      'CREATE VIRTUAL TABLE commit_search USING fts5(message, author);\n',
    ],
    [
      'docs/Guide de lecture.md',
      '# Guide de lecture\n\nSélectionnez un commit, puis un fichier pour lire son diff.\n',
    ],
    [
      'docs/échantillons/résumé.md',
      '# Résumé des changements\n\nAjouts, suppressions, renommages et modifications dans un même commit.\n',
    ],
    [
      'packages/core/src/capabilities.ts',
      'export const capabilities = ["history", "details", "diff"] as const\n',
    ],
    ['tests/fixtures/empty.txt', ''],
    ['assets/new-preview.bin', Buffer.from([0, 68, 73, 70, 70, 10, 20, 30])],
    [
      'src/generated/catalog.ts',
      '// Large synthetic diff: the viewer should truncate this patch.\n' +
        Array.from(
          { length: 2400 },
          (_, index) =>
            'export const entry' +
            index +
            ' = { id: ' +
            index +
            ', label: "Fixture item ' +
            index +
            '" }\n',
        ).join(''),
    ],
  ]

  commit(1, parent, 'test(fixture): prepare mixed file changes')
  for (const [path, before] of modified) file(path, before)
  for (const [before, , name] of renamed) file(before, service(name, false))
  for (const path of deleted)
    file(path, '// Retired synthetic adapter: ' + path + '\nexport const obsolete = true\n')
  file('config/current-viewer', 'viewer.yml', '120000')
  file('docs/current-guide', 'The guide will move to a symbolic link.\n')
  text('\n')

  commit(2, ':1', 'feat(fixture): showcase 46 mixed file changes')
  for (const [path, , after, mode] of modified) file(path, after, mode)
  for (const [before, after, name] of renamed) {
    remove(before)
    file(after, service(name, name === 'ThemeRegistry' || name === 'PanelLayout'))
  }
  for (const path of deleted) remove(path)
  for (const [path, content] of added) file(path, content)
  file('config/current-viewer', 'viewer:\n  theme: classic\n  read_only: true\n')
  file('docs/current-guide', 'Guide de lecture.md', '120000')
  text('\nreset ' + tag + '\nfrom :2\n\n')
  git(['fast-import', '--quiet'], Buffer.concat(chunks))
  git(['reset', '--hard', 'main'])
}
