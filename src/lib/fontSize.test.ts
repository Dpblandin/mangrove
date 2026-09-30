import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test'
import {
  DEFAULT_FONT_SIZE_OFFSET,
  DEFAULT_DIFF_FONT_SIZE,
  MAX_DIFF_FONT_SIZE,
  MIN_DIFF_FONT_SIZE,
  MAX_FONT_SIZE_OFFSET,
  MIN_FONT_SIZE_OFFSET,
  fontSizeOffset,
  diffFontSize,
  getStoredDiffFontSize,
  getStoredFontSizeOffset,
  handleFontSizeShortcut,
  initializeFontSize,
  setFontSizeOffset,
  setDiffFontSize,
} from './fontSize'

let preferences: Map<string, string>
let styles: Map<string, string>
const key = 'mangrove:font-size-offset'
const diffKey = 'mangrove:diff-font-size'

function event(overrides: Partial<KeyboardEvent>) {
  return {
    key: '',
    code: '',
    metaKey: false,
    ctrlKey: true,
    altKey: false,
    defaultPrevented: false,
    preventDefault: vi.fn(),
    ...overrides,
  } as unknown as KeyboardEvent & { preventDefault: (this: void) => void }
}

beforeEach(() => {
  preferences = new Map()
  styles = new Map()
  vi.stubGlobal('localStorage', {
    getItem: (name: string) => preferences.get(name) ?? null,
    setItem: (name: string, value: string) => preferences.set(name, value),
  })
  vi.stubGlobal('document', {
    documentElement: {
      style: { setProperty: (name: string, value: string) => styles.set(name, value) },
    },
  })
  setFontSizeOffset(DEFAULT_FONT_SIZE_OFFSET)
  setDiffFontSize(DEFAULT_DIFF_FONT_SIZE)
  preferences.clear()
})
afterEach(() => vi.unstubAllGlobals())

describe('font size preferences', () => {
  it('applies the larger default and restores a persisted size before rendering', () => {
    initializeFontSize()
    expect(styles.get('--font-size-offset')).toBe('1px')
    setFontSizeOffset(3)
    expect(preferences.get(key)).toBe('3')
    setFontSizeOffset(1)
    preferences.set(key, '3')
    initializeFontSize()
    expect(fontSizeOffset.value).toBe(3)
    expect(styles.get('--font-size-offset')).toBe('3px')
  })

  it.each(['', ' ', 'invalid', 'Infinity', 'NaN'])(
    'uses the default for invalid stored data %j',
    (saved) => {
      preferences.set(key, saved)
      preferences.set(diffKey, saved)
      expect(getStoredFontSizeOffset()).toBe(DEFAULT_FONT_SIZE_OFFSET)
      expect(getStoredDiffFontSize()).toBe(DEFAULT_DIFF_FONT_SIZE)
    },
  )

  it('bounds both stored and applied sizes and rounds fractional preferences', () => {
    preferences.set(key, '200')
    expect(getStoredFontSizeOffset()).toBe(MAX_FONT_SIZE_OFFSET)
    expect(setFontSizeOffset(-200)).toBe(MIN_FONT_SIZE_OFFSET)
    expect(styles.get('--font-size-offset')).toBe('0px')
    expect(setFontSizeOffset(2.7)).toBe(3)
    expect(setFontSizeOffset(Number.NaN)).toBe(DEFAULT_FONT_SIZE_OFFSET)
  })

  it('still applies sizes and shortcuts when storage is blocked', () => {
    const blocked = () => {
      throw new Error('Preferences unavailable')
    }
    vi.stubGlobal('localStorage', { getItem: blocked, setItem: blocked })
    expect(getStoredFontSizeOffset()).toBe(DEFAULT_FONT_SIZE_OFFSET)
    expect(getStoredDiffFontSize()).toBe(DEFAULT_DIFF_FONT_SIZE)
    expect(() => initializeFontSize()).not.toThrow()
    expect(() => setDiffFontSize(12)).not.toThrow()
    expect(styles.get('--diff-font-size')).toBe('12px')
    expect(handleFontSizeShortcut(event({ key: '+' }))).toBe(true)
    expect(styles.get('--font-size-offset')).toBe('2px')
  })
})

describe('diff font size preferences', () => {
  it('starts smaller and restores the diff size independently of the interface size', () => {
    initializeFontSize()
    expect(diffFontSize.value).toBe(11)
    expect(styles.get('--diff-font-size')).toBe('11px')
    setDiffFontSize(14)
    expect(fontSizeOffset.value).toBe(DEFAULT_FONT_SIZE_OFFSET)
    expect(preferences.get(key)).toBe(String(DEFAULT_FONT_SIZE_OFFSET))
    expect(preferences.get(diffKey)).toBe('14')
    setDiffFontSize(DEFAULT_DIFF_FONT_SIZE)
    preferences.set(diffKey, '14')
    preferences.set(key, '3')
    initializeFontSize()
    expect(diffFontSize.value).toBe(14)
    expect(styles.get('--diff-font-size')).toBe('14px')
    expect(fontSizeOffset.value).toBe(3)
  })

  it('keeps stored or requested diff sizes within usable bounds', () => {
    preferences.set(diffKey, '200')
    initializeFontSize()
    expect(diffFontSize.value).toBe(MAX_DIFF_FONT_SIZE)
    expect(setDiffFontSize(-200)).toBe(MIN_DIFF_FONT_SIZE)
    expect(setDiffFontSize(12.7)).toBe(13)
    expect(setDiffFontSize(Number.NaN)).toBe(DEFAULT_DIFF_FONT_SIZE)
    expect(fontSizeOffset.value).toBe(DEFAULT_FONT_SIZE_OFFSET)
  })

  it('keeps global shortcuts working on both sizes after a custom diff adjustment', () => {
    setDiffFontSize(14)
    handleFontSizeShortcut(event({ key: '+', metaKey: true, ctrlKey: false }))
    expect(fontSizeOffset.value).toBe(2)
    expect(diffFontSize.value).toBe(15)
    handleFontSizeShortcut(event({ key: '-', code: 'Equal', metaKey: true, ctrlKey: false }))
    expect(fontSizeOffset.value).toBe(1)
    expect(diffFontSize.value).toBe(14)
    handleFontSizeShortcut(event({ key: '0' }))
    expect(fontSizeOffset.value).toBe(DEFAULT_FONT_SIZE_OFFSET)
    expect(diffFontSize.value).toBe(DEFAULT_DIFF_FONT_SIZE)
  })
})

describe('font size keyboard shortcuts', () => {
  it.each([
    { key: '-', code: 'Equal', expected: 2 },
    { key: '+', code: 'Minus', expected: 4 },
    { key: '=', code: 'Minus', expected: 4 },
    { key: '0', code: 'Equal', expected: DEFAULT_FONT_SIZE_OFFSET },
  ])('uses the typed symbol before the physical key: $key on $code', (keys) => {
    setFontSizeOffset(3)
    const shortcut = event({ ...keys, metaKey: true, ctrlKey: false })
    expect(handleFontSizeShortcut(shortcut)).toBe(true)
    expect(shortcut.preventDefault).toHaveBeenCalledOnce()
    expect(fontSizeOffset.value).toBe(keys.expected)
  })

  it.each([
    { key: '=', code: 'Equal', ctrlKey: true, metaKey: false },
    { key: '+', code: 'Equal', ctrlKey: false, metaKey: true },
    { key: '+', code: 'NumpadAdd', ctrlKey: true, metaKey: false },
  ])('increases text size and suppresses browser zoom for $code', (keys) => {
    const shortcut = event(keys)
    expect(handleFontSizeShortcut(shortcut)).toBe(true)
    expect(shortcut.preventDefault).toHaveBeenCalledOnce()
    expect(fontSizeOffset.value).toBe(2)
  })

  it('decreases, clamps repeated shortcuts and resets to the larger default', () => {
    setFontSizeOffset(MAX_FONT_SIZE_OFFSET)
    expect(handleFontSizeShortcut(event({ key: '+' }))).toBe(true)
    expect(fontSizeOffset.value).toBe(MAX_FONT_SIZE_OFFSET)
    handleFontSizeShortcut(event({ key: '-', code: 'NumpadSubtract' }))
    expect(fontSizeOffset.value).toBe(MAX_FONT_SIZE_OFFSET - 1)
    setFontSizeOffset(MIN_FONT_SIZE_OFFSET)
    const smaller = event({ key: '-', code: 'Minus' })
    expect(handleFontSizeShortcut(smaller)).toBe(true)
    expect(smaller.preventDefault).toHaveBeenCalledOnce()
    expect(fontSizeOffset.value).toBe(MIN_FONT_SIZE_OFFSET)
    handleFontSizeShortcut(event({ key: '0', code: 'Digit0' }))
    expect(fontSizeOffset.value).toBe(DEFAULT_FONT_SIZE_OFFSET)
  })

  it.each([
    { key: '+', ctrlKey: false, metaKey: false },
    { key: '+', altKey: true },
    { key: '+', defaultPrevented: true },
    { key: 'k', code: 'KeyK' },
    { key: 'Insert', code: 'Numpad0' },
  ])('leaves typing and other shortcuts alone: %j', (keys) => {
    const shortcut = event(keys)
    expect(handleFontSizeShortcut(shortcut)).toBe(false)
    expect(shortcut.preventDefault).not.toHaveBeenCalled()
    expect(fontSizeOffset.value).toBe(DEFAULT_FONT_SIZE_OFFSET)
  })
})
