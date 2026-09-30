import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test'
import { effectScope, ref, shallowRef } from 'vue'
import type { GitClient } from '../lib/api'
import type { FetchResult, RepositoryInfo } from '../lib/types'
import { getStoredAutoFetchInterval, storeAutoFetchInterval } from '../lib/autoFetch'
import { useAutoFetch } from './useAutoFetch'

const repositoryInfo = (id = 'one', remotes = ['https://example.test/repo']): RepositoryInfo => ({
  id,
  name: id,
  path: `/repos/${id}`,
  head: null,
  headName: 'main',
  references: [],
  remotes,
  index: { indexed: 0, complete: true, error: null },
})

const scopes: ReturnType<typeof effectScope>[] = []
function setup(minutes = 1) {
  const repository = shallowRef<RepositoryInfo | null>(repositoryInfo())
  const interval = ref(minutes)
  const client = vi.fn<GitClient>().mockResolvedValue({ error: null })
  const refresh = vi.fn(async () => {})
  const scope = effectScope()
  scopes.push(scope)
  const state = scope.run(() => useAutoFetch(repository, interval, refresh, client as GitClient))!
  return { repository, interval, client, refresh, scope, state }
}

beforeEach(() => vi.useFakeTimers())
afterEach(() => {
  scopes.splice(0).forEach((scope) => scope.stop())
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

describe('auto-fetch', () => {
  it('waits for the interval, then refreshes after fetching the active repository', async () => {
    const { client, refresh } = setup()
    await vi.advanceTimersByTimeAsync(59_999)
    expect(client).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(1)
    expect(client).toHaveBeenCalledWith('fetch_repository', { sessionId: 'one' })
    expect(refresh).toHaveBeenCalledOnce()
    await vi.advanceTimersByTimeAsync(60_000)
    expect(client).toHaveBeenCalledTimes(2)
  })

  it('applies interval changes and cancels timers when disabled or without remotes', async () => {
    const { client, interval, repository } = setup()
    await vi.advanceTimersByTimeAsync(30_000)
    interval.value = 2
    await vi.advanceTimersByTimeAsync(119_999)
    expect(client).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(1)
    expect(client).toHaveBeenCalledOnce()
    interval.value = 0
    await vi.advanceTimersByTimeAsync(180_000)
    expect(client).toHaveBeenCalledOnce()
    interval.value = 1
    repository.value = repositoryInfo('local', [])
    await vi.advanceTimersByTimeAsync(60_000)
    expect(client).toHaveBeenCalledOnce()
  })

  it('cancels an armed timer when the active repository closes and resumes on reopening', async () => {
    const { client, repository } = setup()
    await vi.advanceTimersByTimeAsync(30_000)
    repository.value = null
    expect(vi.getTimerCount()).toBe(0)
    await vi.advanceTimersByTimeAsync(180_000)
    expect(client).not.toHaveBeenCalled()
    expect(vi.getTimerCount()).toBe(0)
    repository.value = repositoryInfo('reopened')
    await vi.advanceTimersByTimeAsync(59_999)
    expect(client).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(1)
    expect(client).toHaveBeenCalledWith('fetch_repository', { sessionId: 'reopened' })
  })

  it.each(['resolve', 'reject'] as const)(
    'ignores a pending fetch that %ss after the active repository closes',
    async (outcome) => {
      const { client, repository, state, refresh } = setup()
      let finish!: () => void
      client.mockImplementationOnce(
        () =>
          new Promise((resolve, reject) => {
            finish = () => {
              if (outcome === 'resolve') resolve({ error: 'closed remote failed' })
              else reject(new Error('closed remote failed'))
            }
          }),
      )
      const fetching = state.fetchNow()
      repository.value = null
      finish()
      await fetching
      await vi.advanceTimersByTimeAsync(180_000)
      expect(refresh).not.toHaveBeenCalled()
      expect(state.error.value).toBe('')
      expect(state.fetching.value).toBe(false)
      expect(client).toHaveBeenCalledOnce()
      expect(vi.getTimerCount()).toBe(0)
    },
  )

  it('never overlaps a slow fetch and starts the next interval after completion', async () => {
    const { client, state, refresh } = setup()
    let finish!: (result: FetchResult) => void
    client.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    await vi.advanceTimersByTimeAsync(60_000)
    expect(state.fetching.value).toBe(true)
    await state.fetchNow()
    await vi.advanceTimersByTimeAsync(180_000)
    expect(client).toHaveBeenCalledOnce()
    finish({ error: null })
    await vi.advanceTimersByTimeAsync(0)
    expect(refresh).toHaveBeenCalledOnce()
    await vi.advanceTimersByTimeAsync(60_000)
    expect(client).toHaveBeenCalledTimes(2)
  })

  it('ignores late responses after switching repositories or disposing the scope', async () => {
    const { client, repository, state, refresh, scope } = setup()
    let finish!: (result: FetchResult) => void
    client.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const fetching = state.fetchNow()
    repository.value = repositoryInfo('two')
    finish({ error: 'old remote failed' })
    await fetching
    expect(refresh).not.toHaveBeenCalled()
    expect(state.error.value).toBe('')
    await vi.advanceTimersByTimeAsync(60_000)
    expect(client).toHaveBeenLastCalledWith('fetch_repository', { sessionId: 'two' })
    client.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const finalFetch = state.fetchNow()
    scope.stop()
    finish({ error: null })
    await finalFetch
    await vi.advanceTimersByTimeAsync(180_000)
    expect(client).toHaveBeenCalledTimes(3)
    expect(refresh).toHaveBeenCalledOnce()
  })

  it('reports partial failures while refreshing successful remotes, then retries', async () => {
    const { client, state, refresh, repository } = setup()
    refresh.mockImplementation(async () => {
      repository.value = { ...repositoryInfo('updated'), path: '/repos/one' }
    })
    client.mockResolvedValueOnce({ error: 'origin : authentication failed' })
    await state.fetchNow()
    expect(state.error.value).toContain('authentication failed')
    expect(refresh).toHaveBeenCalledOnce()
    client.mockRejectedValueOnce('Git unavailable')
    await vi.advanceTimersByTimeAsync(60_000)
    expect(state.error.value).toContain('Git unavailable')
    await vi.advanceTimersByTimeAsync(60_000)
    expect(state.error.value).toBe('')
    expect(refresh).toHaveBeenCalledTimes(2)
  })
})

describe('auto-fetch preference', () => {
  it('restores valid values including zero, and rejects malformed preferences', () => {
    const storage = new Map<string, string>()
    vi.stubGlobal('localStorage', {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, value: string) => storage.set(key, value),
    })
    expect(getStoredAutoFetchInterval()).toBe(1)
    for (const value of [0, 1, 60]) {
      storeAutoFetchInterval(value)
      expect(getStoredAutoFetchInterval()).toBe(value)
    }
    storeAutoFetchInterval(61)
    expect(getStoredAutoFetchInterval()).toBe(60)
    for (const value of ['', 'NaN', '-1', '61', '1.5']) {
      storage.set('mangrove:auto-fetch-interval', value)
      expect(getStoredAutoFetchInterval()).toBe(1)
    }
    vi.stubGlobal('localStorage', {
      getItem: () => {
        throw new Error('blocked')
      },
    })
    expect(getStoredAutoFetchInterval()).toBe(1)
    expect(() => storeAutoFetchInterval(0)).not.toThrow()
  })
})
