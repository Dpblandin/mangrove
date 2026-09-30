export type ThemeId = 'classic' | 'mangrove' | 'macchiato'

export const themes = [
  { id: 'classic', label: 'Classique' },
  { id: 'mangrove', label: 'Mangrove' },
  { id: 'macchiato', label: 'Catppuccin Macchiato' },
] as const satisfies readonly { id: ThemeId; label: string }[]

const storageKey = 'mangrove:theme'

export function getStoredTheme(): ThemeId {
  try {
    const saved = localStorage.getItem(storageKey)
    return themes.find((theme) => theme.id === saved)?.id ?? 'classic'
  } catch {
    return 'classic'
  }
}

export function setTheme(theme: ThemeId): void {
  document.documentElement.dataset.theme = theme
  try {
    localStorage.setItem(storageKey, theme)
  } catch {
    // An unavailable preferences store must not prevent applying the theme.
  }
}
