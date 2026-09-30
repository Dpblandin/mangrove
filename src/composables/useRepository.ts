import { onScopeDispose, ref, shallowRef, watch } from 'vue'
import { request, type GitClient } from '../lib/api'
import type {
  ChangedFile,
  CommitDetails,
  CommitPage,
  CommitRow,
  FileDiff,
  IndexProgress,
  RepositoryInfo,
} from '../lib/types'

export const PAGE_SIZE = 128
const PAGE_CACHE_SIZE = 12

type PendingPage = { controller: AbortController; promise: Promise<void> }

export function useRepository(client: GitClient = request) {
  const repository = shallowRef<RepositoryInfo | null>(null)
  const progress = ref<IndexProgress>({ indexed: 0, complete: false, error: null })
  const pages = shallowRef(new Map<number, CommitRow[]>())
  const total = ref(0)
  const query = ref('')
  const filter = ref('')
  const opening = ref(false)
  const loadingPages = ref(0)
  const error = ref('')
  const selectedSha = ref('')
  const selectedIndex = ref(0)
  const details = shallowRef<CommitDetails | null>(null)
  const detailLoading = ref(false)
  const detailError = ref('')
  const selectedFile = shallowRef<ChangedFile | null>(null)
  const diff = shallowRef<FileDiff | null>(null)
  const diffLoading = ref(false)
  const diffError = ref('')
  let epoch = 0
  let pageEpoch = 0
  let selectionEpoch = 0
  let diffEpoch = 0
  let navigationEpoch = 0
  let navigating = false
  let requestedIndex: number | null = null
  let desiredQuery = ''
  let settingQuery = false
  let searchIndexed = 0
  let debounce: ReturnType<typeof setTimeout> | undefined
  let poll: ReturnType<typeof setTimeout> | undefined
  let detailController: AbortController | undefined
  let diffController: AbortController | undefined
  let navigationController: AbortController | undefined
  const pending = new Map<number, PendingPage>()
  const controllers = new Set<AbortController>()

  async function call<T>(
    command: string,
    args: Record<string, unknown>,
    controller = new AbortController(),
  ): Promise<T> {
    controllers.add(controller)
    try {
      return await client<T>(command, args, controller.signal)
    } finally {
      controllers.delete(controller)
    }
  }

  function cancelNavigation() {
    navigationEpoch += 1
    navigating = false
    requestedIndex = null
    navigationController?.abort()
  }

  function clearSelection() {
    selectionEpoch += 1
    diffEpoch += 1
    detailController?.abort()
    diffController?.abort()
    selectedSha.value = ''
    selectedIndex.value = 0
    details.value = null
    selectedFile.value = null
    diff.value = null
    detailLoading.value = false
    diffLoading.value = false
    detailError.value = ''
    diffError.value = ''
  }

  function mergeProgress(status: IndexProgress) {
    progress.value = {
      indexed: Math.max(progress.value.indexed, status.indexed),
      complete: progress.value.complete || status.complete,
      error: status.error ?? progress.value.error,
    }
  }

  function getRow(index: number): CommitRow | undefined {
    return pages.value.get(Math.floor(index / PAGE_SIZE) * PAGE_SIZE)?.[index % PAGE_SIZE]
  }

  function pageNeedsRefresh(offset: number, rows: CommitRow[]) {
    return (
      (Boolean(filter.value) && searchIndexed < progress.value.indexed) ||
      (rows.length < PAGE_SIZE && offset + rows.length < total.value)
    )
  }

  function revalidatePages(except?: number) {
    return Promise.all(
      [...pages.value]
        .filter(([offset, rows]) => offset !== except && pageNeedsRefresh(offset, rows))
        .map(([offset]) => loadPage(offset, true)),
    )
  }

  function loadRange(start: number, end: number) {
    const last = Math.min(end, total.value - 1)
    for (
      let offset = Math.floor(Math.max(0, start) / PAGE_SIZE) * PAGE_SIZE;
      offset <= last;
      offset += PAGE_SIZE
    )
      void loadPage(offset)
  }

  function loadPage(offset: number, force = false): Promise<void> {
    const repo = repository.value
    if (!repo || debounce !== undefined) return Promise.resolve()
    const existing = pending.get(offset)
    if (existing) return existing.promise
    const cached = pages.value.get(offset)
    if (!force && cached && !pageNeedsRefresh(offset, cached)) return Promise.resolve()
    const generation = pageEpoch
    const controller = new AbortController()
    const entry: PendingPage = { controller, promise: Promise.resolve() }
    pending.set(offset, entry)
    loadingPages.value = pending.size
    entry.promise = (async () => {
      try {
        let refresh: boolean
        do {
          const page = await client<CommitPage>(
            'list_commits',
            {
              sessionId: repo.id,
              offset,
              limit: PAGE_SIZE,
              query: filter.value,
            },
            controller.signal,
          )
          if (generation !== pageEpoch || repository.value?.id !== repo.id) return
          if (filter.value) searchIndexed = Math.max(searchIndexed, page.index.indexed)
          const next = new Map(pages.value)
          next.delete(offset)
          next.set(offset, page.commits)
          while (next.size > PAGE_CACHE_SIZE) next.delete(next.keys().next().value!)
          pages.value = next
          mergeProgress(page.index)
          // The walk only appends within this view. An older SQLite snapshot must
          // neither shrink its result count nor leave a newly incomplete tail cached.
          total.value = Math.max(total.value, page.total, filter.value ? 0 : progress.value.indexed)
          if (page.index.error) error.value = page.index.error
          if (!selectedSha.value && !navigating && offset === 0 && page.commits[0])
            void select(page.commits[0], 0)
          refresh = pageNeedsRefresh(offset, page.commits)
          if (!refresh) void revalidatePages(offset)
        } while (refresh)
      } catch (cause) {
        if (generation === pageEpoch && !controller.signal.aborted) error.value = String(cause)
      } finally {
        if (pending.get(offset) === entry) {
          pending.delete(offset)
          loadingPages.value = pending.size
        }
      }
    })()
    return entry.promise
  }

  function resetPages() {
    pageEpoch += 1
    for (const { controller } of pending.values()) controller.abort()
    pending.clear()
    pages.value = new Map()
    searchIndexed = 0
    loadingPages.value = 0
  }

  async function readProgress(generation: number) {
    const repo = repository.value
    if (!repo || generation !== epoch) return
    try {
      const status = await call<IndexProgress>('index_status', { sessionId: repo.id })
      if (generation !== epoch) return
      mergeProgress(status)
      if (!filter.value && debounce === undefined) total.value = progress.value.indexed
      if (debounce === undefined) {
        await loadPage(0)
        if (generation !== epoch) return
        await revalidatePages()
      }
      if (generation !== epoch) return
      if (status.error) error.value = status.error
      if (!progress.value.complete && !progress.value.error)
        poll = setTimeout(() => void readProgress(generation), 350)
    } catch (cause) {
      if (generation === epoch) error.value = String(cause)
    }
  }

  function clearQuery() {
    clearTimeout(debounce)
    debounce = undefined
    desiredQuery = ''
    settingQuery = true
    query.value = ''
    settingQuery = false
    filter.value = ''
  }

  async function open(path: string) {
    const generation = ++epoch
    clearTimeout(poll)
    clearQuery()
    for (const controller of controllers) controller.abort()
    cancelNavigation()
    resetPages()
    clearSelection()
    repository.value = null
    progress.value = { indexed: 0, complete: false, error: null }
    total.value = 0
    error.value = ''
    opening.value = true
    try {
      const info = await call<RepositoryInfo>('open_repository', { path })
      if (generation !== epoch) return false
      repository.value = info
      progress.value = info.index
      total.value = info.index.indexed
      await loadPage(0)
      if (generation !== epoch) return false
      if (!progress.value.complete) void readProgress(generation)
      return true
    } catch (cause) {
      if (generation === epoch) error.value = String(cause)
      return false
    } finally {
      if (generation === epoch) opening.value = false
    }
  }

  async function select(row: CommitRow, index: number) {
    const repo = repository.value
    if (!repo) return
    cancelNavigation()
    clearSelection()
    const generation = selectionEpoch
    const controller = new AbortController()
    detailController = controller
    selectedSha.value = row.sha
    selectedIndex.value = index
    detailLoading.value = true
    try {
      const value = await call<CommitDetails>(
        'commit_details',
        { sessionId: repo.id, sha: row.sha },
        controller,
      )
      if (generation !== selectionEpoch || repo.id !== repository.value?.id) return
      details.value = value
      if (value.files[0]) void selectFile(value.files[0])
    } catch (cause) {
      if (generation === selectionEpoch && !controller.signal.aborted)
        detailError.value = String(cause)
    } finally {
      if (generation === selectionEpoch) detailLoading.value = false
    }
  }

  async function selectFile(file: ChangedFile) {
    const repo = repository.value
    const sha = selectedSha.value
    if (!repo || !sha) return
    const generation = ++diffEpoch
    diffController?.abort()
    const controller = new AbortController()
    diffController = controller
    selectedFile.value = file
    diff.value = null
    diffError.value = ''
    diffLoading.value = true
    try {
      const value = await call<FileDiff>(
        'file_diff',
        { sessionId: repo.id, sha, fileIndex: file.index },
        controller,
      )
      if (generation === diffEpoch && repo.id === repository.value?.id) diff.value = value
    } catch (cause) {
      if (generation === diffEpoch && !controller.signal.aborted) diffError.value = String(cause)
    } finally {
      if (generation === diffEpoch) diffLoading.value = false
    }
  }

  async function loadSelection(index: number, generation: number, repositoryId: string) {
    await loadPage(Math.floor(index / PAGE_SIZE) * PAGE_SIZE)
    if (generation !== navigationEpoch || repositoryId !== repository.value?.id) return null
    const row = getRow(index)
    if (!row) return null
    void select(row, index)
    return index
  }

  async function selectIndex(index: number): Promise<number | null> {
    const repo = repository.value
    if (!repo || total.value === 0 || debounce !== undefined) return null
    cancelNavigation()
    navigating = true
    const target = Math.max(0, Math.min(total.value - 1, index))
    requestedIndex = target
    const generation = navigationEpoch
    try {
      return await loadSelection(target, generation, repo.id)
    } finally {
      if (generation === navigationEpoch) {
        navigating = false
        requestedIndex = null
      }
    }
  }

  function moveSelection(direction: -1 | 1) {
    const current =
      requestedIndex ?? (selectedSha.value ? selectedIndex.value : direction === 1 ? -1 : 0)
    return selectIndex(current + direction)
  }

  async function jump(sha: string): Promise<number | null> {
    const repo = repository.value
    if (!repo) return null
    cancelNavigation()
    clearQuery()
    resetPages()
    clearSelection()
    total.value = progress.value.indexed
    error.value = ''
    navigating = true
    const generation = navigationEpoch
    const controller = new AbortController()
    navigationController = controller
    try {
      const position = await call<number | null>(
        'commit_position',
        { sessionId: repo.id, sha },
        controller,
      )
      if (generation !== navigationEpoch || repo.id !== repository.value?.id) return null
      if (position === null) {
        error.value = 'Cette référence n’est pas encore indexée.'
        navigating = false
        await loadPage(0)
        return null
      }
      return await loadSelection(position, generation, repo.id)
    } catch (cause) {
      if (generation === navigationEpoch && !controller.signal.aborted) error.value = String(cause)
      return null
    } finally {
      if (generation === navigationEpoch) navigating = false
    }
  }

  watch(
    query,
    () => {
      if (settingQuery) return
      const next = query.value.trim().slice(0, 256)
      if (next === desiredQuery) return
      desiredQuery = next
      clearTimeout(debounce)
      debounce = undefined
      cancelNavigation()
      resetPages()
      clearSelection()
      total.value = 0
      error.value = ''
      debounce = setTimeout(() => {
        debounce = undefined
        filter.value = next
        void loadPage(0)
      }, 160)
    },
    { flush: 'sync' },
  )

  onScopeDispose(() => {
    epoch += 1
    cancelNavigation()
    resetPages()
    clearSelection()
    clearTimeout(poll)
    clearTimeout(debounce)
    for (const controller of controllers) controller.abort()
  })

  return {
    repository,
    progress,
    pages,
    total,
    query,
    filter,
    opening,
    loadingPages,
    error,
    selectedSha,
    selectedIndex,
    details,
    detailLoading,
    detailError,
    selectedFile,
    diff,
    diffLoading,
    diffError,
    getRow,
    loadPage,
    loadRange,
    open,
    select,
    selectIndex,
    moveSelection,
    selectFile,
    jump,
  }
}
