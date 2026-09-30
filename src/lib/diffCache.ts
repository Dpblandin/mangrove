import type { GitClient } from './api'
import {
  canHighlight,
  MAX_SYNTAX_TOKENS,
  syntaxLanguage,
  type HighlightedDiff,
  type SyntaxRequest,
  type SyntaxResponse,
} from './diffSyntax'
import type { ChangedFile, FileDiff } from './types'
import type {
  DiffHighlightWorker,
  DiffHighlightWorkerFactory,
} from '../composables/useDiffHighlighting'

export type PreparedDiff = { diff: FileDiff; tokens: HighlightedDiff | null }
type Context<File> = {
  files: File[]
  read: (file: File, signal: AbortSignal) => Promise<FileDiff>
}
type Cached = { value: PreparedDiff; characters: number; tokens: number; retryHighlight: boolean }
type DiffJob<File> = {
  key: string
  generation: number
  file: File
  controller: AbortController
  background: boolean
  waiters: Set<symbol>
  state: 'reading' | 'ready' | 'highlighting' | 'settled'
  diff?: FileDiff
  promise: Promise<PreparedDiff>
  resolve: (value: PreparedDiff) => void
  reject: (reason: unknown) => void
}

const MAX_CHARACTERS = 8 * 1024 * 1024
const MAX_CACHED_TOKENS = 200_000
const MAX_ENTRIES = 256
const TIMEOUT_MS = 2_000
const FOREGROUND_TIMEOUT_MS = 8_000
const PRELOAD_PAUSE_MS = 150

function aborted() {
  return new DOMException('Requête annulée', 'AbortError')
}

function createWorker(): DiffHighlightWorker {
  return new Worker(new URL('../workers/diffHighlight.worker.ts', import.meta.url), {
    type: 'module',
  })
}

function tokenSize(tokens: HighlightedDiff | null) {
  let count = 0
  let characters = 0
  if (tokens)
    for (const hunk of tokens)
      for (const line of hunk)
        for (const token of line) {
          count++
          characters += token.content.length
        }
  return { count, characters }
}

/** Owns one selection. Pending native reads survive abort until they settle. */
export function createDiffCache<File extends { path: string }>(
  fileKey: (file: File) => string,
  workerFactory?: DiffHighlightWorkerFactory,
) {
  type Job = DiffJob<File>
  type Active = { job: Job; id: number; startedAt: number; timer: ReturnType<typeof setTimeout> }
  const factory = workerFactory ?? createWorker
  let workerEnabled = Boolean(workerFactory) || typeof Worker !== 'undefined'
  let context: Context<File> | null = null
  let generation = 0
  let disposed = false
  const cache = new Map<string, Cached>()
  const jobs = new Map<string, Job>()
  let cachedCharacters = 0
  let cachedTokens = 0
  let backgroundFull = false
  let worker: DiffHighlightWorker | undefined
  let active: Active | undefined
  let nextId = 0
  let preloadWanted = false
  let releaseWorkerWhenIdle = false
  let preloadCursor = 0
  let preloadTimer: ReturnType<typeof setTimeout> | undefined
  let backgroundJob: Job | undefined
  // This slot is released by the actual Git promise, never by reset/abort.
  let backgroundGit: Job | undefined

  function current(job: Job) {
    return !disposed && job.generation === generation && jobs.get(job.key) === job
  }

  function hasForeground() {
    return [...jobs.values()].some((job) => job.waiters.size > 0)
  }

  function backgroundBudgetFull() {
    return (
      backgroundFull ||
      cache.size >= MAX_ENTRIES ||
      cachedCharacters >= MAX_CHARACTERS ||
      cachedTokens >= MAX_CACHED_TOKENS
    )
  }

  function stopWorker() {
    if (active) {
      clearTimeout(active.timer)
      active.job.state = 'ready'
    }
    active = undefined
    const previous = worker
    worker = undefined
    previous?.terminate()
  }

  function remember(job: Job, value: PreparedDiff, retryHighlight: boolean) {
    const size = tokenSize(value.tokens)
    let characters = size.characters
    for (const hunk of value.diff.hunks) {
      characters += hunk.header.length
      for (const line of hunk.lines) characters += line.content.length
    }
    // A huge selection must stop prefetching instead of rotating through thousands of diffs.
    // Explicit reads can still evict the LRU; a promoted background job is explicit too.
    if (
      job.background &&
      job.waiters.size === 0 &&
      (cache.size + 1 > MAX_ENTRIES ||
        cachedCharacters + characters > MAX_CHARACTERS ||
        cachedTokens + size.count > MAX_CACHED_TOKENS)
    ) {
      backgroundFull = true
      return
    }
    if (characters > MAX_CHARACTERS || size.count > MAX_CACHED_TOKENS) return
    const previous = cache.get(job.key)
    if (previous) {
      cachedCharacters -= previous.characters
      cachedTokens -= previous.tokens
    }
    cache.delete(job.key)
    cache.set(job.key, { value, characters, tokens: size.count, retryHighlight })
    cachedCharacters += characters
    cachedTokens += size.count
    while (
      cache.size > MAX_ENTRIES ||
      cachedCharacters > MAX_CHARACTERS ||
      cachedTokens > MAX_CACHED_TOKENS
    ) {
      const oldest = cache.keys().next().value!
      const entry = cache.get(oldest)!
      cache.delete(oldest)
      cachedCharacters -= entry.characters
      cachedTokens -= entry.tokens
    }
  }

  function finish(job: Job, tokens: HighlightedDiff | null, retryHighlight = false) {
    if (!current(job) || !job.diff) return
    const value = { diff: job.diff, tokens }
    remember(job, value, retryHighlight)
    jobs.delete(job.key)
    job.state = 'settled'
    job.resolve(value)
    scheduleBackground()
  }

  function cancel(job: Job, reason: unknown = aborted()) {
    if (!current(job)) return
    if (active?.job === job) stopWorker()
    jobs.delete(job.key)
    job.state = 'settled'
    job.controller.abort()
    job.reject(reason)
    pumpWorker()
    scheduleBackground()
  }

  function getWorker() {
    if (worker) return worker
    const created = factory()
    worker = created
    created.onmessage = ({ data }: MessageEvent<SyntaxResponse>) => {
      if (worker !== created || !active || data.id !== active.id) return
      const job = active.job
      clearTimeout(active.timer)
      active = undefined
      const size = tokenSize(data.tokens)
      finish(
        job,
        size.count <= MAX_SYNTAX_TOKENS && size.characters <= 256 * 1024 ? data.tokens : null,
      )
      pumpWorker()
    }
    const failure = () => {
      if (worker !== created) return
      const job = active?.job
      stopWorker()
      if (job) finish(job, null)
      pumpWorker()
    }
    created.onerror = failure
    created.onmessageerror = failure
    return created
  }

  function highlightTimer(job: Job, id: number, delay: number) {
    return setTimeout(() => {
      if (active?.id !== id) return
      stopWorker()
      finish(job, null, true)
      pumpWorker()
    }, delay)
  }

  function pumpWorker() {
    if (active) {
      if (active.job.waiters.size === 0 && hasForeground()) stopWorker()
      else return
    }
    const ready = [...jobs.values()].filter((job) => job.state === 'ready')
    const foreground = ready.find((job) => job.waiters.size > 0)
    const job =
      foreground ??
      (!hasForeground() && preloadWanted && !backgroundBudgetFull() ? ready[0] : undefined)
    if (!job || !job.diff) {
      if (releaseWorkerWhenIdle && !hasForeground()) stopWorker()
      return
    }
    if (!workerEnabled) {
      finish(job, null)
      pumpWorker()
      return
    }
    try {
      const target = getWorker()
      const id = ++nextId
      job.state = 'highlighting'
      active = {
        job,
        id,
        startedAt: Date.now(),
        timer: highlightTimer(job, id, job.waiters.size > 0 ? FOREGROUND_TIMEOUT_MS : TIMEOUT_MS),
      }
      const request: SyntaxRequest = {
        id,
        path: job.file.path,
        hunks: job.diff.hunks.map((hunk) => ({
          header: hunk.header,
          lines: hunk.lines.map((line) => ({
            kind: line.kind,
            oldLine: line.oldLine,
            newLine: line.newLine,
            content: line.content,
          })),
        })),
      }
      target.postMessage(request)
    } catch {
      // Unavailable workers must not trigger a background scan of plain diffs.
      workerEnabled = false
      stopWorker()
      finish(job, null)
      pumpWorker()
    }
  }

  function start(job: Job) {
    const read = context!.read
    if (job.background) backgroundGit = job
    void (async () => {
      try {
        const diff = await read(job.file, job.controller.signal)
        if (!current(job)) return
        job.diff = diff
        if (
          !workerEnabled ||
          diff.binary ||
          !syntaxLanguage(job.file.path) ||
          !canHighlight(diff.hunks)
        )
          finish(job, null)
        else {
          job.state = 'ready'
          pumpWorker()
        }
      } catch (cause) {
        cancel(job, cause)
      } finally {
        if (backgroundGit === job) backgroundGit = undefined
        scheduleBackground()
      }
    })()
  }

  function makeJob(file: File, background: boolean) {
    let resolve!: Job['resolve']
    let reject!: Job['reject']
    const promise = new Promise<PreparedDiff>((done, fail) => {
      resolve = done
      reject = fail
    })
    // Background work and reset can settle before a foreground subscriber arrives.
    void promise.catch(() => {})
    const job: Job = {
      key: fileKey(file),
      generation,
      file,
      background,
      controller: new AbortController(),
      waiters: new Set(),
      state: 'reading',
      promise,
      resolve,
      reject,
    }
    jobs.set(job.key, job)
    return job
  }

  function scheduleBackground() {
    if (
      !preloadWanted ||
      !workerEnabled ||
      disposed ||
      !context ||
      backgroundBudgetFull() ||
      preloadTimer !== undefined
    )
      return
    preloadTimer = setTimeout(() => {
      preloadTimer = undefined
      if (backgroundBudgetFull()) return
      pumpWorker()
      if (backgroundJob || backgroundGit || hasForeground()) return
      while (preloadCursor < context!.files.length) {
        const file = context!.files[preloadCursor++]!
        const key = fileKey(file)
        if (!syntaxLanguage(file.path) || cache.has(key) || jobs.has(key)) continue
        const job = makeJob(file, true)
        backgroundJob = job
        const finished = () => {
          if (backgroundJob === job) backgroundJob = undefined
          scheduleBackground()
        }
        void job.promise.then(finished, finished)
        start(job)
        break
      }
    }, PRELOAD_PAUSE_MS)
  }

  function read(file: File, signal?: AbortSignal): Promise<PreparedDiff> {
    if (disposed || !context || signal?.aborted) return Promise.reject(aborted())
    const key = fileKey(file)
    const target = context.files.find((candidate) => fileKey(candidate) === key)
    if (!target)
      return Promise.reject(new Error('Ce fichier n’appartient pas à la sélection courante.'))
    const cached = cache.get(key)
    if (cached && (!cached.retryHighlight || !workerEnabled)) {
      cache.delete(key)
      cache.set(key, cached)
      return Promise.resolve(cached.value)
    }
    const existing = jobs.get(key)
    const job = existing ?? makeJob(target, false)
    const subscriber = Symbol()
    job.waiters.add(subscriber)
    if (active?.job === job && job.waiters.size === 1) {
      // An opened file needs time to load its grammar, even if preparation began in the background.
      clearTimeout(active.timer)
      active.timer = highlightTimer(
        job,
        active.id,
        Math.max(0, FOREGROUND_TIMEOUT_MS - (Date.now() - active.startedAt)),
      )
    }
    pumpWorker()
    if (!existing) {
      if (cached?.retryHighlight) {
        job.diff = cached.value.diff
        job.state = 'ready'
        pumpWorker()
      } else start(job)
    }
    return new Promise<PreparedDiff>((resolve, reject) => {
      let settled = false
      function settle(value?: PreparedDiff, reason?: unknown) {
        if (settled) return
        settled = true
        signal?.removeEventListener('abort', abort)
        job.waiters.delete(subscriber)
        if (reason !== undefined) reject(reason)
        else resolve(value!)
        pumpWorker()
        scheduleBackground()
      }
      function abort() {
        settle(undefined, aborted())
        if (job.waiters.size === 0 && !job.background) cancel(job)
      }
      signal?.addEventListener('abort', abort, { once: true })
      void job.promise.then(
        (value) => settle(value),
        (cause) => settle(undefined, cause),
      )
      if (signal?.aborted) abort()
    })
  }

  function pause() {
    preloadWanted = false
    releaseWorkerWhenIdle = true
    clearTimeout(preloadTimer)
    preloadTimer = undefined
    if (!active || active.job.waiters.size === 0) stopWorker()
  }

  function reset(next: Context<File> | null) {
    pause()
    stopWorker()
    // Invalidate reads synchronously, including native promises which ignore abort.
    for (const job of jobs.values()) {
      job.state = 'settled'
      job.controller.abort()
      job.reject(aborted())
    }
    jobs.clear()
    generation++
    context = disposed || !next ? null : { ...next, files: [...next.files] }
    releaseWorkerWhenIdle = false
    cache.clear()
    cachedCharacters = 0
    cachedTokens = 0
    backgroundFull = false
    backgroundJob = undefined
    preloadCursor = 0
  }

  function preload() {
    if (disposed || !context || !workerEnabled) return
    preloadWanted = true
    releaseWorkerWhenIdle = false
    scheduleBackground()
  }

  function dispose() {
    reset(null)
    disposed = true
  }

  return { reset, read, preload, pause, dispose }
}

export function createCommitDiffCache(
  client: GitClient,
  workerFactory?: DiffHighlightWorkerFactory,
) {
  const cache = createDiffCache<ChangedFile>((file) => `${file.index}:${file.path}`, workerFactory)
  let preloadAllowed = true
  return {
    ...cache,
    preload() {
      if (preloadAllowed) cache.preload()
    },
    reset(
      context: { sessionId: string; sha: string; files: ChangedFile[]; fileCount?: number } | null,
    ) {
      // Native file_diff recalculates the whole commit; large commits stay on demand.
      preloadAllowed = (context?.fileCount ?? context?.files.length ?? 0) <= 1_000
      cache.reset(
        context
          ? {
              files: context.files,
              read: (file, signal) =>
                client<FileDiff>(
                  'file_diff',
                  { sessionId: context.sessionId, sha: context.sha, fileIndex: file.index },
                  signal,
                ),
            }
          : null,
      )
    },
  }
}
