import { describe, expect, it } from 'vite-plus/test'
import { effectScope, shallowRef } from 'vue'
import type { GitClient } from '../lib/api'
import type { FileContent, FileHistoryPage } from '../lib/types'
import { useFileBrowser, type FileSelection } from './useFileBrowser'

const selection = (sessionId = 'repo', sha = 'revision', fileKey = '66696c65'): FileSelection => ({
  sessionId,
  sha,
  file: { fileKey, path: 'file.ts', index: 0, status: 'modified', oldPath: null },
})
const content = (text = 'const value = 1', kind: FileContent['kind'] = 'text'): FileContent => ({
  sha: 'revision',
  fileKey: '66696c65',
  path: 'file.ts',
  kind,
  size: text.length,
  lines: text ? [text] : [],
  truncated: false,
  endsWithNewline: false,
  parentSha: 'parent',
})
const page = (nextCursor: string | null = null): FileHistoryPage => ({
  entries: [],
  nextCursor,
  scanned: 256,
  limitation: null,
})

function setup() {
  const calls: {
    command: string
    args: Record<string, unknown>
    signal?: AbortSignal
    resolve: (value: unknown) => void
    reject: (reason: unknown) => void
  }[] = []
  const client: GitClient = <T>(
    command: string,
    args: Record<string, unknown>,
    signal?: AbortSignal,
  ) =>
    new Promise<T>((resolve, reject) => {
      calls.push({ command, args, signal, resolve: (value) => resolve(value as T), reject })
    })
  const source = shallowRef<FileSelection | null>(selection())
  const scope = effectScope()
  const browser = scope.run(() => useFileBrowser(() => source.value, client))!
  return { calls, source, scope, browser, client }
}

describe('file views', () => {
  it('keeps an identical view request in flight without restarting its native read', async () => {
    const { browser, calls, scope } = setup()
    const first = browser.show('file')
    await browser.show('file')
    expect(calls).toHaveLength(1)
    expect(calls[0]!.signal?.aborted).toBe(false)
    calls[0]!.resolve(content())
    await first
    expect(browser.content.value?.lines).toEqual(['const value = 1'])
    expect(browser.loading.value).toBe(false)
    scope.stop()
  })

  it('loads only the requested view and reuses data within the same revision', async () => {
    const { browser, calls, scope } = setup()
    expect(calls).toHaveLength(0)
    const read = browser.show('file')
    expect(calls[0]?.command).toBe('file_content')
    expect(calls[0]?.args).toEqual({ sessionId: 'repo', sha: 'revision', fileKey: '66696c65' })
    calls[0]!.resolve(content())
    await read
    const historyRead = browser.show('history')
    calls[1]!.resolve(page())
    await historyRead
    await browser.show('file')
    expect(calls).toHaveLength(2)
    expect(browser.target.value?.sha).toBe('revision')
    expect(browser.target.value?.file.fileKey).toBe('66696c65')
    scope.stop()
    expect(browser.content.value).toBeNull()
  })

  for (const change of ['file', 'commit', 'repository', 'close'] as const) {
    it(`ignores late content after a ${change} selection`, async () => {
      const { browser, calls, source, scope } = setup()
      const read = browser.show('file')
      source.value =
        change === 'close'
          ? null
          : selection(
              change === 'repository' ? 'other' : 'repo',
              change === 'commit' ? 'other-revision' : 'revision',
              change === 'file' ? '6f74686572' : '66696c65',
            )
      expect(calls[0]!.signal?.aborted).toBe(true)
      calls[0]!.resolve(content('stale'))
      await read
      expect(browser.content.value).toBeNull()
      expect(browser.loading.value).toBe(false)
      scope.stop()
    })
  }

  it('ignores late history and its pagination bookkeeping after a new selection', async () => {
    const { browser, calls, source, scope } = setup()
    const first = browser.show('history')
    calls[0]!.resolve(page('next'))
    await first
    const next = browser.page(1)
    source.value = selection('other-repo')
    calls[1]!.resolve(page('stale-next'))
    await next
    expect(browser.history.value).toBeNull()
    expect(browser.previousCursors.value).toEqual([])
    scope.stop()
  })

  it('ignores a late blame after switching back to the diff', async () => {
    const { browser, calls, scope } = setup()
    const read = browser.show('blame')
    calls[0]!.resolve(content())
    await Promise.resolve()
    await Promise.resolve()
    expect(calls[1]!.command).toBe('file_blame')
    const diff = browser.show('diff')
    expect(calls).toHaveLength(2)
    calls[1]!.resolve({ groups: [{ sha: 'stale' }], limitation: null })
    await read
    calls[2]!.resolve({ binary: false, truncated: false, additions: 1, deletions: 0, hunks: [] })
    await Promise.all([read, diff])
    expect(browser.blame.value).toBeNull()
    expect(browser.mode.value).toBe('diff')
    scope.stop()
  })

  it('uses the retained initial diff without an extra native read', async () => {
    const { browser, source, calls, scope } = setup()
    source.value = {
      ...selection(),
      initialDiff: { binary: false, truncated: false, additions: 2, deletions: 1, hunks: [] },
    }
    await browser.show('diff')
    expect(calls).toHaveLength(0)
    expect(browser.diff.value?.additions).toBe(2)
    scope.stop()
  })

  for (const kind of ['binary', 'deleted', 'large', 'submodule'] as const) {
    it(`does not request blame for ${kind} content`, async () => {
      const { browser, calls, scope } = setup()
      const read = browser.show('blame')
      calls[0]!.resolve(content('', kind))
      await read
      expect(calls).toHaveLength(1)
      scope.stop()
    })
  }

  it('does not request blame for an empty file', async () => {
    const { browser, calls, scope } = setup()
    const read = browser.show('blame')
    calls[0]!.resolve(content(''))
    await read
    expect(calls).toHaveLength(1)
    scope.stop()
  })

  it('retains only one history page and traverses empty pages', async () => {
    const { browser, calls, scope } = setup()
    const first = browser.show('history')
    calls[0]!.resolve(page('next'))
    await first
    const next = browser.page(1)
    expect(calls[1]!.args.cursor).toBe('next')
    calls[1]!.resolve(page('third'))
    await next
    expect(browser.previousCursors.value).toEqual([null])
    const previous = browser.page(-1)
    expect(calls[2]!.args.cursor).toBeNull()
    calls[2]!.resolve(page('next'))
    await previous
    expect(browser.previousCursors.value).toEqual([])
    scope.stop()
  })

  it('retains the displayed history page and cursor when another view cancels pagination', async () => {
    const { browser, calls, scope } = setup()
    const first = browser.show('history')
    const initial = page('next')
    calls[0]!.resolve(initial)
    await first
    const next = browser.page(1)
    expect(browser.history.value).toBe(initial)
    const file = browser.show('file')
    const history = browser.show('history')
    expect(browser.history.value).toBe(initial)
    calls[1]!.resolve(page('stale'))
    await Promise.all([next, file, history])
    expect(calls).toHaveLength(2)
    expect(browser.history.value).toBe(initial)
    expect(browser.previousCursors.value).toEqual([])
    const retry = browser.page(1)
    expect(calls[2]!.args.cursor).toBe('next')
    calls[2]!.resolve(page('third'))
    await retry
    expect(browser.previousCursors.value).toEqual([null])
    scope.stop()
  })

  it('retries a failed history page without discarding its previous page or cursor stack', async () => {
    const { browser, calls, scope } = setup()
    const first = browser.show('history')
    const initial = page('next')
    calls[0]!.resolve(initial)
    await first
    const next = browser.page(1)
    calls[1]!.reject(new Error('Lecture interrompue'))
    await next
    expect(browser.history.value).toBe(initial)
    expect(browser.previousCursors.value).toEqual([])
    const retry = browser.show('history')
    expect(calls[2]!.args.cursor).toBe('next')
    calls[2]!.resolve(page('third'))
    await retry
    expect(browser.error.value).toBe('')
    expect(browser.previousCursors.value).toEqual([null])
    scope.stop()
  })

  it('starts only the latest view after an uncancellable native request settles', async () => {
    const { browser, calls, scope } = setup()
    const first = browser.show('file')
    const second = browser.show('history')
    const latest = browser.show('blame')
    expect(calls).toHaveLength(1)
    calls[0]!.resolve(content('stale'))
    await Promise.all([first, second])
    expect(calls).toHaveLength(2)
    expect(calls[1]!.command).toBe('file_content')
    calls[1]!.resolve(content('latest'))
    await Promise.resolve()
    await Promise.resolve()
    expect(calls[2]!.command).toBe('file_blame')
    calls[2]!.resolve({ groups: [], limitation: null })
    await latest
    expect(browser.content.value?.lines).toEqual(['latest'])
    scope.stop()
  })

  it('keeps the native read slot occupied after closing and reopening the workspace', async () => {
    const { browser, calls, client, scope } = setup()
    const closing = browser.show('file')
    scope.stop()
    const reopenedScope = effectScope()
    const reopened = reopenedScope.run(() => useFileBrowser(() => selection(), client))!
    const history = reopened.show('history')
    expect(calls).toHaveLength(1)
    calls[0]!.resolve(content('stale'))
    await closing
    expect(calls[1]!.command).toBe('file_history')
    calls[1]!.resolve(page())
    await history
    expect(browser.content.value).toBeNull()
    expect(reopened.history.value).not.toBeNull()
    reopenedScope.stop()
  })

  it('returns to the original revision with its existing diff', async () => {
    const { browser, source, calls, scope } = setup()
    const diff = { binary: false, truncated: false, additions: 1, deletions: 0, hunks: [] }
    source.value = { ...selection(), initialDiff: diff }
    const historic = browser.selectRevision({
      sha: 'old',
      path: 'old.ts',
      fileKey: '6f6c64',
      status: 'added',
      oldPath: null,
    })
    calls[0]!.resolve({ ...diff, additions: 2 })
    await historic
    await browser.restoreSelection()
    expect(calls).toHaveLength(1)
    expect(browser.diff.value).toBe(diff)
    expect(browser.target.value?.sha).toBe('revision')
    expect(browser.target.value?.file.path).toBe('file.ts')
    scope.stop()
  })

  it('selects a historic revision with its historic path for every subsequent view', async () => {
    const { browser, calls, scope } = setup()
    const read = browser.selectRevision(
      { sha: 'old-revision', path: 'old.ts', fileKey: '6f6c64', oldPath: null, status: 'added' },
      'file',
    )
    calls[0]!.resolve(content())
    await read
    const history = browser.show('history')
    expect(calls[1]!.args).toEqual({
      sessionId: 'repo',
      sha: 'old-revision',
      fileKey: '6f6c64',
      cursor: null,
    })
    calls[1]!.resolve(page())
    await history
    expect(browser.target.value?.file.path).toBe('old.ts')
    scope.stop()
  })

  it('does not let a stale commit inspection overwrite a newer group or file', async () => {
    const { browser, calls, source, scope } = setup()
    const first = browser.inspectCommit('first')
    const next = browser.inspectCommit('next')
    calls[0]!.resolve({ sha: 'first' })
    await first
    expect(browser.inspectedCommit.value).toBeNull()
    source.value = selection('other')
    calls[1]!.resolve({ sha: 'next' })
    await next
    expect(browser.inspectedCommit.value).toBeNull()
    expect(browser.inspecting.value).toBe(false)
    scope.stop()
  })

  it('reuses the same commit inspection while pending and after it is displayed', async () => {
    const { browser, calls, scope } = setup()
    const first = browser.inspectCommit('selected-group')
    await browser.inspectCommit('selected-group')
    expect(calls).toHaveLength(1)
    expect(calls[0]!.signal?.aborted).toBe(false)
    calls[0]!.resolve({ sha: 'selected-group' })
    await first
    const displayed = browser.inspectedCommit.value
    await browser.inspectCommit('selected-group')
    expect(calls).toHaveLength(1)
    expect(browser.inspectedCommit.value).toBe(displayed)
    expect(browser.inspecting.value).toBe(false)
    scope.stop()
  })

  it('closes pending inspection and ignores its late result or error', async () => {
    const { browser, calls, scope } = setup()
    const pending = browser.inspectCommit('selected-group')
    browser.closeInspection()
    calls[0]!.reject(new Error('Late inspection error'))
    await pending
    expect(browser.inspectedCommit.value).toBeNull()
    expect(browser.inspectionError.value).toBe('')
    expect(browser.inspecting.value).toBe(false)
    scope.stop()
  })

  it('reports missing objects and allows retry without a stale error', async () => {
    const { browser, calls, scope } = setup()
    const first = browser.show('file')
    calls[0]!.reject(new Error('Objet Git absent. Aucun fetch automatique.'))
    await first
    expect(browser.error.value).toContain('Objet Git absent')
    const retry = browser.show('file')
    calls[1]!.resolve(content())
    await retry
    expect(browser.error.value).toBe('')
    scope.stop()
  })
})
