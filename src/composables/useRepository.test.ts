import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test'
import { effectScope } from 'vue'
import type { GitClient } from '../lib/api'
import type { HighlightedDiff, SyntaxRequest, SyntaxResponse } from '../lib/diffSyntax'
import type {
  ChangedFile,
  CommitDetails,
  CommitPage,
  CommitRow,
  FileDiff,
  IndexProgress,
  RepositoryInfo,
} from '../lib/types'
import { PAGE_SIZE, useRepository } from './useRepository'

interface PendingCall {
  command: string
  args: Record<string, unknown>
  signal: AbortSignal | undefined
  settled: boolean
  resolve(value: unknown): void
  reject(reason: unknown): void
}

function delayedClient() {
  const calls: PendingCall[] = []
  // Deliberately ignore abort: Tauri commands may complete after the view changes.
  const client: GitClient = <T>(
    command: string,
    args: Record<string, unknown>,
    signal?: AbortSignal,
  ) =>
    new Promise<T>((resolve, reject) => {
      const call: PendingCall = {
        command,
        args,
        signal,
        settled: false,
        resolve(value) {
          call.settled = true
          resolve(value as T)
        },
        reject(reason) {
          call.settled = true
          reject(reason)
        },
      }
      calls.push(call)
    })
  function take(command: string, args: Record<string, unknown> = {}) {
    const call = calls.find(
      (candidate) =>
        !candidate.settled &&
        candidate.command === command &&
        Object.entries(args).every(([key, value]) => candidate.args[key] === value),
    )
    if (!call) throw new Error(`Missing ${command} request: ${JSON.stringify(args)}`)
    return call
  }
  return { client, calls, take }
}

const index = (indexed: number, complete = true): IndexProgress => ({
  indexed,
  complete,
  error: null,
})
const repository = (id = 'repo', count = 1, complete = true): RepositoryInfo => ({
  id,
  name: id,
  path: `/tmp/${id}`,
  head: null,
  headName: 'main',
  references: [],
  remotes: [],
  index: index(count, complete),
})
const row = (position: number, sha = `commit-${position}`): CommitRow => ({
  sha,
  position,
  summary: sha,
  author: 'Author',
  email: 'author@example.test',
  time: 0,
  parents: [],
  graph: { lane: 0, color: 0, width: 1, edges: [] },
})
const page = (
  commits: CommitRow[] = [row(0)],
  total = commits.length,
  status = index(total),
  offset = 0,
): CommitPage => ({ commits, total, offset, index: status })
const fullPage = (offset: number, total: number, status = index(total)): CommitPage =>
  page(
    Array.from({ length: Math.min(PAGE_SIZE, Math.max(0, total - offset)) }, (_, position) =>
      row(offset + position),
    ),
    total,
    status,
    offset,
  )
const file = (position: number): ChangedFile => ({
  index: position,
  path: `file-${position}.ts`,
  oldPath: null,
  status: 'modified',
})
const details = (sha: string, files: ChangedFile[] = []): CommitDetails => ({
  sha,
  message: sha,
  author: 'Author',
  email: 'author@example.test',
  time: 0,
  parents: [],
  files,
  fileCount: files.length,
})
const diff = (additions: number): FileDiff => ({
  additions,
  deletions: 0,
  binary: false,
  truncated: false,
  hunks: [],
})
const textDiff = (content: string): FileDiff => ({
  ...diff(1),
  hunks: [
    {
      header: '@@ -0,0 +1 @@',
      lines: [{ kind: 'addition', oldLine: null, newLine: 1, content }],
    },
  ],
})
const colored = (content: string): HighlightedDiff => [[[{ content, kind: 'string' }]]]

function controlledWorkers() {
  const workers: ControlledWorker[] = []
  class ControlledWorker {
    onmessage: ((event: MessageEvent<SyntaxResponse>) => void) | null = null
    onerror: ((event: Event) => void) | null = null
    onmessageerror: ((event: MessageEvent) => void) | null = null
    requests: SyntaxRequest[] = []
    terminate = vi.fn()

    constructor() {
      workers.push(this)
    }

    postMessage(request: SyntaxRequest) {
      this.requests.push(request)
    }

    complete(tokens: HighlightedDiff | null, request = this.requests.at(-1)!) {
      // A terminated worker can still deliver a message queued by the host.
      this.onmessage?.({ data: { id: request.id, tokens } } as MessageEvent<SyntaxResponse>)
    }
  }
  vi.stubGlobal('Worker', ControlledWorker)
  return workers
}

async function flush() {
  for (let step = 0; step < 12; step += 1) await Promise.resolve()
}

let cleanup: (() => void)[]
function setup() {
  const transport = delayedClient()
  const scope = effectScope()
  const state = scope.run(() => useRepository(transport.client))!
  cleanup.push(() => scope.stop())
  return { state, transport, scope }
}

async function openReady(
  state: ReturnType<typeof useRepository>,
  transport: ReturnType<typeof delayedClient>,
  info = repository(),
  firstPage = page(),
) {
  const opening = state.open(info.path)
  transport.take('open_repository').resolve(info)
  await flush()
  transport.take('list_commits', { offset: 0, sessionId: info.id }).resolve(firstPage)
  await opening
  await flush()
}

async function openDisplayed(
  state: ReturnType<typeof useRepository>,
  transport: ReturnType<typeof delayedClient>,
) {
  await openReady(state, transport, repository('repo', 3), fullPage(0, 3))
  transport
    .take('commit_details', { sha: 'commit-0' })
    .resolve(details('commit-0', [file(0), file(1)]))
  await flush()
  transport.take('file_diff', { sha: 'commit-0', fileIndex: 0 }).resolve(diff(3))
  await flush()
}

beforeEach(() => {
  vi.useFakeTimers()
  cleanup = []
})

describe('background history refresh', () => {
  it('shares concurrent position lookups and retries when the index advances', async () => {
    const { state, transport } = setup()
    await openDisplayed(state, transport)
    const refreshing = state.refresh()
    transport
      .take('open_repository')
      .resolve({ ...repository('updated', 0, false), path: '/tmp/repo' })
    await flush()
    transport.take('list_commits', { sessionId: 'updated' }).resolve(page([], 0, index(0, false)))
    await flush()
    const position = transport.take('commit_position', { sessionId: 'updated' })
    transport.take('index_status', { sessionId: 'updated' }).resolve(index(0, false))
    await flush()
    expect(
      transport.calls.filter(
        (call) => call.command === 'commit_position' && call.args.sessionId === 'updated',
      ),
    ).toHaveLength(1)
    position.resolve(null)
    await refreshing
    await vi.advanceTimersByTimeAsync(350)
    transport.take('index_status', { sessionId: 'updated' }).resolve(index(3))
    await flush()
    transport
      .take('list_commits', { sessionId: 'updated' })
      .resolve(page([row(0, 'new'), row(1, 'commit-0'), row(2, 'commit-1')]))
    await flush()
    transport.take('commit_position', { sessionId: 'updated' }).resolve(1)
    await flush()
    expect(state.selectedIndex.value).toBe(1)
    expect(state.selectedSha.value).toBe('commit-0')
  })

  it.each([1, PAGE_SIZE + 20])(
    'reanchors the selected commit at position %i after an initially empty index finishes',
    async (position) => {
      const { state, transport } = setup()
      await openDisplayed(state, transport)
      const displayed = state.details.value
      const displayedDiff = state.diff.value
      const refreshing = state.refresh()
      transport
        .take('open_repository')
        .resolve({ ...repository('updated', 0, false), path: '/tmp/repo' })
      await flush()
      transport.take('list_commits', { sessionId: 'updated' }).resolve(page([], 0, index(0, false)))
      await flush()
      transport.take('commit_position', { sessionId: 'updated' }).resolve(null)
      await refreshing
      expect(state.selectedSha.value).toBe('commit-0')
      expect(state.details.value).toBe(displayed)
      expect(state.diff.value).toBe(displayedDiff)

      const count = position + 3
      transport.take('index_status', { sessionId: 'updated' }).resolve(index(count))
      await flush()
      transport.take('list_commits', { sessionId: 'updated' }).resolve(
        page(
          Array.from({ length: Math.min(PAGE_SIZE, count) }, (_, current) =>
            row(current, current === position ? 'commit-0' : `new-${current}`),
          ),
          count,
        ),
      )
      await flush()
      transport.take('commit_position', { sessionId: 'updated' }).resolve(position)
      await flush()
      expect(state.progress.value.complete).toBe(true)
      expect(state.selectedIndex.value).toBe(position)
      expect(state.selectedSha.value).toBe('commit-0')
      expect(state.details.value).toBe(displayed)
      expect(state.diff.value).toBe(displayedDiff)
    },
  )

  it('keeps a newer selection when an indexing position lookup responds late', async () => {
    const { state, transport } = setup()
    await openDisplayed(state, transport)
    const refreshing = state.refresh()
    transport
      .take('open_repository')
      .resolve({ ...repository('updated', 0, false), path: '/tmp/repo' })
    await flush()
    transport.take('list_commits', { sessionId: 'updated' }).resolve(page([], 0, index(0, false)))
    await flush()
    transport.take('commit_position', { sessionId: 'updated' }).resolve(null)
    await refreshing
    transport.take('index_status', { sessionId: 'updated' }).resolve(index(3))
    await flush()
    const rows = [row(0, 'new'), row(1, 'commit-0'), row(2, 'commit-1')]
    transport.take('list_commits', { sessionId: 'updated' }).resolve(page(rows))
    await flush()
    const oldPosition = transport.take('commit_position', { sessionId: 'updated' })
    const selecting = state.select(rows[2]!, 2)
    transport.take('commit_details', { sha: 'commit-1' }).resolve(details('commit-1'))
    await selecting
    oldPosition.resolve(1)
    await flush()
    expect(state.selectedSha.value).toBe('commit-1')
    expect(state.selectedIndex.value).toBe(2)
    expect(state.details.value?.sha).toBe('commit-1')
  })

  it('reanchors a filtered selection as results arrive during the renewed index', async () => {
    const { state, transport } = setup()
    await openDisplayed(state, transport)
    state.query.value = 'feature'
    await vi.advanceTimersByTimeAsync(160)
    transport.take('list_commits', { query: 'feature' }).resolve(page([row(0, 'feature-commit')]))
    await flush()
    transport.take('commit_details', { sha: 'feature-commit' }).resolve(details('feature-commit'))
    await flush()
    const displayed = state.details.value
    const refreshing = state.refresh()
    transport
      .take('open_repository')
      .resolve({ ...repository('updated', 0, false), path: '/tmp/repo' })
    await flush()
    transport
      .take('list_commits', { sessionId: 'updated', query: 'feature' })
      .resolve(page([], 0, index(0, false)))
    await refreshing
    transport.take('index_status', { sessionId: 'updated' }).resolve(index(4))
    await flush()
    transport
      .take('list_commits', { sessionId: 'updated', query: 'feature' })
      .resolve(page([row(0, 'new-feature'), row(3, 'feature-commit')], 2, index(4)))
    await flush()
    expect(state.progress.value.complete).toBe(true)
    expect(state.selectedIndex.value).toBe(1)
    expect(state.selectedSha.value).toBe('feature-commit')
    expect(state.filter.value).toBe('feature')
    expect(state.details.value).toBe(displayed)
  })

  it('keeps the selected commit and diff, and leaves an unchanged session alone', async () => {
    const { state, transport } = setup()
    await openDisplayed(state, transport)
    const displayed = state.details.value
    const displayedDiff = state.diff.value
    const unchanged = state.refresh()
    transport.take('open_repository').resolve(repository('repo', 3))
    await unchanged
    expect(state.details.value).toBe(displayed)
    expect(state.diff.value).toBe(displayedDiff)
    expect(transport.calls.filter((call) => call.command === 'list_commits')).toHaveLength(1)

    const refreshing = state.refresh()
    transport.take('open_repository').resolve({ ...repository('updated', 4), path: '/tmp/repo' })
    await flush()
    expect(state.repository.value?.id).toBe('updated')
    expect(state.opening.value).toBe(false)
    expect(state.selectedSha.value).toBe('commit-0')
    expect(state.details.value).toBe(displayed)
    expect(state.diff.value).toBe(displayedDiff)
    transport
      .take('list_commits', { sessionId: 'updated' })
      .resolve(page([row(0, 'new'), row(1, 'commit-0'), row(2, 'commit-1'), row(3, 'commit-2')]))
    await flush()
    transport.take('commit_position', { sessionId: 'updated' }).resolve(1)
    await refreshing
    expect(state.selectedIndex.value).toBe(1)
    expect(state.details.value).toBe(displayed)
    expect(state.diff.value).toBe(displayedDiff)
  })

  it('preserves the active search during a refresh', async () => {
    const { state, transport } = setup()
    await openDisplayed(state, transport)
    state.query.value = 'feature'
    await vi.advanceTimersByTimeAsync(160)
    transport.take('list_commits', { query: 'feature' }).resolve(page([row(0, 'feature-commit')]))
    await flush()
    transport
      .take('commit_details', { sha: 'feature-commit' })
      .resolve(details('feature-commit', []))
    await flush()
    const refreshing = state.refresh()
    transport.take('open_repository').resolve({ ...repository('updated'), path: '/tmp/repo' })
    await flush()
    transport
      .take('list_commits', { sessionId: 'updated', query: 'feature' })
      .resolve(page([row(0, 'feature-commit')]))
    await refreshing
    expect(state.query.value).toBe('feature')
    expect(state.filter.value).toBe('feature')
    expect(state.selectedSha.value).toBe('feature-commit')
    expect(state.details.value?.sha).toBe('feature-commit')
  })

  it('ignores an obsolete refresh after another repository opens', async () => {
    const { state, transport } = setup()
    await openDisplayed(state, transport)
    const refreshing = state.refresh()
    const oldOpen = transport.take('open_repository')
    const opening = state.open('/tmp/second')
    transport.take('open_repository', { path: '/tmp/second' }).resolve(repository('second'))
    await flush()
    transport.take('list_commits', { sessionId: 'second' }).resolve(page())
    await opening
    oldOpen.resolve({ ...repository('updated'), path: '/tmp/repo' })
    await refreshing
    expect(state.repository.value?.id).toBe('second')
  })
})

afterEach(() => {
  for (const dispose of cleanup) dispose()
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

describe('repository navigation', () => {
  it('closes the active view and ignores a diff that finishes after the last tab closes', async () => {
    const { state, transport } = setup()
    await openDisplayed(state, transport)
    const reading = state.selectFile(file(1))
    const pendingDiff = transport.take('file_diff', { fileIndex: 1 })

    state.close()
    pendingDiff.resolve(diff(99))
    await reading

    expect(pendingDiff.signal?.aborted).toBe(true)
    expect(state.repository.value).toBeNull()
    expect(state.pages.value.size).toBe(0)
    expect(state.total.value).toBe(0)
    expect(state.selectedSha.value).toBe('')
    expect(state.details.value).toBeNull()
    expect(state.diff.value).toBeNull()
    expect(state.opening.value).toBe(false)
  })

  it('ignores a repository opening that completes after its tab closes', async () => {
    const { state, transport } = setup()
    const opening = state.open('/tmp/closed-tab')
    const pendingOpen = transport.take('open_repository')

    state.close()
    pendingOpen.resolve(repository('closed-tab'))

    expect(await opening).toBe(false)
    expect(pendingOpen.signal?.aborted).toBe(true)
    expect(state.repository.value).toBeNull()
    expect(state.opening.value).toBe(false)
    expect(transport.calls.filter((call) => call.command === 'list_commits')).toHaveLength(0)
  })

  it('discards old repository results even when transport cancellation is ignored', async () => {
    const { state, transport } = setup()
    const firstOpen = state.open('/tmp/first')
    transport.take('open_repository').resolve(repository('first'))
    await flush()
    const oldPage = transport.take('list_commits')

    await openReady(state, transport, repository('second'), page([row(0, 'second-commit')]))
    transport.take('commit_details', { sha: 'second-commit' }).resolve(details('second-commit'))
    await flush()
    oldPage.resolve(page([row(0, 'old-commit')], 900))
    expect(await firstOpen).toBe(false)

    expect(oldPage.signal?.aborted).toBe(true)
    expect(state.repository.value?.id).toBe('second')
    expect(state.getRow(0)?.sha).toBe('second-commit')
    expect(state.selectedSha.value).toBe('second-commit')
    expect(state.details.value?.sha).toBe('second-commit')
    expect(state.total.value).toBe(1)
    expect(state.opening.value).toBe(false)
    expect(transport.calls.filter((call) => call.command === 'index_status')).toHaveLength(0)
  })

  it('clears a previous repository when opening an invalid path, including late details', async () => {
    const { state, transport } = setup()
    await openReady(state, transport, repository('valid', 200), fullPage(0, 200))
    const oldDetails = transport.take('commit_details')
    const opening = state.open('/tmp/not-a-repository')
    transport.take('open_repository').reject(new Error('Not a Git repository'))
    expect(await opening).toBe(false)
    oldDetails.resolve(details('commit-0', [file(0)]))
    await flush()

    expect(state.repository.value).toBeNull()
    expect(state.progress.value.indexed).toBe(0)
    expect(state.pages.value.size).toBe(0)
    expect(state.total.value).toBe(0)
    expect(state.selectedSha.value).toBe('')
    expect(state.details.value).toBeNull()
    expect(state.diff.value).toBeNull()
    expect(state.detailLoading.value).toBe(false)
    expect(state.error.value).toContain('Not a Git repository')
    expect(transport.calls.filter((call) => call.command === 'file_diff')).toHaveLength(0)
  })

  it('keeps the latest commit and file selection when responses arrive out of order', async () => {
    const { state, transport } = setup()
    await openReady(state, transport)
    const initialDetails = transport.take('commit_details')
    const second = state.select(row(1), 1)
    const secondDetails = transport.take('commit_details', { sha: 'commit-1' })
    secondDetails.resolve(details('commit-1', [file(0), file(1), file(2)]))
    await flush()
    transport.take('file_diff', { sha: 'commit-1', fileIndex: 0 }).resolve(diff(3))
    await second
    const oldSelection = state.selectFile(file(1))
    const initialDiff = transport.take('file_diff', { fileIndex: 1 })
    const selectingFile = state.selectFile(file(2))
    transport.take('file_diff', { fileIndex: 2 }).resolve(diff(42))
    await selectingFile
    initialDetails.reject(new Error('Old details failed'))
    initialDiff.resolve(diff(999))
    await oldSelection

    expect(initialDetails.signal?.aborted).toBe(true)
    expect(initialDiff.signal?.aborted).toBe(true)
    expect(state.selectedSha.value).toBe('commit-1')
    expect(state.selectedIndex.value).toBe(1)
    expect(state.details.value?.sha).toBe('commit-1')
    expect(state.selectedFile.value?.path).toBe('file-2.ts')
    expect(state.diff.value?.additions).toBe(42)
    expect(state.detailError.value).toBe('')
    expect(state.diffError.value).toBe('')
    expect(state.detailLoading.value).toBe(false)
    expect(state.diffLoading.value).toBe(false)
  })

  it('clears search before jumping to a reference and ignores a superseded jump', async () => {
    const { state, transport } = setup()
    await openReady(state, transport, repository('repo', 400), fullPage(0, 400))
    state.query.value = 'missing'
    await vi.advanceTimersByTimeAsync(160)
    const search = transport.take('list_commits', { query: 'missing' })
    const firstJump = state.jump('first')
    const oldPosition = transport.take('commit_position', { sha: 'first' })
    const latestJump = state.jump('latest')
    transport.take('commit_position', { sha: 'latest' }).resolve(130)
    await flush()
    const targetPage = transport.take('list_commits', { query: '', offset: 128 })
    const targetRows = fullPage(128, 400)
    targetRows.commits[2] = row(130, 'latest')
    targetPage.resolve(targetRows)
    expect(await latestJump).toBe(130)
    oldPosition.resolve(0)
    expect(await firstJump).toBeNull()
    search.resolve(page([], 0, index(400)))
    await vi.advanceTimersByTimeAsync(160)

    expect(search.signal?.aborted).toBe(true)
    expect(oldPosition.signal?.aborted).toBe(true)
    expect(state.query.value).toBe('')
    expect(state.filter.value).toBe('')
    expect(state.selectedSha.value).toBe('latest')
    expect(state.selectedIndex.value).toBe(130)
    expect(state.getRow(130)?.sha).toBe('latest')
    expect(state.total.value).toBe(400)
    expect(
      transport.calls.filter(
        (call) => call.command === 'list_commits' && call.args.query === 'missing',
      ),
    ).toHaveLength(1)
  })

  it('does not apply a reference jump after a new search starts', async () => {
    const { state, transport } = setup()
    await openReady(state, transport)
    const jumping = state.jump('target')
    const position = transport.take('commit_position')
    state.query.value = 'new search'
    position.resolve(0)
    expect(await jumping).toBeNull()
    await vi.advanceTimersByTimeAsync(160)
    transport.take('list_commits', { query: 'new search' }).resolve(page([], 0, index(1)))
    await flush()
    expect(state.filter.value).toBe('new search')
    expect(state.total.value).toBe(0)
    expect(state.selectedSha.value).toBe('')
  })
})

describe('reference navigation during indexing', () => {
  it('waits for the requested branch after leaving Solo without displaying a false missing-reference error', async () => {
    const { state, transport } = setup()
    const path = '/tmp/solo'
    await openReady(state, transport, {
      ...repository('solo'),
      path,
      soloRefs: ['refs/heads/topic'],
    })
    const probe = state.jump('main-tip', { reportMissing: false })
    transport.take('commit_position', { sessionId: 'solo' }).resolve(null)
    expect(await probe).toBeNull()
    expect(state.error.value).toBe('')
    expect(transport.calls.filter((call) => call.command === 'list_commits')).toHaveLength(1)

    const opening = state.open(path, [])
    transport.take('open_repository').resolve({ ...repository('full', 0, false), path })
    await flush()
    transport.take('list_commits', { sessionId: 'full' }).resolve(page([], 0, index(0, false)))
    await opening
    const jumping = state.jump('main-tip', { waitForIndex: true })
    transport.take('commit_position', { sessionId: 'full' }).resolve(null)
    await flush()
    await vi.advanceTimersByTimeAsync(1_000)
    expect(state.error.value).toBe('')
    expect(state.selectedSha.value).toBe('')
    expect(
      transport.calls.filter(
        (call) => call.command === 'commit_position' && call.args.sessionId === 'full',
      ),
    ).toHaveLength(1)

    transport.take('index_status', { sessionId: 'full' }).resolve(index(2))
    await flush()
    transport.take('commit_position', { sessionId: 'full' }).resolve(1)
    transport
      .take('list_commits', { sessionId: 'full' })
      .resolve(page([row(0, 'another-tip'), row(1, 'main-tip')]))
    expect(await jumping).toBe(1)
    expect(state.selectedSha.value).toBe('main-tip')
    expect(state.error.value).toBe('')
    expect(transport.calls.filter((call) => call.command === 'index_status')).toHaveLength(1)
    expect(
      transport.calls.filter(
        (call) => call.command === 'commit_details' && call.args.sha === 'another-tip',
      ),
    ).toHaveLength(0)
  })

  it('rechecks an old missing-position result when indexing completed before the progress watcher starts', async () => {
    const { state, transport } = setup()
    await openReady(state, transport, repository('repo', 0, false), page([], 0, index(0, false)))
    const jumping = state.jump('target', { waitForIndex: true })
    const oldPosition = transport.take('commit_position')
    transport.take('index_status').resolve(index(1))
    await flush()
    oldPosition.resolve(null)
    await flush()
    transport.take('commit_position').resolve(0)
    transport.take('list_commits').resolve(page([row(0, 'target')]))
    expect(await jumping).toBe(0)
    expect(state.selectedSha.value).toBe('target')
    expect(state.error.value).toBe('')
  })

  it.each(['search', 'close'])(
    'cancels an indexing wait when a new %s replaces navigation',
    async (change) => {
      const { state, transport } = setup()
      await openReady(state, transport, repository('repo', 0, false), page([], 0, index(0, false)))
      const jumping = state.jump('old-target', { waitForIndex: true })
      const position = transport.take('commit_position')
      position.resolve(null)
      await flush()
      if (change === 'search') state.query.value = 'new search'
      else state.close()
      expect(await jumping).toBeNull()
      expect(position.signal?.aborted).toBe(true)
      transport.take('index_status').resolve(index(1))
      await vi.advanceTimersByTimeAsync(160)
      if (change === 'search') {
        transport
          .take('list_commits', { query: 'new search' })
          .resolve(page([row(0, 'search-result')]))
      }
      await flush()
      expect(transport.calls.filter((call) => call.command === 'commit_position')).toHaveLength(1)
      expect(state.selectedSha.value).toBe(change === 'search' ? 'search-result' : '')
      expect(state.error.value).toBe('')
    },
  )

  it('still reports a missing target when its traversal was already fully indexed', async () => {
    const { state, transport } = setup()
    await openReady(state, transport)
    const jumping = state.jump('missing', { waitForIndex: true })
    transport.take('commit_position').resolve(null)
    await flush()
    transport.take('list_commits').resolve(page())
    expect(await jumping).toBeNull()
    expect(state.error.value).toBe('Cette référence n’est pas encore indexée.')
    await vi.advanceTimersByTimeAsync(31_000)
    expect(transport.calls.filter((call) => call.command === 'commit_position')).toHaveLength(1)
  })

  it('bounds an indexing wait without adding position polling when progress stalls', async () => {
    const { state, transport } = setup()
    await openReady(state, transport, repository('repo', 0, false), page([], 0, index(0, false)))
    const jumping = state.jump('target', { waitForIndex: true })
    transport.take('commit_position').resolve(null)
    await flush()
    await vi.advanceTimersByTimeAsync(30_000)
    transport.take('list_commits').resolve(page([], 0, index(0, false)))
    expect(await jumping).toBeNull()
    expect(state.error.value).toBe('Cette référence n’est pas encore indexée.')
    expect(transport.calls.filter((call) => call.command === 'commit_position')).toHaveLength(1)
  })

  it('reports native navigation failures even when the Solo probe suppresses absent targets', async () => {
    const { state, transport } = setup()
    await openReady(state, transport)
    const jumping = state.jump('target', { reportMissing: false })
    transport.take('commit_position').reject(new Error('Native session expired'))
    expect(await jumping).toBeNull()
    expect(state.error.value).toContain('Native session expired')
  })
})

describe('displayed selection transitions', () => {
  it('publishes commit and file colors with their patches, and preserves a pause requested during preparation', async () => {
    const workers = controlledWorkers()
    const { state, transport } = setup()
    await openReady(state, transport, repository('repo', 3), fullPage(0, 3))
    transport.take('commit_details').resolve(details('commit-0', [file(0)]))
    await flush()
    const firstPatch = textDiff('first')
    const firstColors = colored('first')
    transport.take('file_diff', { sha: 'commit-0' }).resolve(firstPatch)
    await flush()
    expect(state.details.value).toBeNull()
    expect(state.diff.value).toBeNull()
    expect(state.highlightedDiff.value).toBeNull()
    workers[0]!.complete(firstColors)
    await flush()
    expect(state.details.value?.sha).toBe('commit-0')
    expect(state.diff.value).toBe(firstPatch)
    expect(state.highlightedDiff.value).toBe(firstColors)

    const selecting = state.select(row(1), 1)
    transport
      .take('commit_details', { sha: 'commit-1' })
      .resolve(details('commit-1', [file(1), file(2)]))
    await flush()
    const nextPatch = textDiff('next')
    const nextColors = colored('next')
    transport.take('file_diff', { sha: 'commit-1', fileIndex: 1 }).resolve(nextPatch)
    await flush()
    expect(state.details.value?.sha).toBe('commit-0')
    expect(state.selectedFile.value?.index).toBe(0)
    expect(state.diff.value).toBe(firstPatch)
    expect(state.highlightedDiff.value).toBe(firstColors)
    state.pauseDiffPreload()
    workers[1]!.complete(nextColors)
    await selecting
    expect(state.details.value?.sha).toBe('commit-1')
    expect(state.selectedFile.value?.index).toBe(1)
    expect(state.diff.value).toBe(nextPatch)
    expect(state.highlightedDiff.value).toBe(nextColors)
    await vi.advanceTimersByTimeAsync(150)
    expect(transport.calls.filter((call) => call.command === 'file_diff')).toHaveLength(2)

    const selectingFile = state.selectFile(file(2))
    const filePatch = textDiff('another file')
    const fileColors = colored('another file')
    transport.take('file_diff', { sha: 'commit-1', fileIndex: 2 }).resolve(filePatch)
    await flush()
    expect(state.selectedFile.value?.index).toBe(1)
    expect(state.diff.value).toBe(nextPatch)
    expect(state.highlightedDiff.value).toBe(nextColors)
    workers.at(-1)!.complete(fileColors)
    await selectingFile
    expect(state.selectedFile.value?.index).toBe(2)
    expect(state.diff.value).toBe(filePatch)
    expect(state.highlightedDiff.value).toBe(fileColors)
  })

  it('ignores old colors after another commit replaces the snapshot and after the repository closes', async () => {
    const workers = controlledWorkers()
    const { state, transport } = setup()
    await openReady(state, transport)
    transport.take('commit_details').resolve(details('commit-0', [file(0)]))
    await flush()
    transport.take('file_diff', { sha: 'commit-0' }).resolve(textDiff('old'))
    await flush()
    const oldWorker = workers[0]!
    const latest = state.select(row(1), 1)
    transport.take('commit_details', { sha: 'commit-1' }).resolve(details('commit-1', [file(1)]))
    await flush()
    const latestPatch = textDiff('latest')
    const latestColors = colored('latest')
    transport.take('file_diff', { sha: 'commit-1' }).resolve(latestPatch)
    await flush()
    workers[1]!.complete(latestColors)
    await latest
    oldWorker.complete(colored('old'))
    await flush()
    expect(oldWorker.terminate).toHaveBeenCalled()
    expect(state.details.value?.sha).toBe('commit-1')
    expect(state.diff.value).toBe(latestPatch)
    expect(state.highlightedDiff.value).toBe(latestColors)

    state.close()
    workers[1]!.complete(colored('late after close'))
    await flush()
    expect(state.details.value).toBeNull()
    expect(state.selectedFile.value).toBeNull()
    expect(state.diff.value).toBeNull()
    expect(state.highlightedDiff.value).toBeNull()
  })

  it('keeps the old metadata, file and patch together until the next commit and its first patch are ready', async () => {
    const { state, transport } = setup()
    await openDisplayed(state, transport)
    const oldDetails = state.details.value
    const oldFile = state.selectedFile.value
    const oldDiff = state.diff.value
    const selecting = state.select(row(1), 1)

    expect(state.selectedSha.value).toBe('commit-1')
    expect(state.selectedIndex.value).toBe(1)
    expect(state.detailLoading.value).toBe(true)
    expect(state.details.value).toBe(oldDetails)
    expect(state.selectedFile.value).toBe(oldFile)
    expect(state.diff.value).toBe(oldDiff)
    await state.selectFile(file(1))
    expect(transport.calls.filter((call) => call.command === 'file_diff')).toHaveLength(1)

    transport.take('commit_details', { sha: 'commit-1' }).resolve(details('commit-1', [file(5)]))
    await flush()
    expect(state.details.value).toBe(oldDetails)
    expect(state.selectedFile.value).toBe(oldFile)
    expect(state.diff.value).toBe(oldDiff)
    expect(state.detailLoading.value).toBe(true)
    expect(state.diffLoading.value).toBe(true)
    await state.selectFile(file(1))
    expect(transport.calls.filter((call) => call.command === 'file_diff')).toHaveLength(2)
    transport.take('file_diff', { sha: 'commit-1', fileIndex: 5 }).resolve(diff(17))
    await selecting

    expect(state.details.value?.sha).toBe('commit-1')
    expect(state.selectedFile.value?.path).toBe('file-5.ts')
    expect(state.diff.value?.additions).toBe(17)
    expect(state.detailLoading.value).toBe(false)
    expect(state.diffLoading.value).toBe(false)
  })

  it('ignores an old first patch after a newer commit has replaced the displayed snapshot', async () => {
    const { state, transport } = setup()
    await openDisplayed(state, transport)
    const first = state.select(row(1), 1)
    transport.take('commit_details', { sha: 'commit-1' }).resolve(details('commit-1', [file(1)]))
    await flush()
    const oldPatch = transport.take('file_diff', { sha: 'commit-1', fileIndex: 1 })
    const latest = state.select(row(2), 2)
    transport.take('commit_details', { sha: 'commit-2' }).resolve(details('commit-2', [file(2)]))
    await flush()
    transport.take('file_diff', { sha: 'commit-2', fileIndex: 2 }).resolve(diff(22))
    await latest
    oldPatch.resolve(diff(999))
    await first

    expect(oldPatch.signal?.aborted).toBe(true)
    expect(state.selectedSha.value).toBe('commit-2')
    expect(state.details.value?.sha).toBe('commit-2')
    expect(state.selectedFile.value?.path).toBe('file-2.ts')
    expect(state.diff.value?.additions).toBe(22)
    expect(state.detailError.value).toBe('')
    expect(state.diffError.value).toBe('')
  })

  it('keeps the displayed file and its patch unchanged while another file is being read', async () => {
    const { state, transport } = setup()
    await openDisplayed(state, transport)
    const displayedDetails = state.details.value
    const oldFile = state.selectedFile.value
    const oldPatch = state.diff.value
    const selecting = state.selectFile(file(1))

    expect(state.selectedFile.value).toBe(oldFile)
    expect(state.diff.value).toBe(oldPatch)
    expect(state.diffLoading.value).toBe(true)
    transport.take('file_diff', { sha: 'commit-0', fileIndex: 1 }).resolve(diff(42))
    await selecting

    expect(state.details.value).toBe(displayedDetails)
    expect(state.selectedFile.value?.path).toBe('file-1.ts')
    expect(state.diff.value?.additions).toBe(42)
    expect(state.diffLoading.value).toBe(false)
  })

  it('cancels a pending file request when the user returns to the file already displayed', async () => {
    const { state, transport } = setup()
    await openDisplayed(state, transport)
    const shownPatch = state.diff.value
    const oldSelection = state.selectFile(file(1))
    const oldPatch = transport.take('file_diff', { sha: 'commit-0', fileIndex: 1 })

    await state.selectFile(file(0))
    expect(oldPatch.signal?.aborted).toBe(true)
    expect(state.diffLoading.value).toBe(false)
    expect(state.selectedFile.value?.path).toBe('file-0.ts')
    expect(state.diff.value).toBe(shownPatch)
    oldPatch.resolve(diff(999))
    await oldSelection

    expect(state.selectedFile.value?.path).toBe('file-0.ts')
    expect(state.diff.value).toBe(shownPatch)
    expect(transport.calls.filter((call) => call.command === 'file_diff')).toHaveLength(2)
  })

  it('clears the old patch together with file identity when the next commit has no changed files', async () => {
    const { state, transport } = setup()
    await openDisplayed(state, transport)
    const selecting = state.select(row(1), 1)
    transport.take('commit_details', { sha: 'commit-1' }).resolve(details('commit-1'))
    await selecting

    expect(state.details.value?.sha).toBe('commit-1')
    expect(state.selectedFile.value).toBeNull()
    expect(state.diff.value).toBeNull()
    expect(state.diffError.value).toBe('')
    expect(transport.calls.filter((call) => call.command === 'file_diff')).toHaveLength(1)
  })

  it('associates a failed metadata request with its intended commit and clears the stale snapshot', async () => {
    const { state, transport } = setup()
    await openDisplayed(state, transport)
    const selecting = state.select(row(1), 1)
    transport.take('commit_details', { sha: 'commit-1' }).reject(new Error('Object unavailable'))
    await selecting

    expect(state.selectedSha.value).toBe('commit-1')
    expect(state.detailError.value).toContain('commit-1')
    expect(state.detailError.value).toContain('Object unavailable')
    expect(state.details.value).toBeNull()
    expect(state.selectedFile.value).toBeNull()
    expect(state.diff.value).toBeNull()
    expect(state.detailLoading.value).toBe(false)
  })

  it('publishes new metadata and the first-file error together without reusing an old patch', async () => {
    const { state, transport } = setup()
    await openDisplayed(state, transport)
    const selecting = state.select(row(1), 1)
    transport.take('commit_details', { sha: 'commit-1' }).resolve(details('commit-1', [file(1)]))
    await flush()
    transport
      .take('file_diff', { sha: 'commit-1', fileIndex: 1 })
      .reject(new Error('Patch unavailable'))
    await selecting

    expect(state.details.value?.sha).toBe('commit-1')
    expect(state.selectedFile.value?.path).toBe('file-1.ts')
    expect(state.diff.value).toBeNull()
    expect(state.diffError.value).toContain('file-1.ts')
    expect(state.diffError.value).toContain('commit-1')
    expect(state.detailError.value).toBe('')
    expect(state.detailLoading.value).toBe(false)
    expect(state.diffLoading.value).toBe(false)
  })

  it('associates a file error with the requested file instead of leaving the previous patch beneath it', async () => {
    const { state, transport } = setup()
    await openDisplayed(state, transport)
    const selecting = state.selectFile(file(1))
    transport
      .take('file_diff', { sha: 'commit-0', fileIndex: 1 })
      .reject(new Error('Patch unavailable'))
    await selecting

    expect(state.details.value?.sha).toBe('commit-0')
    expect(state.selectedFile.value?.path).toBe('file-1.ts')
    expect(state.diff.value).toBeNull()
    expect(state.diffError.value).toContain('file-1.ts')
    expect(state.diffError.value).toContain('commit-0')
    expect(state.diffLoading.value).toBe(false)
  })

  it('ignores a pending file failure after a search has cleared the displayed snapshot', async () => {
    const { state, transport } = setup()
    await openDisplayed(state, transport)
    const selecting = state.selectFile(file(1))
    const oldPatch = transport.take('file_diff', { sha: 'commit-0', fileIndex: 1 })
    state.query.value = 'different'
    oldPatch.reject(new Error('Late file failure'))
    await selecting

    expect(oldPatch.signal?.aborted).toBe(true)
    expect(state.selectedSha.value).toBe('')
    expect(state.details.value).toBeNull()
    expect(state.selectedFile.value).toBeNull()
    expect(state.diff.value).toBeNull()
    expect(state.diffLoading.value).toBe(false)
    expect(state.diffError.value).toBe('')
  })

  it.each(['search', 'open', 'reference', 'dispose'] as const)(
    'clears the entire snapshot on %s and ignores the outstanding first patch',
    async (change) => {
      const { state, transport, scope } = setup()
      await openDisplayed(state, transport)
      const selecting = state.select(row(1), 1)
      transport.take('commit_details', { sha: 'commit-1' }).resolve(details('commit-1', [file(1)]))
      await flush()
      const oldPatch = transport.take('file_diff', { sha: 'commit-1', fileIndex: 1 })

      if (change === 'search') state.query.value = 'different'
      else if (change === 'open') {
        const opening = state.open('/tmp/invalid')
        transport.take('open_repository').reject(new Error('Not a repository'))
        await opening
      } else if (change === 'reference') void state.jump('different')
      else scope.stop()

      oldPatch.resolve(diff(999))
      await selecting
      expect(oldPatch.signal?.aborted).toBe(true)
      expect(state.details.value).toBeNull()
      expect(state.selectedFile.value).toBeNull()
      expect(state.diff.value).toBeNull()
      expect(state.detailLoading.value).toBe(false)
      expect(state.diffLoading.value).toBe(false)
      expect(state.detailError.value).toBe('')
      expect(state.diffError.value).toBe('')
    },
  )
})

describe('progressive loading and search', () => {
  it('keeps the final search count when an older second-page snapshot arrives after completion', async () => {
    const { state, transport } = setup()
    await openReady(
      state,
      transport,
      repository('repo', 256, false),
      fullPage(0, 256, index(256, false)),
    )
    const status = transport.take('index_status')
    state.query.value = 'match'
    await vi.advanceTimersByTimeAsync(160)
    transport
      .take('list_commits', { query: 'match', offset: 0 })
      .resolve(fullPage(0, 256, index(256, false)))
    await flush()

    status.resolve(index(384, false))
    await flush()
    const refreshingFirst = transport.take('list_commits', { query: 'match', offset: 0 })
    const loadingSecond = state.loadPage(128)
    const oldSecond = transport.take('list_commits', { query: 'match', offset: 128 })
    refreshingFirst.resolve(fullPage(0, 512))
    await flush()
    oldSecond.resolve(fullPage(128, 384, index(384, false)))
    await loadingSecond
    await vi.advanceTimersByTimeAsync(1_000)

    expect(state.total.value).toBe(512)
    expect(state.progress.value.complete).toBe(true)
    expect(state.getRow(128)?.sha).toBe('commit-128')
    expect(transport.calls.filter((call) => call.command === 'index_status')).toHaveLength(1)
  })

  it('reloads an old partial page even when another page has already announced completion', async () => {
    const { state, transport } = setup()
    await openReady(
      state,
      transport,
      repository('repo', 300, false),
      fullPage(0, 300, index(300, false)),
    )
    const status = transport.take('index_status')
    const loadingTail = state.loadPage(256)
    transport.take('list_commits', { offset: 256 }).resolve(fullPage(256, 300, index(300, false)))
    await loadingTail

    status.resolve(index(350, false))
    await flush()
    const refreshingTail = transport.take('list_commits', { offset: 256 })
    const loadingMiddle = state.loadPage(128)
    transport.take('list_commits', { offset: 128 }).resolve(fullPage(128, 500))
    await loadingMiddle
    refreshingTail.resolve(fullPage(256, 350, index(350, false)))
    await flush()
    transport.take('list_commits', { offset: 256 }).resolve(fullPage(256, 500))
    await flush()
    await vi.advanceTimersByTimeAsync(1_000)

    expect(state.total.value).toBe(500)
    expect(state.progress.value.complete).toBe(true)
    expect(state.getRow(350)?.sha).toBe('commit-350')
    expect(transport.calls.filter((call) => call.command === 'index_status')).toHaveLength(1)
  })

  it('debounces searches, cancels obsolete pages immediately, and clears selection for no results', async () => {
    const { state, transport } = setup()
    await openReady(state, transport)
    transport.take('commit_details').resolve(details('commit-0', [file(0)]))
    await flush()
    transport.take('file_diff').resolve(diff(3))
    await flush()

    state.query.value = 'old'
    await vi.advanceTimersByTimeAsync(160)
    const oldSearch = transport.take('list_commits', { query: 'old' })
    state.query.value = 'n'
    await vi.advanceTimersByTimeAsync(80)
    state.query.value = 'new'
    oldSearch.resolve(page([row(5, 'old-result')], 1))
    await vi.advanceTimersByTimeAsync(159)
    expect(state.getRow(0)).toBeUndefined()
    expect(transport.calls.filter((call) => call.command === 'list_commits')).toHaveLength(2)
    await vi.advanceTimersByTimeAsync(1)
    transport.take('list_commits', { query: 'new' }).resolve(page([], 0, index(1)))
    await flush()

    expect(oldSearch.signal?.aborted).toBe(true)
    expect(state.filter.value).toBe('new')
    expect(state.total.value).toBe(0)
    expect(state.selectedSha.value).toBe('')
    expect(state.details.value).toBeNull()
    expect(state.selectedFile.value).toBeNull()
    expect(state.diff.value).toBeNull()
    expect(state.loadingPages.value).toBe(0)
  })

  it('bounds retained pages while allowing evicted ranges to be loaded again', async () => {
    const { state, transport } = setup()
    const count = PAGE_SIZE * 20
    const fullPage = (offset: number) =>
      page(
        Array.from({ length: PAGE_SIZE }, (_, position) => row(offset + position)),
        count,
        index(count),
        offset,
      )
    await openReady(state, transport, repository('repo', count), fullPage(0))
    for (let offset = PAGE_SIZE; offset < PAGE_SIZE * 14; offset += PAGE_SIZE) {
      const loading = state.loadPage(offset)
      transport.take('list_commits', { offset }).resolve(fullPage(offset))
      await loading
    }
    expect(state.pages.value.size).toBe(12)
    expect(state.getRow(0)).toBeUndefined()
    expect(state.getRow(PAGE_SIZE * 13)?.position).toBe(PAGE_SIZE * 13)

    const reloading = state.loadPage(0)
    const duplicate = state.loadPage(0)
    expect(
      transport.calls.filter((call) => call.command === 'list_commits' && !call.settled),
    ).toHaveLength(1)
    transport.take('list_commits', { offset: 0 }).resolve(fullPage(0))
    await Promise.all([reloading, duplicate])
    expect(state.getRow(0)?.position).toBe(0)
    expect(state.pages.value.size).toBe(12)

    state.query.value = 'different view'
    expect(state.pages.value.size).toBe(0)
    expect(state.getRow(PAGE_SIZE * 13)).toBeUndefined()
  })

  it('refreshes an initially empty page as indexing progresses and stops polling on completion', async () => {
    const { state, transport } = setup()
    await openReady(state, transport, repository('repo', 0, false), page([], 0, index(0, false)))
    transport.take('index_status').resolve(index(2, false))
    await flush()
    transport
      .take('list_commits', { offset: 0 })
      .resolve(page([row(0), row(1)], 2, index(2, false)))
    await flush()
    expect(state.getRow(1)?.position).toBe(1)
    expect(state.selectedSha.value).toBe('commit-0')

    await vi.advanceTimersByTimeAsync(350)
    transport.take('index_status').resolve(index(3))
    await flush()
    transport
      .take('list_commits', { offset: 0 })
      .resolve(page([row(0), row(1), row(2)], 3, index(3)))
    await flush()
    expect(state.total.value).toBe(3)
    expect(state.progress.value.complete).toBe(true)
    expect(state.getRow(2)?.position).toBe(2)
    await vi.advanceTimersByTimeAsync(2_000)
    expect(transport.calls.filter((call) => call.command === 'index_status')).toHaveLength(2)
  })

  it('refreshes filtered results discovered during indexing, including when final status arrives before a page', async () => {
    const { state, transport } = setup()
    await openReady(
      state,
      transport,
      repository('repo', 1, false),
      page([row(0)], 1, index(1, false)),
    )
    const status = transport.take('index_status')
    state.query.value = 'match'
    await vi.advanceTimersByTimeAsync(160)
    const search = transport.take('list_commits', { query: 'match' })
    status.resolve(index(4))
    await flush()
    search.resolve(page([], 0, index(1, false)))
    await flush()
    transport
      .take('list_commits', { query: 'match' })
      .resolve(page([row(3, 'matching-commit')], 1, index(4)))
    await flush()

    expect(state.total.value).toBe(1)
    expect(state.getRow(0)?.sha).toBe('matching-commit')
    expect(state.progress.value.indexed).toBe(4)
    expect(state.progress.value.complete).toBe(true)
    expect(state.selectedSha.value).toBe('matching-commit')
  })

  it('cancels timers and ignores outstanding responses after the owning scope is disposed', async () => {
    const { state, transport, scope } = setup()
    await openReady(state, transport, repository('repo', 0, false), page([], 0, index(0, false)))
    const status = transport.take('index_status')
    state.query.value = 'not requested'
    scope.stop()
    status.resolve(index(100))
    await vi.advanceTimersByTimeAsync(2_000)
    expect(status.signal?.aborted).toBe(true)
    expect(state.progress.value.indexed).toBe(0)
    expect(transport.calls.filter((call) => call.command === 'list_commits')).toHaveLength(1)
    expect(transport.calls.filter((call) => call.command === 'index_status')).toHaveLength(1)
  })
})

describe('keyboard selection', () => {
  it('loads a missing page and preserves repeated arrow intents until the latest row can be selected', async () => {
    const { state, transport } = setup()
    await openReady(state, transport, repository('repo', 256), fullPage(0, 256))
    void state.select(row(127), 127)

    const first = state.moveSelection(1)
    const second = state.moveSelection(1)
    const latest = state.moveSelection(1)
    expect(
      transport.calls.filter((call) => call.command === 'list_commits' && !call.settled),
    ).toHaveLength(1)
    transport.take('list_commits', { offset: 128 }).resolve(fullPage(128, 256))

    expect(await first).toBeNull()
    expect(await second).toBeNull()
    expect(await latest).toBe(130)
    expect(state.selectedIndex.value).toBe(130)
    expect(state.selectedSha.value).toBe('commit-130')
    transport.take('commit_details', { sha: 'commit-130' }).resolve(details('commit-130'))
    await flush()
    expect(state.details.value?.sha).toBe('commit-130')
  })

  it.each(['search', 'open', 'reference', 'click'] as const)(
    'ignores a delayed keyboard selection after a %s changes the selection context',
    async (change) => {
      const { state, transport } = setup()
      await openReady(state, transport, repository('repo', 256), fullPage(0, 256))
      void state.select(row(127), 127)
      const moving = state.moveSelection(1)
      const oldPage = transport.take('list_commits', { offset: 128 })

      if (change === 'search') {
        state.query.value = 'missing'
        await vi.advanceTimersByTimeAsync(160)
        transport.take('list_commits', { query: 'missing' }).resolve(page([], 0, index(256)))
        await flush()
      } else if (change === 'open') {
        await openReady(state, transport, repository('other'), page([row(0, 'other-commit')]))
      } else if (change === 'reference') {
        const jumping = state.jump('commit-2')
        transport.take('commit_position', { sha: 'commit-2' }).resolve(2)
        await flush()
        transport.take('list_commits', { offset: 0 }).resolve(fullPage(0, 256))
        expect(await jumping).toBe(2)
      } else {
        void state.select(row(1), 1)
      }

      oldPage.resolve(fullPage(128, 256))
      expect(await moving).toBeNull()
      expect(state.selectedSha.value).toBe(
        { search: '', open: 'other-commit', reference: 'commit-2', click: 'commit-1' }[change],
      )
      expect(
        transport.calls.filter(
          (call) => call.command === 'commit_details' && call.args.sha === 'commit-128',
        ),
      ).toHaveLength(0)
    },
  )
})

describe('visible page retention', () => {
  it('keeps both visible pages present when returning across an evicted page boundary', async () => {
    const { state, transport } = setup()
    const total = PAGE_SIZE * 13
    await openReady(state, transport, repository('repo', total), fullPage(0, total))
    for (let offset = PAGE_SIZE; offset < total; offset += PAGE_SIZE) {
      const reading = state.loadPage(offset)
      transport.take('list_commits', { offset }).resolve(fullPage(offset, total))
      await reading
    }
    // Page 1 is consulted first, while page 0 finishes its slower native read.
    state.loadRange(115, 140)
    expect(
      transport.calls.filter((call) => call.command === 'list_commits' && !call.settled),
    ).toHaveLength(1)
    transport.take('list_commits', { offset: 0 }).resolve(fullPage(0, total))
    await flush()
    expect(state.getRow(120)?.sha).toBe('commit-120')
    expect(state.getRow(130)?.sha).toBe('commit-130')
    expect(state.pages.value.size).toBeLessThanOrEqual(12)
  })

  it.each([120, null])(
    'restores the same visible range after a reference jump to %s',
    async (position) => {
      const { state, transport } = setup()
      await openReady(state, transport, repository('repo', 256), fullPage(0, 256))
      state.loadRange(115, 140)
      transport.take('list_commits', { offset: 128 }).resolve(fullPage(128, 256))
      await flush()
      const jumping = state.jump('target')
      transport.take('commit_position').resolve(position)
      await flush()
      transport.take('list_commits', { offset: 0 }).resolve(fullPage(0, 256))
      transport.take('list_commits', { offset: 128 }).resolve(fullPage(128, 256))
      expect(await jumping).toBe(position)
      expect(state.getRow(130)?.sha).toBe('commit-130')
    },
  )

  it('keeps commit background preparation paused when a search auto-selects a result', async () => {
    const workers = controlledWorkers()
    const { state, transport } = setup()
    await openReady(state, transport)
    state.pauseDiffPreload()
    state.query.value = 'result'
    await vi.advanceTimersByTimeAsync(160)
    transport.take('list_commits', { query: 'result' }).resolve(page([row(0, 'result')]))
    await flush()
    transport
      .take('commit_details', { sha: 'result' })
      .resolve(details('result', [file(0), file(1)]))
    await flush()
    transport.take('file_diff', { fileIndex: 0 }).resolve(textDiff('const value = 1'))
    await flush()
    workers.at(-1)!.complete(colored('const value = 1'))
    await vi.advanceTimersByTimeAsync(1_000)
    expect(transport.calls.filter((call) => call.command === 'file_diff')).toHaveLength(1)
    state.resumeDiffPreload()
    await vi.advanceTimersByTimeAsync(150)
    expect(transport.calls.filter((call) => call.command === 'file_diff')).toHaveLength(2)
  })
})

describe('repository refresh', () => {
  it('opens a Solo traversal with its preserved search before loading or selecting commits', async () => {
    const { state, transport } = setup()
    const info = { ...repository(), soloRefs: ['refs/heads/topic'] }
    const opening = state.open(info.path, info.soloRefs, 'active search')
    transport.take('open_repository').resolve(info)
    expect(await opening).toBe(true)
    expect(state.query.value).toBe('active search')
    expect(transport.calls.filter((call) => call.command === 'list_commits')).toHaveLength(0)
    expect(transport.calls.filter((call) => call.command === 'commit_details')).toHaveLength(0)

    await vi.advanceTimersByTimeAsync(160)
    const pages = transport.calls.filter((call) => call.command === 'list_commits')
    expect(pages).toHaveLength(1)
    expect(pages[0]!.args.query).toBe('active search')
    pages[0]!.resolve(page([row(0, 'matching-commit')]))
    await flush()
    expect(state.selectedSha.value).toBe('matching-commit')
    expect(transport.take('commit_details').args.sha).toBe('matching-commit')
  })

  it('retains the Solo filter, displayed commit and active search for an unchanged session', async () => {
    const { state, transport } = setup()
    const info = { ...repository(), soloRefs: ['refs/heads/topic'] }
    await openReady(state, transport, info)
    transport.take('commit_details').resolve(details('commit-0', []))
    await flush()
    state.query.value = 'active search'
    await flush()
    await vi.advanceTimersByTimeAsync(160)
    transport.take('list_commits', { query: 'active search' }).resolve(page())
    await flush()
    transport.take('commit_details').resolve(details('commit-0', []))
    await flush()
    const before = state.details.value
    const refreshing = state.refresh()
    const request = transport.take('open_repository')
    expect(request.args).toEqual({ path: info.path, soloRefs: info.soloRefs })
    request.resolve(info)
    await flush()
    for (const call of transport.calls.filter(
      (call) => !call.settled && call.command === 'list_commits',
    ))
      call.resolve(page())
    expect(await refreshing).toBe(true)
    expect(state.details.value).toBe(before)
    expect(state.selectedSha.value).toBe('commit-0')
    expect(state.query.value).toBe('active search')
  })

  it('loads a new traversal when the native session changes and releases old selection', async () => {
    const { state, transport } = setup()
    await openDisplayed(state, transport)
    const refreshing = state.refresh()
    const updated = {
      ...repository('new-generation'),
      path: '/tmp/repo',
      soloRefs: ['refs/heads/topic'],
    }
    transport.take('open_repository').resolve(updated)
    await flush()
    transport
      .take('list_commits', { sessionId: updated.id, offset: 0 })
      .resolve(page([row(0, 'new-tip')]))
    expect(await refreshing).toBe(true)
    expect(state.repository.value?.id).toBe(updated.id)
    expect(state.selectedSha.value).toBe('new-tip')
    expect(state.diff.value).toBeNull()
  })
})
