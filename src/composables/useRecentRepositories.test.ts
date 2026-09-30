import { afterEach, describe, expect, it, vi } from 'vite-plus/test'
import { effectScope } from 'vue'
import type { GitClient } from '../lib/api'
import { useRecentRepositories } from './useRecentRepositories'

afterEach(() => vi.unstubAllGlobals())

function setup(saved: unknown, client?: GitClient) {
  let stored = JSON.stringify(saved)
  vi.stubGlobal('localStorage', {
    getItem: () => stored,
    setItem: (_key: string, value: string) => {
      stored = value
    },
  })
  const scope = effectScope()
  const state = scope.run(() => useRecentRepositories(client))!
  return { state, scope, saved: () => JSON.parse(stored) }
}

describe('recent repositories', () => {
  it('bounds restored entries, retains distinct paths and persists removal without reading Git', () => {
    const client = vi.fn(async () => {
      throw new Error('No Git read expected')
    })
    const initial = Array.from({ length: 9 }, (_, index) => ({
      path: `/repo/${index}`,
      name: `repo-${index}`,
    }))
    const { state, scope, saved } = setup([null, initial[0], initial[0], ...initial], client)
    try {
      expect(state.entries.value).toHaveLength(6)
      state.remember({ path: '/repo/2', name: 'mangrove · feature' })
      expect(state.entries.value[0]).toEqual({ path: '/repo/2', name: 'mangrove · feature' })
      state.remove('/repo/2')
      expect(saved()).toEqual(state.entries.value)
      expect(state.entries.value.map((entry) => entry.path)).toEqual([
        '/repo/0',
        '/repo/1',
        '/repo/3',
        '/repo/4',
        '/repo/5',
      ])
      expect(client).not.toHaveBeenCalled()
    } finally {
      scope.stop()
    }
  })

  it('refreshes legacy names without resurrecting removed entries or overwriting a newer opening', async () => {
    let resolve!: (value: unknown) => void
    const client: GitClient = <T>() =>
      new Promise<T>((done) => {
        resolve = (value) => done(value as T)
      })
    const { state, scope, saved } = setup(
      [
        { path: '/wt/removed', name: 't3code-a' },
        { path: '/wt/renamed', name: 't3code-b' },
        { path: '/wt/current', name: 't3code-c' },
        { path: '/missing', name: 'missing' },
      ],
      client,
    )
    try {
      const pending = state.refreshNames()
      expect(state.refreshNames()).toBe(pending)
      state.remove('/wt/removed')
      state.remember({ path: '/wt/current', name: 'mangrove · newer' })
      resolve([
        { path: '/wt/removed', name: 'mangrove · removed' },
        { path: '/wt/renamed', name: 'mangrove · feature' },
        { path: '/wt/current', name: 'mangrove · older' },
        { path: '/missing', name: null },
      ])
      await pending
      expect(saved()).toEqual([
        { path: '/wt/current', name: 'mangrove · newer' },
        { path: '/wt/renamed', name: 'mangrove · feature' },
        { path: '/missing', name: 'missing' },
      ])
    } finally {
      scope.stop()
    }
  })

  it('ignores metadata that arrives after the welcome view is disposed', async () => {
    let resolve!: (value: unknown) => void
    let signal: AbortSignal | undefined
    const client: GitClient = <T>(
      _command: string,
      _args: Record<string, unknown>,
      requestSignal?: AbortSignal,
    ) => {
      signal = requestSignal
      return new Promise<T>((done) => {
        resolve = (value) => done(value as T)
      })
    }
    const initial = [{ path: '/wt/a', name: 't3code-a' }]
    const { state, scope, saved } = setup(initial, client)
    const pending = state.refreshNames()
    scope.stop()
    expect(signal?.aborted).toBe(true)
    resolve([{ path: '/wt/a', name: 'mangrove · feature' }])
    await pending
    expect(saved()).toEqual(initial)
  })

  it('does not overwrite newer openings or recreated entries that retain the same display name', async () => {
    let resolve!: (value: unknown) => void
    const client: GitClient = <T>() =>
      new Promise<T>((done) => {
        resolve = (value) => done(value as T)
      })
    const reopened = { path: '/wt/reopened', name: 'mangrove · main' }
    const recreated = { path: '/wt/recreated', name: 'mangrove · feature' }
    const unchanged = { path: '/wt/unchanged', name: 't3code-old' }
    const { state, scope, saved } = setup([reopened, recreated, unchanged], client)
    try {
      const pending = state.refreshNames()
      state.remember(reopened)
      state.remove(recreated.path)
      state.remember(recreated)
      resolve([
        { path: reopened.path, name: 'mangrove · outdated-main' },
        { path: recreated.path, name: 'mangrove · outdated-feature' },
        { path: unchanged.path, name: 'mangrove · refreshed' },
      ])
      await pending
      expect(saved()).toEqual([
        recreated,
        reopened,
        { path: unchanged.path, name: 'mangrove · refreshed' },
      ])
    } finally {
      scope.stop()
    }
  })

  it('updates an existing label without changing the recent order or restoring a removed entry', () => {
    const first = { path: '/first', name: 'first' }
    const second = { path: '/wt/second', name: 'mangrove · main' }
    const { state, scope, saved } = setup([first, second])
    try {
      state.updateName({ path: second.path, name: 'mangrove · feature' })
      expect(saved()).toEqual([first, { path: second.path, name: 'mangrove · feature' }])
      state.remove(second.path)
      state.updateName({ path: second.path, name: 'mangrove · next-feature' })
      expect(saved()).toEqual([first])
    } finally {
      scope.stop()
    }
  })
})
