import { afterEach, describe, expect, it, vi } from 'vite-plus/test'
import { getStoredTheme, setTheme } from './themes'

afterEach(() => vi.unstubAllGlobals())

describe('theme preferences', () => {
  it('defaults to the classic theme for an unsupported stored preference', () => {
    vi.stubGlobal('localStorage', { getItem: () => 'obsolete-theme' })
    expect(getStoredTheme()).toBe('classic')
  })

  it('applies a theme immediately and restores its persisted preference', () => {
    const preferences = new Map<string, string>()
    const dataset: Record<string, string> = {}
    vi.stubGlobal('document', { documentElement: { dataset } })
    vi.stubGlobal('localStorage', {
      getItem: (key: string) => preferences.get(key) ?? null,
      setItem: (key: string, value: string) => preferences.set(key, value),
    })
    expect(getStoredTheme()).toBe('classic')
    setTheme('macchiato')
    expect(dataset.theme).toBe('macchiato')
    expect(getStoredTheme()).toBe('macchiato')
  })

  it('keeps theme application working when preference storage is blocked', () => {
    const dataset: Record<string, string> = {}
    const blocked = () => {
      throw new Error('Preferences unavailable')
    }
    vi.stubGlobal('document', { documentElement: { dataset } })
    vi.stubGlobal('localStorage', { getItem: blocked, setItem: blocked })
    expect(getStoredTheme()).toBe('classic')
    expect(() => setTheme('mangrove')).not.toThrow()
    expect(dataset.theme).toBe('mangrove')
  })
})
