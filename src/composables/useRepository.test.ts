import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test'
import { effectScope } from 'vue'
import type { GitClient } from '../lib/api'
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

beforeEach(() => {
  vi.useFakeTimers()
  cleanup = []
})

afterEach(() => {
  for (const dispose of cleanup) dispose()
  vi.useRealTimers()
})

describe('repository navigation', () => {
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
    secondDetails.resolve(details('commit-1', [file(0), file(1)]))
    await second
    const initialDiff = transport.take('file_diff', { fileIndex: 0 })
    const selectingFile = state.selectFile(file(1))
    transport.take('file_diff', { fileIndex: 1 }).resolve(diff(42))
    await selectingFile
    initialDetails.reject(new Error('Old details failed'))
    initialDiff.resolve(diff(999))
    await flush()

    expect(initialDetails.signal?.aborted).toBe(true)
    expect(initialDiff.signal?.aborted).toBe(true)
    expect(state.selectedSha.value).toBe('commit-1')
    expect(state.selectedIndex.value).toBe(1)
    expect(state.details.value?.sha).toBe('commit-1')
    expect(state.selectedFile.value?.path).toBe('file-1.ts')
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
