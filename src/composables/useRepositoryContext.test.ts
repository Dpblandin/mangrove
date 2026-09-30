import { describe, expect, it, vi } from 'vite-plus/test'
import { effectScope, ref, shallowRef } from 'vue'
import type { GitClient } from '../lib/api'
import type { RepositoryInfo, StashDetails, StashList } from '../lib/types'
import { useRepositoryContext } from './useRepositoryContext'

function pending() {
  const calls: {
    command: string
    args: Record<string, unknown>
    signal?: AbortSignal
    resolve(value: unknown): void
    reject(reason: unknown): void
  }[] = []
  const client: GitClient = <T>(
    command: string,
    args: Record<string, unknown>,
    signal?: AbortSignal,
  ) =>
    new Promise<T>((resolve, reject) =>
      calls.push({ command, args, signal, resolve: (value) => resolve(value as T), reject }),
    )
  return { calls, client }
}
const repo = (id: string): RepositoryInfo => ({
  id,
  path: `/tmp/${id}`,
  name: id,
  head: null,
  headName: 'main',
  references: [],
  remotes: [],
  index: { complete: true, indexed: 0, error: null },
})
const stash = (sha: string): StashDetails => ({
  sha,
  message: sha,
  author: 'Author',
  time: 0,
  baseSha: 'base',
  groups: [
    {
      area: 'index',
      comparison: 'Base → index',
      oldSha: 'base',
      newSha: 'index',
      fileCount: 1,
      files: [
        { index: 0, fileKey: '612e747874', path: 'a.txt', oldPath: null, status: 'modified' },
      ],
    },
  ],
})
const list = (sha: string): StashList => ({
  stashes: [{ sha, index: 0, message: sha, author: 'Author', time: 0, baseSha: 'base' }],
  truncated: false,
})
function setup() {
  const transport = pending()
  const repository = shallowRef<RepositoryInfo | null>(repo('first'))
  const active = ref(true)
  const scope = effectScope()
  const state = scope.run(() =>
    useRepositoryContext(repository, () => active.value, transport.client),
  )!
  return { ...transport, state, repository, active, scope }
}

describe('repository context', () => {
  it('loads lists on demand, deduplicates requests and ignores late lists from a previous repository', async () => {
    const { state, calls, repository, scope } = setup()
    try {
      expect(calls).toHaveLength(0)
      const old = state.loadStashes()
      expect(state.loadStashes()).toBe(old)
      repository.value = repo('second')
      const current = state.loadStashes()
      calls[0]!.resolve(list('old'))
      await old
      expect(state.stashes.value).toBeNull()
      expect(state.stashesLoading.value).toBe(true)
      calls[1]!.resolve(list('current'))
      await current
      expect(state.stashes.value?.stashes[0]?.sha).toBe('current')
      expect(state.stashesLoading.value).toBe(false)
    } finally {
      scope.stop()
    }
  })

  it('keeps stash identity by SHA and ignores old details after a new selection', async () => {
    const { state, calls, scope } = setup()
    try {
      const old = state.selectStash('old')
      const current = state.selectStash('current')
      calls[1]!.resolve(stash('current'))
      await current
      calls[0]!.resolve(stash('old'))
      await old
      expect(state.details.value?.sha).toBe('current')
      expect(state.selectedSha.value).toBe('current')
    } finally {
      scope.stop()
    }
  })

  it('releases a pending diff when leaving the stash view without publishing a late error', async () => {
    const { state, calls, active, scope } = setup()
    try {
      const selection = state.selectStash('saved')
      calls[0]!.resolve(stash('saved'))
      await selection
      const reading = state.selectFile('index', state.details.value!.groups[0]!.files[0]!)
      await vi.waitFor(() => expect(calls.some((call) => call.command === 'stash_diff')).toBe(true))
      active.value = false
      calls.find((call) => call.command === 'stash_diff')!.reject(new Error('late native error'))
      await reading
      expect(state.details.value).toBeNull()
      expect(state.selectedFile.value).toBeNull()
      expect(state.diffError.value).toBe('')
      expect(state.fileLoading.value).toBe(false)
    } finally {
      scope.stop()
    }
  })

  it('clears a stash removed externally and its cached selection', async () => {
    const { state, calls, scope } = setup()
    try {
      const selection = state.selectStash('removed')
      calls[0]!.resolve(stash('removed'))
      await selection
      const refreshing = state.loadStashes()
      calls[1]!.resolve({ stashes: [], truncated: false })
      await refreshing
      expect(state.details.value).toBeNull()
      expect(state.selectedSha.value).toBe('')
    } finally {
      scope.stop()
    }
  })
})

describe('stash read cancellation', () => {
  it('aborts native details when selecting another stash or closing its view', async () => {
    const { state, calls, active, scope } = setup()
    try {
      const old = state.selectStash('old')
      const latest = state.selectStash('latest')
      expect(calls[0]!.signal?.aborted).toBe(true)
      active.value = false
      expect(calls[1]!.signal?.aborted).toBe(true)
      calls[0]!.resolve(stash('old'))
      calls[1]!.resolve(stash('latest'))
      await Promise.all([old, latest])
      expect(state.details.value).toBeNull()
      expect(state.detailsLoading.value).toBe(false)
    } finally {
      scope.stop()
    }
  })

  it('aborts the previous file subscription and keeps the newly selected patch', async () => {
    const { state, calls, scope } = setup()
    try {
      const value = stash('selected')
      value.groups[0]!.files.push({
        index: 1,
        fileKey: '622e747874',
        path: 'b.txt',
        oldPath: null,
        status: 'modified',
      })
      const selection = state.selectStash('selected')
      calls[0]!.resolve(value)
      await selection
      const readingA = state.selectFile('index', value.groups[0]!.files[0]!)
      await vi.waitFor(() => expect(calls).toHaveLength(2))
      const readingB = state.selectFile('index', value.groups[0]!.files[1]!)
      expect(calls[1]!.signal?.aborted).toBe(true)
      calls[1]!.resolve({ binary: true, truncated: false, additions: 9, deletions: 0, hunks: [] })
      await vi.waitFor(() => expect(calls).toHaveLength(3))
      calls[2]!.resolve({ binary: true, truncated: false, additions: 2, deletions: 0, hunks: [] })
      await Promise.all([readingA, readingB])
      expect(state.selectedFile.value?.path).toBe('b.txt')
      expect(state.diff.value?.additions).toBe(2)
    } finally {
      scope.stop()
    }
  })
})
