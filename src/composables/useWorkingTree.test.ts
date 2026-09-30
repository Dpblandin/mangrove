import { Buffer } from 'node:buffer'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test'
import { effectScope, shallowRef } from 'vue'
import type { GitClient } from '../lib/api'
import type { HighlightedDiff, SyntaxRequest, SyntaxResponse } from '../lib/diffSyntax'
import type { FileDiff, RepositoryInfo, WorkingTreeFile, WorkingTreeStatus } from '../lib/types'
import type { DiffHighlightWorker } from './useDiffHighlighting'
import { useWorkingTree } from './useWorkingTree'

function delayedClient() {
  const calls: {
    command: string
    args: Record<string, unknown>
    signal?: AbortSignal
    settled: boolean
    resolve(value: unknown): void
    reject(reason: unknown): void
  }[] = []
  const client: GitClient = <T>(
    command: string,
    args: Record<string, unknown>,
    signal?: AbortSignal,
  ) =>
    new Promise<T>((resolve, reject) => {
      // Like a dispatched Tauri command, this fake intentionally ignores abort.
      const call = {
        command,
        args,
        signal,
        settled: false,
        resolve(value: unknown) {
          call.settled = true
          resolve(value as T)
        },
        reject(reason: unknown) {
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
    if (!call) throw new Error(`Missing ${command}: ${JSON.stringify(args)}`)
    return call
  }
  return { client, calls, take }
}

const repo = (id = 'repo'): RepositoryInfo => ({
  id,
  name: id,
  path: `/tmp/${id}`,
  head: null,
  headName: 'main',
  references: [],
  remotes: [],
  index: { indexed: 0, complete: true, error: null },
})
const file = (
  path = 'src/main.ts',
  index = 0,
  key = Buffer.from(path).toString('hex'),
): WorkingTreeFile => ({
  key,
  index,
  path,
  status: 'modified',
  oldPath: null,
})
const status = (
  staged: WorkingTreeFile[] = [],
  unstaged: WorkingTreeFile[] = [],
  available = true,
): WorkingTreeStatus => ({
  available,
  staged,
  unstaged,
  stagedCount: staged.length,
  unstagedCount: unstaged.length,
})
const patch = (additions: number): FileDiff => ({
  additions,
  deletions: 0,
  binary: false,
  truncated: false,
  hunks: [],
})
const textPatch = (content: string): FileDiff => ({
  ...patch(1),
  hunks: [
    {
      header: '@@ -0,0 +1 @@',
      lines: [{ kind: 'addition', oldLine: null, newLine: 1, content }],
    },
  ],
})
const colored = (content: string): HighlightedDiff => [[[{ content, kind: 'string' }]]]

function controlledWorkers() {
  class ControlledWorker implements DiffHighlightWorker {
    onmessage: DiffHighlightWorker['onmessage'] = null
    onerror: DiffHighlightWorker['onerror'] = null
    onmessageerror: DiffHighlightWorker['onmessageerror'] = null
    requests: SyntaxRequest[] = []
    postMessage = vi.fn((request: SyntaxRequest) => this.requests.push(structuredClone(request)))
    terminate = vi.fn()

    respond(tokens: HighlightedDiff | null) {
      // Queued host responses can arrive even after terminate().
      this.onmessage?.call(
        this as unknown as Worker,
        {
          data: { id: this.requests.at(-1)!.id, tokens },
        } as MessageEvent<SyntaxResponse>,
      )
    }
  }
  const workers: ControlledWorker[] = []
  const factory = vi.fn(() => {
    const worker = new ControlledWorker()
    workers.push(worker)
    return worker
  })
  return { factory, workers }
}

async function flush() {
  for (let step = 0; step < 12; step++) await Promise.resolve()
}
const cleanup: (() => void)[] = []
function setup(
  initial: RepositoryInfo | null = repo(),
  options: Parameters<typeof useWorkingTree>[2] = {},
) {
  const repository = shallowRef(initial)
  const transport = delayedClient()
  const scope = effectScope()
  const state = scope.run(() => useWorkingTree(repository, transport.client, options))!
  cleanup.push(() => scope.stop())
  return { state, repository, transport, scope }
}
async function ready(
  state: ReturnType<typeof useWorkingTree>,
  transport: ReturnType<typeof delayedClient>,
  value = status([], [file()]),
) {
  const refreshing = state.refresh()
  transport.take('working_tree_status').resolve(value)
  await refreshing
}
async function display(
  state: ReturnType<typeof useWorkingTree>,
  transport: ReturnType<typeof delayedClient>,
  target = file(),
) {
  const selecting = state.selectFile('unstaged', target)
  transport.take('working_tree_diff', { area: 'unstaged', fileKey: target.key }).resolve(patch(3))
  await selecting
}
beforeEach(() => vi.useFakeTimers())
afterEach(() => {
  for (const dispose of cleanup.splice(0)) dispose()
  vi.useRealTimers()
})

describe('working tree reads', () => {
  it('loads status on opening a repository, deduplicates concurrent refreshes and performs no automatic patch read', async () => {
    const { state, repository, transport } = setup(null)
    expect(transport.calls).toHaveLength(0)
    repository.value = repo()
    const first = state.refresh()
    const second = state.refresh()
    expect(first).toBe(second)
    expect(state.loading.value).toBe(true)
    expect(transport.calls).toHaveLength(1)
    const value = status([], [file()])
    transport.take('working_tree_status').resolve(value)
    await Promise.all([first, second])
    await flush()

    expect(state.status.value).toBe(value)
    expect(state.selectedFile.value).toBeNull()
    expect(state.selectedArea.value).toBeNull()
    expect(state.loading.value).toBe(false)
    expect(transport.calls).toHaveLength(1)
  })

  it('ignores an older repository status even when its command completes after the latest one', async () => {
    const { state, repository, transport } = setup(repo('first'))
    const oldStatus = transport.take('working_tree_status', { sessionId: 'first' })
    repository.value = repo('second')
    const value = status([], [file('second.ts')])
    const refreshing = state.refresh()
    transport.take('working_tree_status', { sessionId: 'second' }).resolve(value)
    await refreshing
    oldStatus.resolve(status([], [file('first.ts')]))
    await flush()

    expect(oldStatus.signal?.aborted).toBe(true)
    expect(state.status.value).toBe(value)
    expect(state.error.value).toBe('')
    expect(state.loading.value).toBe(false)
  })

  it('refreshes on session changes without rereading for metadata updates within the same session', async () => {
    const { state, repository, transport } = setup()
    await ready(state, transport)
    await display(state, transport)
    const shownPatch = state.diff.value
    repository.value = { ...repo(), headName: 'develop' }
    await flush()

    expect(transport.calls).toHaveLength(2)
    expect(state.diff.value).toBe(shownPatch)
    expect(state.loading.value).toBe(false)
    repository.value = null
    expect(state.status.value).toBeNull()
    expect(state.selectedFile.value).toBeNull()
    expect(state.selectedArea.value).toBeNull()
    expect(state.diff.value).toBeNull()
    expect(state.error.value).toBe('')
  })

  it('retains the local file selection while a fetch renews the same repository session', async () => {
    const { state, repository, transport } = setup()
    await ready(state, transport)
    await display(state, transport)
    const shownFile = state.selectedFile.value
    const shownPatch = state.diff.value
    repository.value = { ...repo('updated'), path: '/tmp/repo' }
    expect(state.selectedFile.value).toBe(shownFile)
    expect(state.diff.value).toBe(shownPatch)
    transport.take('working_tree_status', { sessionId: 'updated' }).resolve(status([], [file()]))
    await flush()
    transport.take('working_tree_diff', { sessionId: 'updated' }).resolve(patch(9))
    await flush()
    expect(state.selectedFile.value?.key).toBe(shownFile?.key)
    expect(state.selectedArea.value).toBe('unstaged')
    expect(state.diff.value?.additions).toBe(9)
  })

  it('keeps area, file and patch together while reading the other area for the same path', async () => {
    const { state, transport } = setup()
    const staged = { ...file(), status: 'added' as const }
    const unstaged = file(staged.path, 4)
    await ready(state, transport, status([staged], [unstaged]))
    const first = state.selectFile('staged', staged)
    transport.take('working_tree_diff', { area: 'staged', fileKey: staged.key }).resolve(patch(3))
    await first
    const shownPatch = state.diff.value
    const selecting = state.selectFile('unstaged', unstaged)

    expect(state.selectedFile.value).toBe(staged)
    expect(state.selectedArea.value).toBe('staged')
    expect(state.diff.value).toBe(shownPatch)
    expect(state.diffLoading.value).toBe(true)
    transport
      .take('working_tree_diff', { area: 'unstaged', fileKey: unstaged.key })
      .resolve(patch(5))
    await selecting
    expect(state.selectedFile.value).toBe(unstaged)
    expect(state.selectedArea.value).toBe('unstaged')
    expect(state.diff.value?.additions).toBe(5)
    expect(state.loading.value).toBe(false)
  })

  it('reads and preserves the correct opaque key when different filenames have the same display path', async () => {
    const { state, transport } = setup()
    const path = 'collision-�.txt'
    const a = file(path, 0, '636f6c6c6973696f6e2dff2e747874')
    const b = file(path, 1, '636f6c6c6973696f6e2dfe2e747874')
    await ready(state, transport, status([], [a, b]))
    await display(state, transport, a)
    const selecting = state.selectFile('unstaged', b)
    const request = transport.take('working_tree_diff', { fileKey: b.key })

    expect(request.args).toEqual({ sessionId: 'repo', area: 'unstaged', fileKey: b.key })
    expect(state.selectedFile.value).toBe(a)
    request.resolve(patch(7))
    await selecting
    expect(state.selectedFile.value).toBe(b)
    expect(state.diff.value?.additions).toBe(7)

    const refreshedB = { ...b, index: 8, status: 'added' as const }
    const refreshing = state.refresh()
    transport.take('working_tree_status').resolve(status([], [a, refreshedB]))
    await refreshing
    expect(state.selectedFile.value).toBe(b)
    transport.take('working_tree_diff', { fileKey: b.key }).resolve(patch(8))
    await flush()

    expect(state.selectedFile.value).toBe(refreshedB)
    expect(state.diff.value?.additions).toBe(8)
  })

  it('clears a vanished key even when another file keeps the same display path', async () => {
    const { state, transport } = setup()
    const a = file('collision-�.txt', 0, '636f6c6c6973696f6e2dff2e747874')
    const b = file(a.path, 1, '636f6c6c6973696f6e2dfe2e747874')
    await ready(state, transport, status([], [a, b]))
    await display(state, transport, a)
    const selecting = state.selectFile('unstaged', b)
    const oldPatch = transport.take('working_tree_diff', { fileKey: b.key })
    const refreshing = state.refresh()
    transport.take('working_tree_status').resolve(status([], [a]))
    await refreshing
    oldPatch.resolve(patch(999))
    await selecting

    expect(oldPatch.signal?.aborted).toBe(true)
    expect(state.selectedFile.value).toBeNull()
    expect(state.selectedArea.value).toBeNull()
    expect(state.diff.value).toBeNull()
    expect(state.loading.value).toBe(false)
    expect(transport.calls.filter((call) => call.command === 'working_tree_diff')).toHaveLength(2)
  })

  it('keeps the latest file selection when requests complete out of order', async () => {
    const { state, transport } = setup()
    const a = file('a.ts')
    const b = file('b.ts', 1)
    await ready(state, transport, status([], [a, b]))
    await display(state, transport, a)
    const oldSelection = state.selectFile('unstaged', b)
    const oldPatch = transport.take('working_tree_diff', { fileKey: b.key })
    const latest = state.selectFile('unstaged', a)
    await latest
    oldPatch.resolve(patch(999))
    await oldSelection

    expect(oldPatch.signal?.aborted).toBe(true)
    expect(state.selectedFile.value?.path).toBe(a.path)
    expect(state.diff.value?.additions).toBe(3)
    expect(transport.calls.filter((call) => call.command === 'working_tree_diff')).toHaveLength(2)
    expect(state.error.value).toBe('')
  })

  it('preserves selection by area and key across refresh while atomically replacing refreshed metadata and patch', async () => {
    const { state, transport } = setup()
    const a = file('a.ts')
    await ready(state, transport, status([], [a]))
    await display(state, transport, a)
    const shownPatch = state.diff.value
    const refreshedFile = { ...file(a.path, 8), oldPath: 'old.ts', status: 'renamed' as const }
    const refreshing = state.refresh()
    transport.take('working_tree_status').resolve(status([], [refreshedFile]))
    await refreshing

    expect(state.selectedFile.value).toBe(a)
    expect(state.diff.value).toBe(shownPatch)
    expect(state.refreshing.value).toBe(false)
    expect(state.diffLoading.value).toBe(true)
    transport.take('working_tree_diff', { area: 'unstaged', fileKey: a.key }).resolve(patch(11))
    await flush()
    expect(state.selectedFile.value).toBe(refreshedFile)
    expect(state.selectedArea.value).toBe('unstaged')
    expect(state.diff.value?.additions).toBe(11)
  })

  it('refreshes the latest selection intent when a file is clicked during a status refresh', async () => {
    const { state, transport } = setup()
    const a = file('a.ts')
    const b = file('b.ts', 1)
    await ready(state, transport, status([], [a, b]))
    await display(state, transport, a)
    const refreshing = state.refresh()
    const selecting = state.selectFile('unstaged', b)
    const oldPatch = transport.take('working_tree_diff', { fileKey: b.key })
    const refreshedB = file(b.path, 9)
    transport.take('working_tree_status').resolve(status([], [a, refreshedB]))
    await refreshing
    oldPatch.resolve(patch(999))
    await selecting
    expect(state.selectedFile.value).toBe(a)
    transport.take('working_tree_diff', { fileKey: b.key }).resolve(patch(30))
    await flush()

    expect(oldPatch.signal?.aborted).toBe(true)
    expect(state.selectedFile.value).toBe(refreshedB)
    expect(state.diff.value?.additions).toBe(30)
  })

  it('clears a vanished selected path and invalidates its pending patch without selecting a different file', async () => {
    const { state, transport } = setup()
    const a = file('a.ts')
    const b = file('b.ts', 1)
    await ready(state, transport, status([], [a, b]))
    await display(state, transport, a)
    const selecting = state.selectFile('unstaged', b)
    const oldPatch = transport.take('working_tree_diff', { fileKey: b.key })
    const refreshing = state.refresh()
    transport.take('working_tree_status').resolve(status([], [a]))
    await refreshing
    oldPatch.resolve(patch(999))
    await selecting

    expect(oldPatch.signal?.aborted).toBe(true)
    expect(state.selectedFile.value).toBeNull()
    expect(state.selectedArea.value).toBeNull()
    expect(state.diff.value).toBeNull()
    expect(state.loading.value).toBe(false)
    expect(state.error.value).toBe('')
  })

  it.each([true, false])(
    'clears selection when status becomes empty (available=%s)',
    async (available) => {
      const { state, transport } = setup()
      await ready(state, transport)
      await display(state, transport)
      const refreshing = state.refresh()
      transport.take('working_tree_status').resolve(status([], [], available))
      await refreshing
      await state.selectFile('unstaged', file())

      expect(state.status.value?.available).toBe(available)
      expect(state.selectedFile.value).toBeNull()
      expect(state.diff.value).toBeNull()
      expect(state.loading.value).toBe(false)
      expect(transport.calls.filter((call) => call.command === 'working_tree_diff')).toHaveLength(1)
    },
  )

  it('retains the last coherent snapshot on status failure and recovers on a subsequent refresh', async () => {
    const { state, transport } = setup()
    await ready(state, transport)
    await display(state, transport)
    const shownStatus = state.status.value
    const shownFile = state.selectedFile.value
    const shownPatch = state.diff.value
    const refreshing = state.refresh()
    transport.take('working_tree_status').reject(new Error('Status unavailable'))
    await refreshing

    expect(state.status.value).toBe(shownStatus)
    expect(state.selectedFile.value).toBe(shownFile)
    expect(state.diff.value).toBe(shownPatch)
    expect(state.error.value).toContain('Status unavailable')
    expect(state.loading.value).toBe(false)
    const recovering = state.refresh()
    transport.take('working_tree_status').resolve(status([], [file()]))
    await recovering
    transport.take('working_tree_diff').resolve(patch(8))
    await flush()
    expect(state.error.value).toBe('')
    expect(state.diff.value?.additions).toBe(8)
  })

  it('publishes a file error under its requested area and path without reusing the previous patch', async () => {
    const { state, transport } = setup()
    const a = file('a.ts')
    const b = file('b.ts', 1)
    await ready(state, transport, status([], [a, b]))
    await display(state, transport, a)
    const selecting = state.selectFile('unstaged', b)
    transport.take('working_tree_diff', { fileKey: b.key }).reject(new Error('Patch unavailable'))
    await selecting

    expect(state.selectedFile.value).toBe(b)
    expect(state.selectedArea.value).toBe('unstaged')
    expect(state.diff.value).toBeNull()
    expect(state.error.value).toContain(b.path)
    expect(state.error.value).toContain('répertoire de travail')
    expect(state.diffLoading.value).toBe(false)
  })

  it('clears old repository selection and ignores its late file failure after another repository opens', async () => {
    const { state, repository, transport } = setup(repo('first'))
    await ready(state, transport)
    const selecting = state.selectFile('unstaged', file())
    const oldPatch = transport.take('working_tree_diff', { sessionId: 'first' })
    repository.value = repo('second')
    await ready(state, transport, status([], [file('second.ts')]))
    oldPatch.reject(new Error('Late failure'))
    await selecting

    expect(oldPatch.signal?.aborted).toBe(true)
    expect(state.selectedFile.value).toBeNull()
    expect(state.diff.value).toBeNull()
    expect(state.error.value).toBe('')
    expect(state.loading.value).toBe(false)
  })

  it('ignores files outside the latest status even when an unknown key has the same display path', async () => {
    const { state, transport } = setup()
    await ready(state, transport)
    await state.selectFile('staged', file())
    await state.selectFile('unstaged', file('missing.ts'))
    await state.selectFile('unstaged', { ...file(), key: 'ff' })
    expect(transport.calls.filter((call) => call.command === 'working_tree_diff')).toHaveLength(0)
    expect(state.loading.value).toBe(false)
  })

  it('aborts outstanding status and diff reads on disposal and ignores all later results', async () => {
    const { state, transport, scope } = setup()
    await ready(state, transport)
    const refreshing = state.refresh()
    const oldStatus = transport.take('working_tree_status')
    const selecting = state.selectFile('unstaged', file())
    const oldPatch = transport.take('working_tree_diff')
    scope.stop()
    oldStatus.resolve(status([], [file('late.ts')]))
    oldPatch.resolve(patch(999))
    await Promise.all([refreshing, selecting])
    await state.refresh()

    expect(oldStatus.signal?.aborted).toBe(true)
    expect(oldPatch.signal?.aborted).toBe(true)
    expect(state.status.value).toBeNull()
    expect(state.selectedFile.value).toBeNull()
    expect(state.diff.value).toBeNull()
    expect(state.loading.value).toBe(false)
    expect(state.error.value).toBe('')
    expect(transport.calls).toHaveLength(3)
  })
})

describe('working tree syntax preparation', () => {
  it('waits for activation and remains paused when deactivated during a status read', async () => {
    const active = shallowRef(false)
    const { factory, workers } = controlledWorkers()
    const { state, transport } = setup(repo(), {
      active: () => active.value,
      workerFactory: factory,
    })
    await ready(state, transport)
    await vi.advanceTimersByTimeAsync(1_000)
    expect(transport.calls.filter((call) => call.command === 'working_tree_diff')).toHaveLength(0)
    expect(factory).not.toHaveBeenCalled()

    active.value = true
    const pendingStatus = state.refresh()
    active.value = false
    transport.take('working_tree_status').resolve(status([], [file()]))
    await pendingStatus
    await vi.advanceTimersByTimeAsync(1_000)
    expect(transport.calls.filter((call) => call.command === 'working_tree_diff')).toHaveLength(0)
    expect(factory).not.toHaveBeenCalled()

    active.value = true
    await ready(state, transport)
    await vi.advanceTimersByTimeAsync(150)
    transport.take('working_tree_diff').resolve(textPatch('active again'))
    await flush()
    workers[0]!.respond(colored('active again'))
    await flush()
    await state.selectFile('unstaged', file())
    expect(state.highlightedDiff.value).toEqual(colored('active again'))
    expect(transport.calls.filter((call) => call.command === 'working_tree_diff')).toHaveLength(1)
  })

  it('allows a foreground click after reactivation during a pending status and replaces it with the refreshed snapshot', async () => {
    const active = shallowRef(false)
    const { factory, workers } = controlledWorkers()
    const { state, transport } = setup(repo(), {
      active: () => active.value,
      workerFactory: factory,
    })
    const original = file()
    await ready(state, transport, status([], [original]))
    active.value = true
    const refreshing = state.refresh()
    active.value = false
    active.value = true
    const selecting = state.selectFile('unstaged', original)
    const oldRead = transport.take('working_tree_diff')
    oldRead.resolve(textPatch('before new status'))
    await flush()
    const oldWorker = workers[0]!
    const refreshed = { ...original, index: 6, status: 'added' as const }
    transport.take('working_tree_status').resolve(status([], [refreshed]))
    await refreshing
    const latestPatch = textPatch('after new status')
    const latestColors = colored('after new status')
    transport.take('working_tree_diff').resolve(latestPatch)
    await flush()
    workers[1]!.respond(latestColors)
    await flush()
    oldWorker.respond(colored('before new status'))
    await selecting

    expect(oldRead.signal?.aborted).toBe(true)
    expect(state.selectedFile.value).toBe(refreshed)
    expect(state.selectedArea.value).toBe('unstaged')
    expect(state.diff.value).toBe(latestPatch)
    expect(state.highlightedDiff.value).toBe(latestColors)
    expect(state.error.value).toBe('')
    expect(transport.calls.filter((call) => call.command === 'working_tree_status')).toHaveLength(2)
  })

  it('prepares staged and unstaged separately for one opaque key and reuses both without another read or parse', async () => {
    const { factory, workers } = controlledWorkers()
    const { state, transport } = setup(repo(), { workerFactory: factory })
    const staged = { ...file(), status: 'added' as const }
    const unstaged = file(staged.path, 4)
    await ready(state, transport, status([staged], [unstaged]))
    await vi.advanceTimersByTimeAsync(150)
    const unstagedPatch = textPatch('working version')
    const unstagedColors = colored('working version')
    transport.take('working_tree_diff', { area: 'unstaged' }).resolve(unstagedPatch)
    await flush()
    workers[0]!.respond(unstagedColors)
    await flush()
    await vi.advanceTimersByTimeAsync(150)
    const stagedPatch = textPatch('index version')
    const stagedColors = colored('index version')
    transport.take('working_tree_diff', { area: 'staged' }).resolve(stagedPatch)
    await flush()
    workers[0]!.respond(stagedColors)
    await flush()

    expect(state.selectedFile.value).toBeNull()
    expect(state.diff.value).toBeNull()
    expect(state.highlightedDiff.value).toBeNull()
    await state.selectFile('staged', staged)
    expect(state.selectedFile.value).toBe(staged)
    expect(state.selectedArea.value).toBe('staged')
    expect(state.diff.value).toBe(stagedPatch)
    expect(state.highlightedDiff.value).toBe(stagedColors)
    await state.selectFile('unstaged', unstaged)
    expect(state.selectedFile.value).toBe(unstaged)
    expect(state.selectedArea.value).toBe('unstaged')
    expect(state.diff.value).toBe(unstagedPatch)
    expect(state.highlightedDiff.value).toBe(unstagedColors)
    expect(transport.calls.filter((call) => call.command === 'working_tree_diff')).toHaveLength(2)
    expect(factory).toHaveBeenCalledOnce()
    expect(workers[0]!.requests).toHaveLength(2)
  })

  it('publishes area, file, patch and colors together only after the foreground parse finishes', async () => {
    const { factory, workers } = controlledWorkers()
    const { state, transport } = setup(repo(), { workerFactory: factory })
    const staged = file()
    const unstaged = { ...staged, index: 5 }
    await ready(state, transport, status([staged], [unstaged]))
    const first = state.selectFile('staged', staged)
    const firstPatch = textPatch('index snapshot')
    const firstColors = colored('index snapshot')
    transport.take('working_tree_diff', { area: 'staged' }).resolve(firstPatch)
    await flush()
    expect(state.selectedFile.value).toBeNull()
    expect(state.diff.value).toBeNull()
    expect(state.highlightedDiff.value).toBeNull()
    expect(state.diffLoading.value).toBe(true)
    workers[0]!.respond(firstColors)
    await first

    const selecting = state.selectFile('unstaged', unstaged)
    const nextPatch = textPatch('working snapshot')
    const nextColors = colored('working snapshot')
    transport.take('working_tree_diff', { area: 'unstaged' }).resolve(nextPatch)
    await flush()
    expect(state.selectedFile.value).toBe(staged)
    expect(state.selectedArea.value).toBe('staged')
    expect(state.diff.value).toBe(firstPatch)
    expect(state.highlightedDiff.value).toBe(firstColors)
    expect(state.diffLoading.value).toBe(true)
    workers[0]!.respond(nextColors)
    await selecting
    expect(state.selectedFile.value).toBe(unstaged)
    expect(state.selectedArea.value).toBe('unstaged')
    expect(state.diff.value).toBe(nextPatch)
    expect(state.highlightedDiff.value).toBe(nextColors)
    expect(state.diffLoading.value).toBe(false)
  })

  it.each(['reading', 'highlighting'] as const)(
    'invalidates an old %s job on refresh even when its opaque key remains present',
    async (phase) => {
      const { factory, workers } = controlledWorkers()
      const { state, transport } = setup(repo(), { workerFactory: factory })
      const a = file('a.ts')
      const b = file('b.ts', 1)
      await ready(state, transport, status([], [a, b]))
      const initial = state.selectFile('unstaged', a)
      const displayedPatch = textPatch('displayed')
      const displayedColors = colored('displayed')
      transport.take('working_tree_diff', { fileKey: a.key }).resolve(displayedPatch)
      await flush()
      workers[0]!.respond(displayedColors)
      await initial
      const oldSelection = state.selectFile('unstaged', b)
      const oldRead = transport.take('working_tree_diff', { fileKey: b.key })
      if (phase === 'highlighting') {
        oldRead.resolve(textPatch('outdated'))
        await flush()
      }
      const oldWorker = workers[0]!
      const refreshing = state.refresh()
      const refreshedB = { ...b, index: 8, status: 'added' as const }
      transport.take('working_tree_status').resolve(status([], [a, refreshedB]))
      await refreshing
      const latestPatch = textPatch('refreshed')
      const latestColors = colored('refreshed')
      const latestRead = transport.calls
        .filter((call) => call.command === 'working_tree_diff' && call.args.fileKey === b.key)
        .at(-1)!
      expect(latestRead).not.toBe(oldRead)
      latestRead.resolve(latestPatch)
      await flush()
      expect(state.selectedFile.value).toBe(a)
      expect(state.diff.value).toBe(displayedPatch)
      expect(state.highlightedDiff.value).toBe(displayedColors)
      expect(state.diffLoading.value).toBe(true)
      workers[1]!.respond(latestColors)
      await flush()
      if (phase === 'reading') oldRead.resolve(textPatch('outdated'))
      else oldWorker.respond(colored('outdated'))
      await oldSelection
      await flush()
      expect(oldRead.signal?.aborted).toBe(true)
      expect(oldWorker.terminate).toHaveBeenCalled()
      expect(state.selectedFile.value).toBe(refreshedB)
      expect(state.selectedArea.value).toBe('unstaged')
      expect(state.diff.value).toBe(latestPatch)
      expect(state.highlightedDiff.value).toBe(latestColors)
      expect(state.error.value).toBe('')
    },
  )

  it.each(['session', 'dispose'] as const)(
    'clears colors and ignores a queued parser response after %s changes',
    async (change) => {
      const { factory, workers } = controlledWorkers()
      const { state, repository, transport, scope } = setup(repo('first'), {
        workerFactory: factory,
      })
      await ready(state, transport)
      const selecting = state.selectFile('unstaged', file())
      transport.take('working_tree_diff').resolve(textPatch('old repository'))
      await flush()
      const oldWorker = workers[0]!
      if (change === 'session') repository.value = repo('second')
      else scope.stop()
      oldWorker.respond(colored('old repository'))
      await selecting
      await flush()
      expect(oldWorker.terminate).toHaveBeenCalled()
      expect(state.selectedFile.value).toBeNull()
      expect(state.selectedArea.value).toBeNull()
      expect(state.diff.value).toBeNull()
      expect(state.highlightedDiff.value).toBeNull()
      expect(state.error.value).toBe('')
    },
  )
})
