import { shallowRef } from 'vue'

export type RepositoryTab = { id: string; path: string | null; name: string }

function repositoryName(path: string) {
  return (
    path
      .replace(/[\\/]+$/, '')
      .split(/[\\/]/)
      .at(-1) || path
  )
}

// Tabs keep only locations. The caller reloads its single repository session on activation.
export function useRepositoryTabs() {
  const tabs = shallowRef<RepositoryTab[]>([])
  const activeId = shallowRef<string | null>(null)
  let nextId = 0

  function createNew(): RepositoryTab {
    const tab = { id: `repository-tab-${++nextId}`, path: null, name: 'Nouvel onglet' }
    tabs.value = [...tabs.value, tab]
    activeId.value = tab.id
    return tab
  }

  function open(path: string): RepositoryTab {
    const draft = tabs.value.find(
      (candidate) => candidate.id === activeId.value && candidate.path === null,
    )
    const existing = tabs.value.find((candidate) => candidate.path === path)
    if (existing) {
      if (draft) tabs.value = tabs.value.filter((candidate) => candidate.id !== draft.id)
      activeId.value = existing.id
      return existing
    }

    const tab = { id: draft?.id ?? `repository-tab-${++nextId}`, path, name: repositoryName(path) }
    tabs.value = draft
      ? tabs.value.map((candidate) => (candidate.id === draft.id ? tab : candidate))
      : [...tabs.value, tab]
    activeId.value = tab.id
    return tab
  }

  function rejectOpen(id: string, previous: RepositoryTab | undefined) {
    if (activeId.value !== id) return
    tabs.value = tabs.value.map((tab) =>
      tab.id === id ? (previous ?? { id, path: null, name: 'Nouvel onglet' }) : tab,
    )
  }

  function activate(id: string): RepositoryTab | null {
    const tab = tabs.value.find((candidate) => candidate.id === id) ?? null
    if (tab) activeId.value = tab.id
    return tab
  }

  function close(id: string) {
    const index = tabs.value.findIndex((tab) => tab.id === id)
    if (index === -1) return
    tabs.value = tabs.value.filter((tab) => tab.id !== id)
    if (activeId.value === id)
      activeId.value = tabs.value[index]?.id ?? tabs.value[index - 1]?.id ?? null
  }

  function update(id: string, info: { path: string; name: string }) {
    if (!tabs.value.some((tab) => tab.id === id)) return
    const duplicate = tabs.value.find((tab) => tab.id !== id && tab.path === info.path)
    if (duplicate) {
      tabs.value = tabs.value.filter((tab) => tab.id !== id)
      if (activeId.value === id) activeId.value = duplicate.id
      return
    }
    tabs.value = tabs.value.map((tab) =>
      tab.id === id ? { id, path: info.path, name: info.name } : tab,
    )
  }

  return { tabs, activeId, createNew, open, rejectOpen, activate, close, update }
}
