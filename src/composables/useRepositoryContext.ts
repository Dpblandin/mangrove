import { computed, onScopeDispose, ref, shallowRef, watch, type Ref } from 'vue'
import { request, type GitClient } from '../lib/api'
import { createDiffCache } from '../lib/diffCache'
import type {
  RepositoryInfo,
  WorktreeList,
  StashList,
  StashDetails,
  ChangedFile,
  FileDiff,
  StashArea,
} from '../lib/types'
import type { HighlightedDiff } from '../lib/diffSyntax'

type StashFile = ChangedFile & { area: StashArea }
type FileSnapshot = {
  file: ChangedFile | null
  area: StashArea | null
  diff: FileDiff | null
  tokens: HighlightedDiff | null
  error: string
}
const emptyFile = (): FileSnapshot => ({
  file: null,
  area: null,
  diff: null,
  tokens: null,
  error: '',
})

export function useRepositoryContext(
  repository: Readonly<Ref<RepositoryInfo | null>>,
  active: () => boolean,
  client: GitClient = request,
) {
  const worktrees = shallowRef<WorktreeList | null>(null)
  const stashes = shallowRef<StashList | null>(null)
  const worktreesLoading = ref(false)
  const stashesLoading = ref(false)
  const worktreesError = ref('')
  const stashesError = ref('')
  const details = shallowRef<StashDetails | null>(null)
  const selectedSha = ref('')
  const detailsLoading = ref(false)
  const detailsError = ref('')
  const fileLoading = ref(false)
  const file = shallowRef(emptyFile())
  const cache = createDiffCache<StashFile>((file) => `${file.area}:${file.fileKey}`)
  let epoch = 0
  let stashEpoch = 0
  let fileEpoch = 0
  let disposed = false
  let detailsController: AbortController | undefined
  let fileController: AbortController | undefined
  const controllers = new Set<AbortController>()
  const pendingLists = new Map<string, Promise<void>>()

  async function call<T>(
    command: string,
    args: Record<string, unknown>,
    controller = new AbortController(),
  ) {
    controllers.add(controller)
    try {
      return await client<T>(command, args, controller.signal)
    } finally {
      controllers.delete(controller)
    }
  }

  function clearFile() {
    fileEpoch++
    fileController?.abort()
    fileLoading.value = false
    file.value = emptyFile()
  }

  function clearStash() {
    stashEpoch++
    detailsController?.abort()
    selectedSha.value = ''
    details.value = null
    detailsLoading.value = false
    detailsError.value = ''
    clearFile()
    cache.reset(null)
  }

  function loadList(kind: 'worktrees' | 'stashes'): Promise<void> {
    const repo = repository.value
    if (!repo || disposed) return Promise.resolve()
    const pending = pendingLists.get(kind)
    if (pending) return pending
    const generation = epoch
    const loading = kind === 'worktrees' ? worktreesLoading : stashesLoading
    const error = kind === 'worktrees' ? worktreesError : stashesError
    loading.value = true
    error.value = ''
    const promise = (async () => {
      try {
        const value = await call<WorktreeList | StashList>(`list_${kind}`, { sessionId: repo.id })
        if (generation !== epoch) return
        if (kind === 'worktrees') worktrees.value = value as WorktreeList
        else {
          stashes.value = value as StashList
          if (
            selectedSha.value &&
            !stashes.value.stashes.some((stash) => stash.sha === selectedSha.value)
          )
            clearStash()
        }
      } catch (cause) {
        if (generation === epoch) error.value = String(cause)
      } finally {
        if (generation === epoch) {
          loading.value = false
          pendingLists.delete(kind)
        }
      }
    })()
    pendingLists.set(kind, promise)
    return promise
  }

  async function selectStash(sha: string) {
    const repo = repository.value
    if (!repo || disposed || !active()) return
    clearStash()
    selectedSha.value = sha
    const generation = epoch
    const selection = stashEpoch
    const controller = new AbortController()
    detailsController = controller
    detailsLoading.value = true
    try {
      const value = await call<StashDetails>(
        'stash_details',
        { sessionId: repo.id, sha },
        controller,
      )
      if (generation !== epoch || selection !== stashEpoch) return
      details.value = value
      cache.reset({
        files: value.groups.flatMap((group) =>
          group.files.map((file) => ({ ...file, area: group.area })),
        ),
        read: (file, signal) =>
          client<FileDiff>(
            'stash_diff',
            { sessionId: repo.id, sha, area: file.area, fileKey: file.fileKey },
            signal,
          ),
      })
    } catch (cause) {
      if (generation === epoch && selection === stashEpoch) detailsError.value = String(cause)
    } finally {
      if (generation === epoch && selection === stashEpoch) detailsLoading.value = false
    }
  }

  async function selectFile(area: StashArea, target: ChangedFile) {
    const group = details.value?.groups.find((group) => group.area === area)
    const valid = group?.files.find((file) => file.fileKey === target.fileKey)
    if (!valid || disposed || !active()) return
    const generation = epoch
    const selection = ++fileEpoch
    fileController?.abort()
    const controller = new AbortController()
    fileController = controller
    const sha = selectedSha.value
    fileLoading.value = true
    try {
      const prepared = await cache.read({ ...valid, area }, controller.signal)
      if (generation === epoch && selection === fileEpoch && sha === selectedSha.value)
        file.value = { file: valid, area, diff: prepared.diff, tokens: prepared.tokens, error: '' }
    } catch (cause) {
      if (generation === epoch && selection === fileEpoch)
        file.value = { file: valid, area, diff: null, tokens: null, error: String(cause) }
    } finally {
      if (generation === epoch && selection === fileEpoch) fileLoading.value = false
    }
  }

  function reset() {
    epoch++
    for (const controller of controllers) controller.abort()
    pendingLists.clear()
    worktrees.value = null
    stashes.value = null
    worktreesLoading.value = false
    stashesLoading.value = false
    worktreesError.value = ''
    stashesError.value = ''
    clearStash()
  }
  watch(() => repository.value?.id, reset, { flush: 'sync' })
  watch(
    active,
    (value) => {
      if (!value) clearStash()
    },
    { flush: 'sync' },
  )
  onScopeDispose(() => {
    disposed = true
    reset()
    cache.dispose()
  })

  return {
    worktrees,
    stashes,
    worktreesLoading,
    stashesLoading,
    worktreesError,
    stashesError,
    details,
    selectedSha,
    detailsLoading,
    detailsError,
    fileLoading,
    selectedFile: computed(() => file.value.file),
    selectedArea: computed(() => file.value.area),
    diff: computed(() => file.value.diff),
    highlightedDiff: computed(() => file.value.tokens),
    diffError: computed(() => file.value.error),
    loadWorktrees: () => loadList('worktrees'),
    loadStashes: () => loadList('stashes'),
    selectStash,
    selectFile,
    clearFile,
    clearStash,
  }
}
