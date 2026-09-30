import { computed, onScopeDispose, ref, shallowRef, watch, type Ref } from 'vue'

export type Pane = 'left' | 'right'
type Preferences = Record<Pane, number>
type PreferenceStore = Pick<Storage, 'getItem' | 'setItem'>

const STORAGE_KEY = 'mangrove:panes'
const DEFAULTS: Preferences = { left: 226, right: 390 }
const MINIMUMS: Preferences = { left: 180, right: 280 }
const LEFT_MAXIMUM = 400
const HISTORY_MINIMUM = 340
const DIVIDERS_WIDTH = 12
const APP_MINIMUM = 880

function clamp(value: number, minimum: number, maximum: number) {
  return Math.max(minimum, Math.min(maximum, Math.round(value)))
}

function browserStore(): PreferenceStore | null {
  try {
    return typeof window === 'undefined' ? null : window.localStorage
  } catch {
    return null
  }
}

function storedPreferences(store: PreferenceStore | null): Preferences {
  const result = { ...DEFAULTS }
  try {
    const saved: unknown = JSON.parse(store?.getItem(STORAGE_KEY) ?? 'null')
    if (typeof saved === 'object' && saved !== null) {
      for (const pane of ['left', 'right'] as const) {
        const value = (saved as Record<string, unknown>)[pane]
        if (typeof value === 'number' && Number.isFinite(value))
          result[pane] =
            pane === 'left'
              ? clamp(value, MINIMUMS.left, LEFT_MAXIMUM)
              : Math.max(MINIMUMS.right, Math.round(value))
      }
    }
  } catch {
    // An unavailable or damaged preference store must not prevent opening the app.
  }
  return result
}

export function usePaneLayout(
  root: Ref<HTMLElement | undefined>,
  store: PreferenceStore | null = browserStore(),
) {
  const preferred = shallowRef(storedPreferences(store))
  const available = ref(
    typeof window === 'undefined' ? 1440 : Math.max(APP_MINIMUM, window.innerWidth),
  )
  const drag = shallowRef<{ pane: Pane; before: Preferences } | null>(null)
  const resizing = computed(() => drag.value !== null)
  const leftBounds = computed(() => ({
    min: MINIMUMS.left,
    max: Math.min(
      LEFT_MAXIMUM,
      available.value - MINIMUMS.right - HISTORY_MINIMUM - DIVIDERS_WIDTH,
    ),
  }))
  const leftWidth = computed(() =>
    clamp(preferred.value.left, leftBounds.value.min, leftBounds.value.max),
  )
  const rightBounds = computed(() => ({
    min: MINIMUMS.right,
    max: available.value - leftWidth.value - HISTORY_MINIMUM - DIVIDERS_WIDTH,
  }))
  const rightWidth = computed(() =>
    clamp(preferred.value.right, rightBounds.value.min, rightBounds.value.max),
  )

  function persist() {
    try {
      store?.setItem(STORAGE_KEY, JSON.stringify(preferred.value))
    } catch {
      // Layout remains usable when persistence is unavailable.
    }
  }

  function resize(pane: Pane, value: number) {
    if (!Number.isFinite(value)) return
    const bounds = pane === 'left' ? leftBounds.value : rightBounds.value
    preferred.value = { ...preferred.value, [pane]: clamp(value, bounds.min, bounds.max) }
    if (!drag.value) persist()
  }

  function startResize(pane: Pane) {
    if (drag.value) finishResize(true)
    drag.value = { pane, before: { ...preferred.value } }
  }

  function finishResize(cancelled = false) {
    const active = drag.value
    if (!active) return
    if (cancelled) preferred.value = active.before
    drag.value = null
    if (!cancelled) persist()
  }

  function reset(pane: Pane) {
    preferred.value = { ...preferred.value, [pane]: DEFAULTS[pane] }
    if (!drag.value) persist()
  }

  let observer: ResizeObserver | undefined
  watch(
    root,
    (element) => {
      observer?.disconnect()
      observer = undefined
      if (!element) return
      const measure = (width: number) => {
        if (Number.isFinite(width) && width > 0) available.value = Math.max(APP_MINIMUM, width)
      }
      measure(element.getBoundingClientRect().width)
      observer = new ResizeObserver((entries) => {
        const entry = entries[0]
        if (entry) measure(entry.contentRect.width)
      })
      observer.observe(element)
    },
    { flush: 'post', immediate: true },
  )
  onScopeDispose(() => observer?.disconnect())

  return {
    leftWidth,
    rightWidth,
    leftBounds,
    rightBounds,
    resizing,
    resize,
    reset,
    startResize,
    finishResize,
  }
}
