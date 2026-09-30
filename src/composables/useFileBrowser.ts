import { onScopeDispose, ref, shallowRef, watch } from 'vue'
import { request, type GitClient } from '../lib/api'
import type {
  ChangedFile,
  CommitDetails,
  FileBlame,
  FileContent,
  FileDiff,
  FileHistoryEntry,
  FileHistoryPage,
} from '../lib/types'

export type FileView = 'diff' | 'file' | 'history' | 'blame'
export interface FileSelection {
  sessionId: string
  sha: string
  file: ChangedFile
  initialDiff?: FileDiff | null
}
// Closing and reopening a workspace must not free a still-running native read.
const nativeReads = new WeakMap<GitClient, Promise<unknown>>()

/** Only the current revision and a single history page are retained, never historical blobs. */
export function useFileBrowser(source: () => FileSelection | null, client: GitClient = request) {
  const target = shallowRef<FileSelection | null>(null)
  const mode = ref<FileView>('diff')
  const content = shallowRef<FileContent | null>(null)
  const history = shallowRef<FileHistoryPage | null>(null)
  const blame = shallowRef<FileBlame | null>(null)
  const diff = shallowRef<FileDiff | null>(null)
  const inspectedCommit = shallowRef<CommitDetails | null>(null)
  const loading = ref(false)
  const error = ref('')
  const inspecting = ref(false)
  const inspectionError = ref('')
  const previousCursors = shallowRef<(string | null)[]>([])
  let cursor: string | null = null
  let revision = 0
  let controller: AbortController | undefined
  let inspectionController: AbortController | undefined
  let inspectionSha: string | undefined
  type HistoryRequest = { cursor: string | null; direction: -1 | 1 }
  let failedPage: HistoryRequest | undefined

  async function read<T>(command: string, args: Record<string, unknown>, signal: AbortSignal) {
    // Tauri cannot interrupt an invoke already running. Keep its slot until it settles;
    // canceled view requests waiting for that slot never start another Git read.
    let active = nativeReads.get(client)
    while (active) {
      try {
        await active
      } catch {
        // The caller that owns this read reports its error.
      }
      if (signal.aborted) throw new DOMException('Requête annulée', 'AbortError')
      active = nativeReads.get(client)
    }
    if (signal.aborted) throw new DOMException('Requête annulée', 'AbortError')
    const pending = client<T>(command, args, signal)
    nativeReads.set(client, pending)
    try {
      return await pending
    } finally {
      if (nativeReads.get(client) === pending) nativeReads.delete(client)
    }
  }

  function closeInspection() {
    inspectionController?.abort()
    inspectionController = undefined
    inspectionSha = undefined
    inspectedCommit.value = null
    inspecting.value = false
    inspectionError.value = ''
  }

  function cancel() {
    revision++
    controller?.abort()
    inspectionController?.abort()
    controller = undefined
    inspectionController = undefined
    inspectionSha = undefined
    loading.value = false
    inspecting.value = false
  }

  function select(value: FileSelection | null) {
    cancel()
    target.value = value
    content.value = null
    history.value = null
    blame.value = null
    diff.value = value?.initialDiff ?? null
    inspectedCommit.value = null
    error.value = ''
    inspectionError.value = ''
    previousCursors.value = []
    cursor = null
    failedPage = undefined
  }

  async function load(historyRequest?: HistoryRequest) {
    const selection = target.value
    if (!selection?.file.fileKey) return
    const view = mode.value
    if (
      (view === 'file' && content.value) ||
      (view === 'blame' && blame.value) ||
      (view === 'history' && history.value && !historyRequest) ||
      (view === 'diff' && diff.value)
    )
      return
    controller?.abort()
    const attempt = ++revision
    const pending = new AbortController()
    controller = pending
    loading.value = true
    error.value = ''
    const args = {
      sessionId: selection.sessionId,
      sha: selection.sha,
      fileKey: selection.file.fileKey,
    }
    const current = () => attempt === revision && !pending.signal.aborted
    try {
      if ((view === 'file' || view === 'blame') && !content.value) {
        const value = await read<FileContent>('file_content', args, pending.signal)
        if (!current()) return
        content.value = value
      }
      if (view === 'blame' && content.value?.kind === 'text' && content.value.lines.length) {
        const value = await read<FileBlame>('file_blame', args, pending.signal)
        if (current()) blame.value = value
      } else if (view === 'history') {
        const value = await read<FileHistoryPage>(
          'file_history',
          { ...args, cursor: historyRequest ? historyRequest.cursor : cursor },
          pending.signal,
        )
        if (current()) {
          if (historyRequest) {
            previousCursors.value =
              historyRequest.direction === 1
                ? [...previousCursors.value, cursor].slice(-12)
                : previousCursors.value.slice(0, -1)
            cursor = historyRequest.cursor
          }
          history.value = value
          failedPage = undefined
        }
      } else if (view === 'diff') {
        const value = await read<FileDiff>('file_revision_diff', args, pending.signal)
        if (current()) diff.value = value
      }
    } catch (cause) {
      if (current()) {
        error.value = String(cause)
        failedPage = historyRequest
      }
    } finally {
      if (current()) {
        loading.value = false
        controller = undefined
      }
    }
  }

  async function show(view: FileView) {
    if (view === mode.value && loading.value) return
    const retryPage = view === mode.value ? failedPage : undefined
    cancel()
    mode.value = view
    error.value = ''
    failedPage = undefined
    if (view !== 'blame') closeInspection()
    await load(retryPage)
  }

  async function selectRevision(
    entry: Pick<FileHistoryEntry, 'sha' | 'path' | 'fileKey' | 'status' | 'oldPath'>,
    view: FileView = 'diff',
  ) {
    const sessionId = target.value?.sessionId
    if (!sessionId) return
    select({
      sessionId,
      sha: entry.sha,
      file: {
        index: -1,
        path: entry.path,
        fileKey: entry.fileKey,
        oldPath: entry.oldPath,
        status: entry.status,
      },
    })
    await show(view)
  }

  async function restoreSelection() {
    const view = mode.value
    select(source())
    await show(view)
  }

  async function page(direction: -1 | 1) {
    if (mode.value !== 'history' || loading.value) return
    const next = direction === 1 ? history.value?.nextCursor : previousCursors.value.at(-1)
    if (next === undefined || (direction === 1 && !next)) return
    cancel()
    failedPage = undefined
    await load({ cursor: next ?? null, direction })
  }

  async function inspectCommit(sha: string) {
    const selection = target.value
    if (!selection || inspectedCommit.value?.sha === sha || inspectionSha === sha) return
    inspectionController?.abort()
    const pending = new AbortController()
    inspectionController = pending
    inspectionSha = sha
    inspecting.value = true
    inspectionError.value = ''
    inspectedCommit.value = null
    try {
      const value = await read<CommitDetails>(
        'commit_details',
        { sessionId: selection.sessionId, sha },
        pending.signal,
      )
      if (!pending.signal.aborted && target.value === selection) inspectedCommit.value = value
    } catch (cause) {
      if (!pending.signal.aborted && target.value === selection)
        inspectionError.value = String(cause)
    } finally {
      if (inspectionController === pending) {
        inspecting.value = false
        inspectionController = undefined
        inspectionSha = undefined
      }
    }
  }

  watch(
    source,
    (value) => {
      select(value)
      mode.value = 'diff'
    },
    { immediate: true, flush: 'sync' },
  )
  onScopeDispose(() => {
    select(null)
  })
  return {
    target,
    mode,
    content,
    history,
    blame,
    diff,
    loading,
    error,
    previousCursors,
    inspectedCommit,
    inspecting,
    inspectionError,
    show,
    page,
    selectRevision,
    restoreSelection,
    inspectCommit,
    closeInspection,
  }
}
