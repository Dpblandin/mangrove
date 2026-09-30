export const DEFAULT_AUTO_FETCH_INTERVAL = 1
export const MAX_AUTO_FETCH_INTERVAL = 60
const storageKey = 'mangrove:auto-fetch-interval'

export function validAutoFetchInterval(value: number): boolean {
  return Number.isInteger(value) && value >= 0 && value <= MAX_AUTO_FETCH_INTERVAL
}

export function parseAutoFetchInterval(value: string | number | null): number | null {
  if (value === null || String(value).trim() === '') return null
  const interval = Number(value)
  return validAutoFetchInterval(interval) ? interval : null
}

export function getStoredAutoFetchInterval(): number {
  try {
    return parseAutoFetchInterval(localStorage.getItem(storageKey)) ?? DEFAULT_AUTO_FETCH_INTERVAL
  } catch {
    return DEFAULT_AUTO_FETCH_INTERVAL
  }
}

export function storeAutoFetchInterval(value: number): void {
  if (!validAutoFetchInterval(value)) return
  try {
    localStorage.setItem(storageKey, String(value))
  } catch {
    // The preference still applies when storage is unavailable.
  }
}
