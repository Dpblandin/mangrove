import { onScopeDispose, shallowRef, watch } from 'vue'
import { canHighlight, syntaxLanguage } from '../lib/diffSyntax'
import type { HighlightedDiff, SyntaxRequest, SyntaxResponse } from '../lib/diffSyntax'
import type { ChangedFile, FileDiff } from '../lib/types'

export type DiffHighlightWorker = Pick<
  Worker,
  'onmessage' | 'onerror' | 'onmessageerror' | 'postMessage' | 'terminate'
>
export type DiffHighlightWorkerFactory = () => DiffHighlightWorker
const TIMEOUT_MS = 2_000

function createWorker(): DiffHighlightWorker {
  return new Worker(new URL('../workers/diffHighlight.worker.ts', import.meta.url), {
    type: 'module',
  })
}

export function useDiffHighlighting(
  file: () => ChangedFile | null,
  diff: () => FileDiff | null,
  factory: DiffHighlightWorkerFactory = createWorker,
  timeoutMs = TIMEOUT_MS,
  selectionRoot?: () => HTMLElement | undefined,
) {
  const tokens = shallowRef<HighlightedDiff | null>(null)
  let worker: DiffHighlightWorker | undefined
  let active: { id: number; timer: ReturnType<typeof setTimeout> } | undefined
  let nextId = 0
  let deferredTokens: HighlightedDiff | null | undefined
  let selectionDocument: Document | undefined

  function selectionTouchesViewer() {
    const element = selectionRoot?.()
    const selection = element?.ownerDocument.getSelection()
    if (!element || !selection || selection.isCollapsed) return false
    for (let index = 0; index < selection.rangeCount; index++)
      if (selection.getRangeAt(index).intersectsNode(element)) return true
    return false
  }

  function publish(value: HighlightedDiff | null) {
    if (selectionTouchesViewer()) deferredTokens = value
    else {
      deferredTokens = undefined
      tokens.value = value
    }
  }

  function selectionChanged() {
    if (deferredTokens !== undefined && !selectionTouchesViewer()) publish(deferredTokens)
  }

  if (selectionRoot)
    watch(
      selectionRoot,
      (element) => {
        selectionDocument?.removeEventListener('selectionchange', selectionChanged)
        selectionDocument = element?.ownerDocument
        selectionDocument?.addEventListener('selectionchange', selectionChanged)
        selectionChanged()
      },
      { immediate: true, flush: 'post' },
    )

  function discardWorker() {
    if (active) clearTimeout(active.timer)
    active = undefined
    const previous = worker
    worker = undefined
    previous?.terminate()
  }

  function getWorker() {
    if (worker) return worker
    const created = factory()
    worker = created
    created.onmessage = ({ data }: MessageEvent<SyntaxResponse>) => {
      if (worker !== created || !active || data.id !== active.id) return
      clearTimeout(active.timer)
      active = undefined
      publish(data.tokens)
    }
    const failure = () => {
      if (worker === created) {
        discardWorker()
        tokens.value = null
        deferredTokens = undefined
      }
    }
    created.onerror = failure
    created.onmessageerror = failure
    return created
  }

  watch(
    [file, diff],
    ([currentFile, currentDiff]) => {
      tokens.value = null
      deferredTokens = undefined
      // A running worker is terminated instead of accumulating requests behind its parser.
      if (active) discardWorker()
      if (!currentFile || !currentDiff) {
        discardWorker()
        return
      }
      if (
        currentDiff.binary ||
        !syntaxLanguage(currentFile.path) ||
        !canHighlight(currentDiff.hunks)
      )
        return

      try {
        const target = getWorker()
        const id = ++nextId
        active = {
          id,
          timer: setTimeout(() => {
            if (active?.id === id) discardWorker()
          }, timeoutMs),
        }
        // Vue may proxy native data. Send only bounded primitive fields, never its proxies.
        const request: SyntaxRequest = {
          id,
          path: currentFile.path,
          hunks: currentDiff.hunks.map((hunk) => ({
            // Hunk context helps identify embedded script/style fragments.
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
        discardWorker()
      }
    },
    { immediate: true, flush: 'pre' },
  )

  onScopeDispose(() => {
    discardWorker()
    tokens.value = null
    deferredTokens = undefined
    selectionDocument?.removeEventListener('selectionchange', selectionChanged)
  })
  return tokens
}
