import { readonly, ref } from 'vue'

export const DEFAULT_FONT_SIZE_OFFSET = 1
export const MIN_FONT_SIZE_OFFSET = 0
export const MAX_FONT_SIZE_OFFSET = 5
export const DEFAULT_DIFF_FONT_SIZE = 11
export const MIN_DIFF_FONT_SIZE = 9
export const MAX_DIFF_FONT_SIZE = 20
const storageKey = 'mangrove:font-size-offset'
const diffStorageKey = 'mangrove:diff-font-size'
const offset = ref(DEFAULT_FONT_SIZE_OFFSET)
const diffSize = ref(DEFAULT_DIFF_FONT_SIZE)
export const fontSizeOffset = readonly(offset)
export const diffFontSize = readonly(diffSize)

function bounded(value: number, min: number, max: number, fallback: number): number {
  return Number.isFinite(value) ? Math.max(min, Math.min(max, Math.round(value))) : fallback
}

function storedSize(key: string, min: number, max: number, fallback: number): number {
  try {
    const saved = localStorage.getItem(key)
    return saved === null || saved.trim() === ''
      ? fallback
      : bounded(Number(saved), min, max, fallback)
  } catch {
    return fallback
  }
}

function persistSize(key: string, value: number): void {
  try {
    localStorage.setItem(key, String(value))
  } catch {
    // Applying a readable size must still work when preferences cannot be saved.
  }
}

export function getStoredFontSizeOffset(): number {
  return storedSize(
    storageKey,
    MIN_FONT_SIZE_OFFSET,
    MAX_FONT_SIZE_OFFSET,
    DEFAULT_FONT_SIZE_OFFSET,
  )
}

export function setFontSizeOffset(value: number): number {
  offset.value = bounded(
    value,
    MIN_FONT_SIZE_OFFSET,
    MAX_FONT_SIZE_OFFSET,
    DEFAULT_FONT_SIZE_OFFSET,
  )
  document.documentElement.style.setProperty('--font-size-offset', `${offset.value}px`)
  persistSize(storageKey, offset.value)
  return offset.value
}

export function getStoredDiffFontSize(): number {
  return storedSize(diffStorageKey, MIN_DIFF_FONT_SIZE, MAX_DIFF_FONT_SIZE, DEFAULT_DIFF_FONT_SIZE)
}

export function setDiffFontSize(value: number): number {
  diffSize.value = bounded(value, MIN_DIFF_FONT_SIZE, MAX_DIFF_FONT_SIZE, DEFAULT_DIFF_FONT_SIZE)
  document.documentElement.style.setProperty('--diff-font-size', `${diffSize.value}px`)
  persistSize(diffStorageKey, diffSize.value)
  return diffSize.value
}

export function initializeFontSize(): void {
  setFontSizeOffset(getStoredFontSizeOffset())
  setDiffFontSize(getStoredDiffFontSize())
}

export function handleFontSizeShortcut(event: KeyboardEvent): boolean {
  if (event.defaultPrevented || (!event.metaKey && !event.ctrlKey) || event.altKey) return false
  // On French Mac keyboards, '-' can come from the physical Equal key.
  const shortcut = ['+', '=', '-', '0'].includes(event.key) ? event.key : event.code
  let next: number
  if (shortcut === '+' || shortcut === '=' || shortcut === 'Equal' || shortcut === 'NumpadAdd')
    next = offset.value + 1
  else if (shortcut === '-' || shortcut === 'Minus' || shortcut === 'NumpadSubtract')
    next = offset.value - 1
  else if (shortcut === '0' || shortcut === 'Digit0') next = DEFAULT_FONT_SIZE_OFFSET
  else return false
  event.preventDefault()
  const previous = offset.value
  const applied = setFontSizeOffset(next)
  setDiffFontSize(
    shortcut === '0' || shortcut === 'Digit0'
      ? DEFAULT_DIFF_FONT_SIZE
      : diffSize.value + applied - previous,
  )
  return true
}
