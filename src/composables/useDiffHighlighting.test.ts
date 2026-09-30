import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test'
import { effectScope, nextTick, reactive, shallowRef } from 'vue'
import type { HighlightedDiff, SyntaxRequest, SyntaxResponse } from '../lib/diffSyntax'
import type { ChangedFile, FileDiff } from '../lib/types'
import { useDiffHighlighting, type DiffHighlightWorker } from './useDiffHighlighting'

class FakeWorker implements DiffHighlightWorker {
  onmessage: DiffHighlightWorker['onmessage'] = null
  onerror: DiffHighlightWorker['onerror'] = null
  onmessageerror: DiffHighlightWorker['onmessageerror'] = null
  requests: SyntaxRequest[] = []
  postMessage = vi.fn((request: SyntaxRequest) => {
    // Exercise the same clone constraint as postMessage without a browser or mounted DOM.
    this.requests.push(structuredClone(request))
  })
  // Retain callbacks after termination to test that late native messages are harmless.
  terminate = vi.fn()

  respond(tokens: HighlightedDiff | null, id = this.requests.at(-1)!.id) {
    this.onmessage?.call(
      this as unknown as Worker,
      {
        data: { id, tokens },
      } as MessageEvent<SyntaxResponse>,
    )
  }
  fail(kind: 'error' | 'messageerror') {
    if (kind === 'error') this.onerror?.call(this as unknown as Worker, {} as ErrorEvent)
    else this.onmessageerror?.call(this as unknown as Worker, {} as MessageEvent)
  }
}

const file = (path = 'src/main.ts'): ChangedFile => ({
  index: 0,
  path,
  oldPath: null,
  status: 'modified',
})
const patch = (content = 'const value = 1\n'): FileDiff => ({
  binary: false,
  truncated: false,
  additions: 1,
  deletions: 0,
  hunks: [
    {
      header: '@@ -0,0 +1 @@\n',
      lines: [{ kind: 'addition', oldLine: null, newLine: 1, content }],
    },
  ],
})
const colored = (content = 'const value = 1'): HighlightedDiff => [[[{ content, kind: 'keyword' }]]]

const cleanup: (() => void)[] = []
function setup(initialFile: ChangedFile | null = null, initialDiff: FileDiff | null = null) {
  const selectedFile = shallowRef(initialFile)
  const diff = shallowRef(initialDiff)
  const workers: FakeWorker[] = []
  const factory = vi.fn(() => {
    const worker = new FakeWorker()
    workers.push(worker)
    return worker
  })
  const scope = effectScope()
  const tokens = scope.run(() =>
    useDiffHighlighting(
      () => selectedFile.value,
      () => diff.value,
      factory,
    ),
  )!
  cleanup.push(() => scope.stop())
  return { selectedFile, diff, workers, factory, scope, tokens }
}

beforeEach(() => vi.useFakeTimers())
afterEach(() => {
  for (const dispose of cleanup.splice(0)) dispose()
  vi.useRealTimers()
})

describe('diff highlighting worker lifecycle', () => {
  it('starts lazily for a supported text diff and leaves raw text visible until the response', async () => {
    const state = setup()
    expect(state.factory).not.toHaveBeenCalled()
    state.selectedFile.value = file()
    await nextTick()
    expect(state.factory).not.toHaveBeenCalled()
    state.diff.value = patch()
    await nextTick()

    expect(state.factory).toHaveBeenCalledOnce()
    expect(state.tokens.value).toBeNull()
    expect(state.workers[0]!.requests[0]).toEqual({
      id: 1,
      path: 'src/main.ts',
      hunks: [{ header: '', lines: patch().hunks[0]!.lines }],
    })
    const result = colored()
    state.workers[0]!.respond(result)
    expect(state.tokens.value).toBe(result)
    vi.advanceTimersByTime(2_000)
    expect(state.workers[0]!.terminate).not.toHaveBeenCalled()
  })

  it('batches a coherent file and diff change instead of processing new filenames with old patches', async () => {
    const state = setup(file(), patch('old\n'))
    const first = state.workers[0]!
    first.respond(colored('old'))
    state.selectedFile.value = file('second.py')
    state.diff.value = patch('new\n')
    await nextTick()

    expect(state.factory).toHaveBeenCalledOnce()
    expect(first.requests).toHaveLength(2)
    expect(first.requests[1]!.path).toBe('second.py')
    expect(first.requests[1]!.hunks[0]!.lines[0]!.content).toBe('new\n')
    expect(state.tokens.value).toBeNull()
  })

  it('terminates an active worker on replacement and ignores its late response and errors', async () => {
    const state = setup(file(), patch('old\n'))
    const old = state.workers[0]!
    state.diff.value = patch('new\n')
    await nextTick()
    const current = state.workers[1]!

    expect(old.terminate).toHaveBeenCalledOnce()
    expect(old.requests).toHaveLength(1)
    expect(current.requests).toHaveLength(1)
    old.respond(colored('old'))
    old.fail('error')
    expect(state.tokens.value).toBeNull()
    expect(current.terminate).not.toHaveBeenCalled()
    const result = colored('new')
    current.respond(result)
    old.respond(colored('old'))
    expect(state.tokens.value).toBe(result)
  })

  it('ignores a stale request id from a reused idle worker and waits for the current request', async () => {
    const state = setup(file(), patch('old\n'))
    const worker = state.workers[0]!
    const oldId = worker.requests[0]!.id
    worker.respond(colored('old'))
    state.diff.value = patch('new\n')
    await nextTick()

    worker.respond(colored('old'), oldId)
    expect(state.tokens.value).toBeNull()
    const result = colored('new')
    worker.respond(result)
    expect(state.tokens.value).toBe(result)
    expect(state.factory).toHaveBeenCalledOnce()
    expect(worker.terminate).not.toHaveBeenCalled()
  })

  it('cancels pending work when the next file is unsupported without queuing or applying old tokens', async () => {
    const state = setup(file(), patch())
    const old = state.workers[0]!
    state.selectedFile.value = file('binary.unknown-language')
    await nextTick()

    expect(old.terminate).toHaveBeenCalledOnce()
    expect(state.factory).toHaveBeenCalledOnce()
    old.respond(colored('obsolete'))
    expect(state.tokens.value).toBeNull()
    expect(vi.getTimerCount()).toBe(0)
    state.selectedFile.value = file('next.ts')
    await nextTick()
    expect(state.factory).toHaveBeenCalledTimes(2)
    expect(state.workers[1]!.requests[0]!.path).toBe('next.ts')
  })

  it.each(['error', 'messageerror'] as const)(
    'falls back silently after %s and creates a clean worker for the next request',
    async (kind) => {
      const state = setup(file(), patch())
      const old = state.workers[0]!
      old.fail(kind)
      expect(state.tokens.value).toBeNull()
      expect(old.terminate).toHaveBeenCalledOnce()
      state.diff.value = patch('replacement\n')
      await nextTick()
      const current = state.workers[1]!
      old.respond(colored('obsolete'))
      expect(state.tokens.value).toBeNull()
      current.respond(colored('replacement'))
      expect(state.tokens.value?.[0]?.[0]?.[0]?.content).toBe('replacement')
    },
  )

  it('stops a timed-out worker at two seconds and never accepts its eventual result', async () => {
    const state = setup(file(), patch())
    const old = state.workers[0]!
    vi.advanceTimersByTime(1_999)
    expect(old.terminate).not.toHaveBeenCalled()
    vi.advanceTimersByTime(1)
    expect(old.terminate).toHaveBeenCalledOnce()
    expect(state.tokens.value).toBeNull()
    state.diff.value = patch('current\n')
    await nextTick()
    old.respond(colored('obsolete'))
    const result = colored('current')
    state.workers[1]!.respond(result)
    vi.advanceTimersByTime(2_000)
    expect(state.tokens.value).toBe(result)
    expect(state.workers[1]!.terminate).not.toHaveBeenCalled()
  })

  it('falls back when worker creation or message cloning fails', async () => {
    const state = setup()
    state.factory.mockImplementationOnce(() => {
      throw new Error('Worker unavailable')
    })
    state.selectedFile.value = file()
    state.diff.value = patch()
    await nextTick()
    expect(state.tokens.value).toBeNull()
    expect(state.workers).toHaveLength(0)
    state.factory.mockImplementationOnce(() => {
      const worker = new FakeWorker()
      worker.postMessage.mockImplementation(() => {
        throw new Error('Message unavailable')
      })
      state.workers.push(worker)
      return worker
    })
    state.diff.value = patch('another\n')
    await nextTick()
    expect(state.tokens.value).toBeNull()
    expect(state.workers[0]!.terminate).toHaveBeenCalledOnce()
    expect(vi.getTimerCount()).toBe(0)
  })

  it('accepts the tokenizer raw-text fallback without retaining a timeout', () => {
    const state = setup(file(), patch())
    state.workers[0]!.respond(null)
    expect(state.tokens.value).toBeNull()
    expect(vi.getTimerCount()).toBe(0)
  })

  it.each([false, true])(
    'releases an idle or active worker when its diff closes (idle=%s)',
    async (idle) => {
      const state = setup(file(), patch())
      const worker = state.workers[0]!
      if (idle) worker.respond(colored())
      state.diff.value = null
      await nextTick()
      expect(state.tokens.value).toBeNull()
      expect(worker.terminate).toHaveBeenCalledOnce()
      worker.respond(colored('late'))
      expect(state.tokens.value).toBeNull()
      expect(vi.getTimerCount()).toBe(0)
    },
  )

  it('terminates work and clears tokens on disposal even if the worker ignores terminate', async () => {
    const state = setup(file(), patch())
    const worker = state.workers[0]!
    state.scope.stop()
    expect(worker.terminate).toHaveBeenCalledOnce()
    expect(vi.getTimerCount()).toBe(0)
    worker.respond(colored('late'))
    state.diff.value = patch('after disposal\n')
    await nextTick()
    expect(state.tokens.value).toBeNull()
    expect(state.factory).toHaveBeenCalledOnce()
  })

  it('projects reactive diff data into cloneable primitives before sending', () => {
    const source = reactive(patch('let answer = 42\r\n'))
    const state = setup(reactive(file()), source)
    const request = state.workers[0]!.requests[0]!
    expect(request.hunks).toEqual([{ header: '', lines: source.hunks[0]!.lines }])
    expect(request.hunks).not.toBe(source.hunks)
    state.workers[0]!.respond(colored('let answer = 42'))
    expect(state.tokens.value).not.toBeNull()
  })

  it.each([
    ['unsupported', file('asset.unknown-language'), patch()],
    ['binary', file(), { ...patch(), binary: true }],
    ['empty', file(), { ...patch(), hunks: [] }],
    ['long line', file(), patch('x'.repeat(20 * 1024 + 1))],
    [
      'many lines',
      file(),
      {
        ...patch(),
        hunks: [
          { header: '', lines: Array.from({ length: 2001 }, () => patch().hunks[0]!.lines[0]!) },
        ],
      },
    ],
    [
      'many characters',
      file(),
      {
        ...patch(),
        hunks: [
          {
            header: '',
            lines: Array.from(
              { length: 30 },
              () => patch('x'.repeat(10 * 1024)).hunks[0]!.lines[0]!,
            ),
          },
        ],
      },
    ],
  ] satisfies [string, ChangedFile, FileDiff][])(
    'does not create a worker for %s diffs',
    (_reason, selectedFile, diff) => {
      const state = setup(selectedFile, diff)
      expect(state.factory).not.toHaveBeenCalled()
      expect(state.tokens.value).toBeNull()
      expect(vi.getTimerCount()).toBe(0)
    },
  )
})
