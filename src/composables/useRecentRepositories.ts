import { onScopeDispose, shallowRef } from 'vue'
import { request, type GitClient } from '../lib/api'

type RecentRepository = { path: string; name: string }
const storageKey = 'mangrove:recent'
const limit = 6

export function useRecentRepositories(client: GitClient = request) {
  const entries = shallowRef<RecentRepository[]>([])
  let disposed = false
  let pending: Promise<void> | undefined
  let controller: AbortController | undefined

  try {
    const saved: unknown = JSON.parse(localStorage.getItem(storageKey) ?? '[]')
    if (Array.isArray(saved)) {
      const paths = new Set<string>()
      for (const entry of saved) {
        if (
          typeof entry !== 'object' ||
          entry === null ||
          typeof entry.path !== 'string' ||
          typeof entry.name !== 'string' ||
          paths.has(entry.path)
        )
          continue
        entries.value.push({ path: entry.path, name: entry.name })
        paths.add(entry.path)
        if (entries.value.length === limit) break
      }
    }
  } catch {
    /* Preferences are optional. */
  }

  function save() {
    try {
      localStorage.setItem(storageKey, JSON.stringify(entries.value))
    } catch {
      /* Preferences are optional. */
    }
  }

  function remember(entry: RecentRepository) {
    entries.value = [
      { path: entry.path, name: entry.name },
      ...entries.value.filter((item) => item.path !== entry.path),
    ].slice(0, limit)
    save()
  }

  function remove(path: string) {
    const remaining = entries.value.filter((entry) => entry.path !== path)
    if (remaining.length === entries.value.length) return
    entries.value = remaining
    save()
  }

  function updateName(updated: RecentRepository) {
    const current = entries.value.find((entry) => entry.path === updated.path)
    if (!current) return
    entries.value = entries.value.map((entry) =>
      entry === current ? { path: updated.path, name: updated.name } : entry,
    )
    if (current.name !== updated.name) save()
  }

  function refreshNames(): Promise<void> {
    if (pending) return pending
    if (disposed || !entries.value.length) return Promise.resolve()
    const original = new Map(entries.value.map((entry) => [entry.path, entry]))
    controller = new AbortController()
    pending = (async () => {
      try {
        const names = await client<{ path: string; name: string | null }[]>(
          'repository_names',
          { paths: [...original.keys()] },
          controller?.signal,
        )
        if (disposed) return
        const resolved = new Map(names.map((entry) => [entry.path, entry.name]))
        let changed = false
        const updated = entries.value.map((entry) => {
          const name = resolved.get(entry.path)
          // A newer opening or rename replaces the entry, even if its name is unchanged.
          if (!name || entry !== original.get(entry.path) || name === entry.name) return entry
          changed = true
          return { path: entry.path, name }
        })
        if (changed) {
          entries.value = updated
          save()
        }
      } catch {
        /* Missing repositories and unavailable metadata leave stored names usable. */
      } finally {
        pending = undefined
      }
    })()
    return pending
  }

  onScopeDispose(() => {
    disposed = true
    controller?.abort()
  })
  return { entries, remember, remove, updateName, refreshNames }
}
