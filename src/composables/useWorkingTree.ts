import { computed, onScopeDispose, ref, shallowRef, watch, type Ref } from 'vue'
import { request, type GitClient } from '../lib/api'
import { createDiffCache } from '../lib/diffCache'
import type { HighlightedDiff } from '../lib/diffSyntax'
import type { DiffHighlightWorkerFactory } from './useDiffHighlighting'
import type {
  FileDiff,
  RepositoryInfo,
  WorkingTreeArea,
  WorkingTreeFile,
  WorkingTreeStatus,
} from '../lib/types'

type FileSnapshot = {
  area: WorkingTreeArea | null
  file: WorkingTreeFile | null
  diff: FileDiff | null
  tokens: HighlightedDiff | null
  error: string
}
type PendingRefresh = { controller: AbortController; promise: Promise<void> }
type LocalDiffFile = WorkingTreeFile & { area: WorkingTreeArea }
type Options = { active?: () => boolean; workerFactory?: DiffHighlightWorkerFactory }
const emptySnapshot = (): FileSnapshot => ({
  area: null,
  file: null,
  diff: null,
  tokens: null,
  error: '',
})

export function useWorkingTree(
  repository: Readonly<Ref<RepositoryInfo | null>>,
  client: GitClient = request,
  options: Options = {},
) {
  const active = computed(options.active ?? (() => true))
  const diffCache = createDiffCache<LocalDiffFile>(
    (file) => `${file.area}:${file.key}`,
    options.workerFactory,
  )
  const status = shallowRef<WorkingTreeStatus | null>(null)
  const snapshot = shallowRef(emptySnapshot())
  const selectedFile = computed(() => snapshot.value.file)
  const selectedArea = computed(() => snapshot.value.area)
  const diff = computed(() => snapshot.value.diff)
  const highlightedDiff = computed(() => snapshot.value.tokens)
  const refreshing = ref(false)
  const diffLoading = ref(false)
  const loading = computed(() => refreshing.value || diffLoading.value)
  const statusError = ref('')
  const error = computed(() => statusError.value || snapshot.value.error)
  const controllers = new Set<AbortController>()
  let generation = 0
  let selectionGeneration = 0
  let disposed = false
  let intent: { area: WorkingTreeArea; key: string } | null = null
  let pendingRefresh: PendingRefresh | undefined
  let selectionController: AbortController | undefined

  async function call<T>(
    command: string,
    args: Record<string, unknown>,
    controller: AbortController,
  ) {
    controllers.add(controller)
    try {
      return await client<T>(command, args, controller.signal)
    } finally {
      controllers.delete(controller)
    }
  }

  function cancelFileRead() {
    selectionGeneration++
    selectionController?.abort()
    diffLoading.value = false
  }

  function clearSelection() {
    cancelFileRead()
    intent = null
    snapshot.value = emptySnapshot()
  }

  function resetDiffCache(repo: RepositoryInfo, value: WorkingTreeStatus | null) {
    const sessionId = repo.id
    diffCache.reset(
      value?.available
        ? {
            files: [
              ...value.unstaged.map((file) => ({ ...file, area: 'unstaged' as const })),
              ...value.staged.map((file) => ({ ...file, area: 'staged' as const })),
            ],
            read: (file, signal) =>
              client<FileDiff>(
                'working_tree_diff',
                { sessionId, area: file.area, fileKey: file.key },
                signal,
              ),
          }
        : null,
    )
  }

  function resetSession(keepSelection = false) {
    generation++
    for (const controller of controllers) controller.abort()
    pendingRefresh = undefined
    refreshing.value = false
    statusError.value = ''
    if (keepSelection) cancelFileRead()
    else {
      status.value = null
      clearSelection()
    }
    diffCache.reset(null)
  }

  function refresh(): Promise<void> {
    const repo = repository.value
    if (!repo || disposed) return Promise.resolve()
    if (pendingRefresh) return pendingRefresh.promise
    const current = generation
    const entry: PendingRefresh = { controller: new AbortController(), promise: Promise.resolve() }
    pendingRefresh = entry
    refreshing.value = true
    statusError.value = ''
    // A status refresh is a new mutable snapshot, even when every path stays the same.
    // Invalidate pending selections before resetting their cache, keeping the last display.
    cancelFileRead()
    resetDiffCache(repo, status.value)
    entry.promise = (async () => {
      try {
        const value = await call<WorkingTreeStatus>(
          'working_tree_status',
          { sessionId: repo.id },
          entry.controller,
        )
        if (current !== generation || repository.value?.id !== repo.id) return
        status.value = value
        resetDiffCache(repo, value)
        const target = intent
        if (!value.available) clearSelection()
        else if (target) {
          const file = value[target.area].find((candidate) => candidate.key === target.key)
          if (file && active.value) void selectFile(target.area, file)
          else if (!file) clearSelection()
        }
        if (active.value) diffCache.preload()
      } catch (cause) {
        if (current === generation && !entry.controller.signal.aborted)
          statusError.value = `Impossible de lire les modifications locales : ${String(cause)}`
      } finally {
        if (pendingRefresh === entry) {
          pendingRefresh = undefined
          refreshing.value = false
        }
      }
    })()
    return entry.promise
  }

  async function selectFile(area: WorkingTreeArea, file: WorkingTreeFile) {
    const repo = repository.value
    const currentStatus = status.value
    if (!repo || disposed || !active.value || !currentStatus?.available) return
    const target = currentStatus[area].find((candidate) => candidate.key === file.key)
    if (!target) return
    const current = generation
    const selection = ++selectionGeneration
    selectionController?.abort()
    const controller = new AbortController()
    selectionController = controller
    intent = { area, key: target.key }
    diffLoading.value = true
    statusError.value = ''
    let patch: FileDiff | null = null
    let tokens: HighlightedDiff | null = null
    let failure = ''
    try {
      const prepared = await diffCache.read({ ...target, area }, controller.signal)
      patch = prepared.diff
      tokens = prepared.tokens
    } catch (cause) {
      const location = area === 'staged' ? 'index' : 'répertoire de travail'
      failure = `Impossible de lire ${target.path} (${location}) : ${String(cause)}`
    } finally {
      if (
        current === generation &&
        selection === selectionGeneration &&
        !controller.signal.aborted
      ) {
        snapshot.value = { area, file: target, diff: patch, tokens, error: failure }
        diffLoading.value = false
      }
    }
  }

  watch(
    active,
    (value) => {
      if (value) {
        // An inactive view can still have a status refresh in flight. Its cache was
        // released on leaving; allow a foreground read while that refresh settles.
        if (pendingRefresh && repository.value) resetDiffCache(repository.value, status.value)
        void refresh()
      } else {
        cancelFileRead()
        diffCache.reset(null)
      }
    },
    { flush: 'sync' },
  )
  watch(
    repository,
    (repo, previous) => {
      if (repo?.id === previous?.id) return
      resetSession(Boolean(repo && repo.path === previous?.path))
      if (repo) void refresh()
    },
    { immediate: true, flush: 'sync' },
  )
  onScopeDispose(() => {
    disposed = true
    resetSession()
    diffCache.dispose()
  })

  return {
    status,
    selectedFile,
    selectedArea,
    diff,
    highlightedDiff,
    loading,
    refreshing,
    diffLoading,
    error,
    refresh,
    selectFile,
  }
}
