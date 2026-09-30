import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test'
import type { DiffHighlightWorker } from '../composables/useDiffHighlighting'
import type { GitClient } from './api'
import { createCommitDiffCache } from './diffCache'
import type { HighlightedDiff, SyntaxRequest, SyntaxResponse } from './diffSyntax'
import type { ChangedFile, FileDiff } from './types'

class FakeWorker implements DiffHighlightWorker {
  onmessage: DiffHighlightWorker['onmessage'] = null
  onerror: DiffHighlightWorker['onerror'] = null
  onmessageerror: DiffHighlightWorker['onmessageerror'] = null
  requests: SyntaxRequest[] = []
  postMessage = vi.fn((request: SyntaxRequest) => {
    this.requests.push(structuredClone(request))
  })
  terminate = vi.fn()
  respond(tokens: HighlightedDiff | null = colored(), id = this.requests.at(-1)!.id) {
    this.onmessage?.call(
      this as unknown as Worker,
      { data: { id, tokens } } as MessageEvent<SyntaxResponse>,
    )
  }
}

const file = (index: number, path = `file-${index}.ts`): ChangedFile => ({
  index,
  path,
  oldPath: null,
  status: 'modified',
})
const patch = (content = 'const value = 1'): FileDiff => ({
  binary: false,
  truncated: false,
  additions: 1,
  deletions: 0,
  hunks: [
    { header: '@@ header @@', lines: [{ kind: 'addition', content, oldLine: null, newLine: 1 }] },
  ],
})
const colored = (content = 'const value = 1'): HighlightedDiff => [[[{ content, kind: 'keyword' }]]]

const cleanup: (() => void)[] = []
function setup(files = [file(0), file(1), file(2)], withWorker = true) {
  const calls: {
    args: Record<string, unknown>
    signal?: AbortSignal
    resolve(value: FileDiff): void
    reject(reason: unknown): void
  }[] = []
  // Native invoke can ignore cancellation; keeping this promise pending exercises that contract.
  const client: GitClient = <T>(
    _command: string,
    args: Record<string, unknown>,
    signal?: AbortSignal,
  ) =>
    new Promise<T>((resolve, reject) => {
      calls.push({ args, signal, resolve: (value) => resolve(value as T), reject })
    })
  const workers: FakeWorker[] = []
  const factory = vi.fn(() => {
    const worker = new FakeWorker()
    workers.push(worker)
    return worker
  })
  const cache = createCommitDiffCache(client, withWorker ? factory : undefined)
  cache.reset({ sessionId: 'session', sha: 'commit', files })
  cleanup.push(cache.dispose)
  return { cache, calls, workers, factory, files }
}
async function flush() {
  for (let index = 0; index < 8; index++) await Promise.resolve()
}
async function firstPrepared(state: ReturnType<typeof setup>) {
  const reading = state.cache.read(state.files[0]!)
  state.calls[0]!.resolve(patch())
  await flush()
  state.workers[0]!.respond()
  return reading
}

beforeEach(() => {
  vi.useFakeTimers()
  vi.stubGlobal('Worker', undefined)
})
afterEach(() => {
  for (const dispose of cleanup.splice(0)) dispose()
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

describe('current commit preparation', () => {
  it('releases an idle worker on pause while retaining already prepared diffs', async () => {
    const state = setup()
    const prepared = await firstPrepared(state)
    state.cache.pause()
    expect(state.workers[0]!.terminate).toHaveBeenCalledOnce()
    expect(await state.cache.read(state.files[0]!)).toBe(prepared)
    expect(state.calls).toHaveLength(1)
    expect(state.factory).toHaveBeenCalledOnce()
  })

  it('allows a foreground parse to finish after pausing, then releases its worker', async () => {
    const state = setup()
    const reading = state.cache.read(state.files[0]!)
    state.calls[0]!.resolve(patch())
    await flush()
    state.cache.pause()
    expect(state.workers[0]!.terminate).not.toHaveBeenCalled()
    state.workers[0]!.respond()
    expect((await reading).tokens).toEqual(colored())
    expect(state.workers[0]!.terminate).toHaveBeenCalledOnce()
    await vi.advanceTimersByTimeAsync(1_000)
    expect(state.calls).toHaveLength(1)
  })

  it('reads plain diffs without Worker and never starts a background scan', async () => {
    const state = setup(undefined, false)
    const reading = state.cache.read(file(0))
    const diff = patch()
    state.calls[0]!.resolve(diff)
    expect(await reading).toEqual({ diff, tokens: null })
    expect(await state.cache.read(file(0))).toEqual({ diff, tokens: null })
    state.cache.preload()
    await vi.advanceTimersByTimeAsync(1_000)
    expect(state.calls).toHaveLength(1)
    expect(state.calls[0]!.args).toEqual({ sessionId: 'session', sha: 'commit', fileIndex: 0 })
  })

  it('publishes only the prepared snapshot and hits the cache without another read or parse', async () => {
    const state = setup()
    let published = false
    const reading = state.cache.read(file(0)).then((value) => {
      published = true
      return value
    })
    state.calls[0]!.resolve(patch())
    await flush()
    expect(published).toBe(false)
    expect(state.workers[0]!.requests[0]!.hunks[0]!.header).toBe('')
    state.workers[0]!.respond()
    const prepared = await reading
    expect(prepared.tokens).toEqual(colored())
    expect(await state.cache.read(file(0))).toBe(prepared)
    expect(state.calls).toHaveLength(1)
    expect(state.workers[0]!.requests).toHaveLength(1)
  })

  it('deduplicates subscribers without letting one abort cancel another', async () => {
    const state = setup()
    const controller = new AbortController()
    const first = state.cache.read(file(0), controller.signal)
    const aborted = expect(first).rejects.toMatchObject({ name: 'AbortError' })
    const second = state.cache.read(file(0))
    controller.abort()
    await aborted
    expect(state.calls).toHaveLength(1)
    expect(state.calls[0]!.signal?.aborted).toBe(false)
    state.calls[0]!.resolve(patch())
    await flush()
    state.workers[0]!.respond()
    expect((await second).tokens).toEqual(colored())
  })

  it('rejects a foreign identity and an already aborted subscriber before reading Git', async () => {
    const state = setup()
    await expect(state.cache.read(file(0, 'different.ts'))).rejects.toThrow('n’appartient pas')
    await expect(state.cache.read(file(0), AbortSignal.abort())).rejects.toMatchObject({
      name: 'AbortError',
    })
    expect(state.calls).toHaveLength(0)
  })

  it('rejects old reads immediately on reset and ignores late native results and old signals', async () => {
    const state = setup()
    const oldSignal = new AbortController()
    const oldRead = state.cache.read(file(0), oldSignal.signal)
    const oldRejected = expect(oldRead).rejects.toMatchObject({ name: 'AbortError' })
    state.cache.reset({ sessionId: 'new-session', sha: 'new-commit', files: [file(0)] })
    await oldRejected
    expect(state.calls[0]!.signal?.aborted).toBe(true)
    const current = state.cache.read(file(0))
    oldSignal.abort()
    state.calls[0]!.resolve(patch('old'))
    await flush()
    expect(state.workers).toHaveLength(0)
    expect(state.calls[1]!.signal?.aborted).toBe(false)
    state.calls[1]!.resolve(patch('new'))
    await flush()
    state.workers[0]!.respond(colored('new'))
    expect((await current).diff.hunks[0]!.lines[0]!.content).toBe('new')
  })
})

describe('sequential background preparation', () => {
  it('paces reads and interrupts a background parser for foreground work without rereading its diff', async () => {
    const state = setup()
    await firstPrepared(state)
    state.cache.preload()
    await vi.advanceTimersByTimeAsync(149)
    expect(state.calls).toHaveLength(1)
    await vi.advanceTimersByTimeAsync(1)
    expect(state.calls[1]!.args.fileIndex).toBe(1)
    state.calls[1]!.resolve(patch('background'))
    await flush()
    const oldWorker = state.workers[0]!
    const oldId = oldWorker.requests.at(-1)!.id
    const foreground = state.cache.read(file(2))
    expect(oldWorker.terminate).toHaveBeenCalledOnce()
    state.calls[2]!.resolve(patch('foreground'))
    await flush()
    const currentWorker = state.workers[1]!
    expect(currentWorker.requests[0]!.path).toBe('file-2.ts')
    oldWorker.respond(colored('obsolete'), oldId)
    currentWorker.respond(colored('foreground'))
    expect((await foreground).tokens).toEqual(colored('foreground'))
    expect(currentWorker.requests[1]!.path).toBe('file-1.ts')
    currentWorker.respond(colored('background'))
    await flush()
    expect((await state.cache.read(file(1))).tokens).toEqual(colored('background'))
    expect(state.calls).toHaveLength(3)
  })

  it('promotes an in-flight background Git read instead of launching the same read twice', async () => {
    const state = setup()
    await firstPrepared(state)
    state.cache.preload()
    await vi.advanceTimersByTimeAsync(150)
    const foreground = state.cache.read(file(1))
    expect(state.calls).toHaveLength(2)
    state.calls[1]!.resolve(patch('promoted'))
    await flush()
    state.workers[0]!.respond(colored('promoted'))
    expect((await foreground).tokens).toEqual(colored('promoted'))
  })

  it('holds the actual native background slot across reset even when abort is ignored', async () => {
    const state = setup()
    state.cache.preload()
    await vi.advanceTimersByTimeAsync(150)
    expect(state.calls).toHaveLength(1)
    state.cache.reset({ sessionId: 'new-session', sha: 'new-commit', files: [file(1)] })
    state.cache.preload()
    await vi.advanceTimersByTimeAsync(600)
    expect(state.calls).toHaveLength(1)
    expect(state.calls[0]!.signal?.aborted).toBe(true)
    state.calls[0]!.resolve(patch('obsolete'))
    await flush()
    expect(state.workers).toHaveLength(0)
    await vi.advanceTimersByTimeAsync(150)
    expect(state.calls).toHaveLength(2)
    expect(state.calls[1]!.args).toEqual({
      sessionId: 'new-session',
      sha: 'new-commit',
      fileIndex: 1,
    })
  })

  it('pauses parsing and scheduling, keeps prepared data and resumes without another Git read', async () => {
    const state = setup()
    const first = await firstPrepared(state)
    state.cache.preload()
    await vi.advanceTimersByTimeAsync(150)
    state.calls[1]!.resolve(patch('background'))
    await flush()
    state.cache.pause()
    expect(state.workers[0]!.terminate).toHaveBeenCalledOnce()
    await vi.advanceTimersByTimeAsync(1_000)
    expect(state.calls).toHaveLength(2)
    expect(await state.cache.read(file(0))).toBe(first)
    state.cache.preload()
    await vi.advanceTimersByTimeAsync(150)
    expect(state.workers[1]!.requests[0]!.path).toBe('file-1.ts')
    state.workers[1]!.respond(colored('background'))
    await flush()
    expect((await state.cache.read(file(1))).tokens).toEqual(colored('background'))
    expect(state.calls).toHaveLength(2)
  })

  it('skips unknown formats and bounds the work before a worker is created', async () => {
    const state = setup([file(0, 'data.txt'), file(1), file(2)])
    state.cache.preload()
    await vi.advanceTimersByTimeAsync(150)
    expect(state.calls[0]!.args.fileIndex).toBe(1)
    state.calls[0]!.resolve(patch('x'.repeat(20 * 1024 + 1)))
    await flush()
    expect(state.factory).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(150)
    state.calls[1]!.resolve({ ...patch(), binary: true })
    await flush()
    expect(state.factory).not.toHaveBeenCalled()
  })
})

describe('bounded LRU and fallbacks', () => {
  it('stops background work at the entry budget while foreground reads can evict the LRU', async () => {
    const files = Array.from({ length: 257 }, (_, index) => file(index))
    const state = setup(files)
    for (const current of files.slice(0, 256)) {
      const reading = state.cache.read(current)
      state.calls.at(-1)!.resolve({ ...patch(), binary: true })
      await reading
    }
    state.cache.preload()
    await vi.advanceTimersByTimeAsync(1_000)
    expect(state.calls).toHaveLength(256)
    const foreground = state.cache.read(files[256]!)
    state.calls[256]!.resolve({ ...patch(), binary: true })
    await foreground
    const oldest = state.cache.read(files[0]!)
    expect(state.calls).toHaveLength(258)
    state.calls[257]!.resolve({ ...patch(), binary: true })
    await oldest
  })

  it('stops background admission instead of churning the existing text cache', async () => {
    const files = Array.from({ length: 30 }, (_, index) => file(index))
    const diff = patch('x'.repeat(512 * 1024))
    let calls = 0
    const client: GitClient = async <T>() => {
      calls++
      return diff as T
    }
    const factory = vi.fn(() => new FakeWorker())
    const cache = createCommitDiffCache(client, factory)
    cleanup.push(cache.dispose)
    cache.reset({ sessionId: 'session', sha: 'commit', files })
    cache.preload()
    await vi.advanceTimersByTimeAsync(10_000)
    expect(calls).toBe(16)
    expect(factory).not.toHaveBeenCalled()
    expect((await cache.read(files[0]!)).diff).toBe(diff)
    expect(calls).toBe(16)
    cache.preload()
    await vi.advanceTimersByTimeAsync(1_000)
    expect(calls).toBe(16)
  })

  it('limits entries to 256 while preserving the most recently read entry', async () => {
    const files = Array.from({ length: 257 }, (_, index) => file(index, `file-${index}.txt`))
    let calls = 0
    const client: GitClient = async <T>() => {
      calls++
      return patch() as T
    }
    const cache = createCommitDiffCache(client)
    cleanup.push(cache.dispose)
    cache.reset({ sessionId: 'session', sha: 'commit', files })
    for (const current of files.slice(0, 256)) await cache.read(current)
    await cache.read(files[0]!)
    await cache.read(files[256]!)
    await cache.read(files[0]!)
    expect(calls).toBe(257)
    await cache.read(files[1]!)
    expect(calls).toBe(258)
  })

  it('evicts by total text size and declines an individually oversized entry', async () => {
    const files = Array.from({ length: 18 }, (_, index) => file(index, `file-${index}.txt`))
    let calls = 0
    let content = 'x'.repeat(512 * 1024)
    const client: GitClient = async <T>() => {
      calls++
      return patch(content) as T
    }
    const cache = createCommitDiffCache(client)
    cleanup.push(cache.dispose)
    cache.reset({ sessionId: 'session', sha: 'commit', files })
    for (const current of files.slice(0, 17)) await cache.read(current)
    await cache.read(files[0]!)
    expect(calls).toBe(18)
    content = 'x'.repeat(8 * 1024 * 1024 + 1)
    await cache.read(files[17]!)
    await cache.read(files[17]!)
    expect(calls).toBe(20)
  })

  it('caps cached tokens at 200000 independently of character size', async () => {
    const files = Array.from({ length: 11 }, (_, index) => file(index))
    const state = setup(files)
    const tokens: HighlightedDiff = [
      [
        Array.from({ length: 20_000 }, (_, index) => ({
          content: 'x',
          kind: index % 2 ? 'plain' : 'keyword',
        })),
      ],
    ]
    for (const current of files) {
      const reading = state.cache.read(current)
      state.calls.at(-1)!.resolve(patch('x'.repeat(20_000)))
      await flush()
      state.workers[0]!.respond(tokens)
      await reading
    }
    const reread = state.cache.read(files[0]!)
    expect(state.calls).toHaveLength(12)
    state.calls[11]!.resolve(patch())
    await flush()
    state.workers[0]!.respond()
    await reread
  })

  it('falls back when a worker response exceeds the rendered token budget', async () => {
    const state = setup()
    const reading = state.cache.read(file(0))
    state.calls[0]!.resolve(patch())
    await flush()
    state.workers[0]!.respond([
      [Array.from({ length: 20_001 }, () => ({ content: 'x', kind: 'plain' }))],
    ])
    expect((await reading).tokens).toBeNull()
  })

  it.each(['error', 'messageerror'] as const)(
    'returns the raw diff after %s and ignores a late response',
    async (failure) => {
      const state = setup()
      const reading = state.cache.read(file(0))
      state.calls[0]!.resolve(patch())
      await flush()
      const worker = state.workers[0]!
      if (failure === 'error') worker.onerror?.call(worker as unknown as Worker, {} as ErrorEvent)
      else worker.onmessageerror?.call(worker as unknown as Worker, {} as MessageEvent)
      const prepared = await reading
      expect(prepared.tokens).toBeNull()
      expect(worker.terminate).toHaveBeenCalledOnce()
      worker.respond(colored('obsolete'))
      expect(await state.cache.read(file(0))).toBe(prepared)
      expect(vi.getTimerCount()).toBe(0)
    },
  )

  it('disables preloading when worker creation fails and still reads selected files', async () => {
    const state = setup()
    state.factory.mockImplementationOnce(() => {
      throw new Error('Worker unavailable')
    })
    const reading = state.cache.read(file(0))
    state.calls[0]!.resolve(patch())
    expect((await reading).tokens).toBeNull()
    state.cache.preload()
    await vi.advanceTimersByTimeAsync(1_000)
    expect(state.calls).toHaveLength(1)
    const next = state.cache.read(file(1))
    state.calls[1]!.resolve(patch())
    expect((await next).tokens).toBeNull()
  })

  it('propagates Git failures and permits a later retry instead of caching an error', async () => {
    const state = setup()
    const reading = state.cache.read(file(0))
    const rejected = expect(reading).rejects.toThrow('Unreadable')
    state.calls[0]!.reject(new Error('Unreadable'))
    await rejected
    const retry = state.cache.read(file(0))
    state.calls[1]!.resolve(patch())
    await flush()
    state.workers[0]!.respond()
    expect((await retry).tokens).toEqual(colored())
  })

  it('disposes timers and worker and rejects subscribers before any late result arrives', async () => {
    const state = setup()
    const reading = state.cache.read(file(0))
    const rejected = expect(reading).rejects.toMatchObject({ name: 'AbortError' })
    state.calls[0]!.resolve(patch())
    await flush()
    const worker = state.workers[0]!
    state.cache.preload()
    state.cache.dispose()
    await rejected
    worker.respond(colored('obsolete'))
    expect(worker.terminate).toHaveBeenCalledOnce()
    expect(vi.getTimerCount()).toBe(0)
    await expect(state.cache.read(file(0))).rejects.toMatchObject({ name: 'AbortError' })
  })
})

it('retries timed-out highlighting on explicit reopening without reading Git again', async () => {
  const state = setup()
  const first = state.cache.read(file(0))
  state.calls[0]!.resolve(patch())
  await flush()
  await vi.advanceTimersByTimeAsync(2_000)
  expect((await first).tokens).toBeNull()
  state.workers[0]!.respond(colored('obsolete'))
  const retry = state.cache.read(file(0))
  await flush()
  expect(state.calls).toHaveLength(1)
  await vi.advanceTimersByTimeAsync(2_500)
  state.workers.at(-1)!.respond()
  expect((await retry).tokens).toEqual(colored())
})

it('keeps large commits on demand instead of scanning every file in the background', async () => {
  const state = setup()
  state.cache.reset({ sessionId: 'session', sha: 'large', files: state.files, fileCount: 6_000 })
  state.cache.preload()
  await vi.advanceTimersByTimeAsync(1_000)
  expect(state.calls).toHaveLength(0)
  const explicit = state.cache.read(file(1))
  state.calls[0]!.resolve(patch())
  await flush()
  state.workers[0]!.respond()
  expect((await explicit).tokens).toEqual(colored())
})
