import { describe, expect, it } from 'vite-plus/test'
import { useRepositoryTabs } from './useRepositoryTabs'

describe('repository tabs', () => {
  it('creates distinct welcome tabs without changing open repository locations', () => {
    const registry = useRepositoryTabs()
    const repository = registry.open('/projects/mangrove')
    const first = registry.createNew()
    const second = registry.createNew()

    expect(first).toEqual({ id: first.id, path: null, name: 'Nouvel onglet' })
    expect(second).toEqual({ id: second.id, path: null, name: 'Nouvel onglet' })
    expect(second.id).not.toBe(first.id)
    expect(registry.tabs.value).toEqual([repository, first, second])
    expect(registry.activeId.value).toBe(second.id)
  })

  it('binds a new repository to the active welcome tab without replacing its identity', () => {
    const registry = useRepositoryTabs()
    const repository = registry.open('/projects/other')
    const backgroundDraft = registry.createNew()
    const draft = registry.createNew()
    const bound = registry.open('/projects/mangrove/')

    expect(bound).toEqual({ id: draft.id, path: '/projects/mangrove/', name: 'mangrove' })
    expect(registry.tabs.value).toEqual([repository, backgroundDraft, bound])
    expect(registry.activeId.value).toBe(draft.id)
    registry.update(bound.id, { path: '/projects/mangrove', name: 'Mangrove' })
    expect(registry.tabs.value.at(-1)).toEqual({
      id: draft.id,
      path: '/projects/mangrove',
      name: 'Mangrove',
    })
  })

  it('selects an existing repository and removes only the active welcome tab', () => {
    const registry = useRepositoryTabs()
    const existing = registry.open('/projects/mangrove')
    const other = registry.open('/projects/other')
    const backgroundDraft = registry.createNew()
    const draft = registry.createNew()

    expect(registry.open('/projects/mangrove')).toBe(existing)
    expect(registry.tabs.value).toEqual([existing, other, backgroundDraft])
    expect(registry.activeId.value).toBe(existing.id)
    registry.update(draft.id, { path: '/projects/late', name: 'Late' })
    expect(registry.tabs.value).toEqual([existing, other, backgroundDraft])
  })

  it('leaves a background welcome tab intact when opening from a repository tab', () => {
    const registry = useRepositoryTabs()
    const first = registry.open('/first')
    const draft = registry.createNew()
    registry.activate(first.id)
    const second = registry.open('/second')

    expect(second.id).not.toBe(draft.id)
    expect(registry.tabs.value).toEqual([first, draft, second])
    expect(registry.activeId.value).toBe(second.id)
  })

  it('closes welcome tabs with the same active-neighbor rules as repository tabs', () => {
    const registry = useRepositoryTabs()
    const draft = registry.createNew()
    const repository = registry.open('/projects/mangrove')
    const inactiveDraft = registry.createNew()
    const activeDraft = registry.createNew()
    registry.activate(repository.id)
    registry.close(inactiveDraft.id)
    expect(registry.activeId.value).toBe(repository.id)
    registry.activate(activeDraft.id)
    registry.close(activeDraft.id)
    expect(registry.activeId.value).toBe(repository.id)
    registry.close(repository.id)
    expect(registry.activeId.value).toBeNull()
    expect(registry.tabs.value).toEqual([])
    registry.update(draft.id, { path: '/late', name: 'Late' })
    expect(registry.tabs.value).toEqual([])
  })

  it('reuses an existing path and keeps ids stable when tabs close and reopen', () => {
    const registry = useRepositoryTabs()
    const first = registry.open('/projects/mangrove')
    const second = registry.open('/projects/easy-care')
    expect(registry.open('/projects/mangrove')).toBe(first)
    expect(registry.tabs.value).toEqual([first, second])
    expect(registry.activeId.value).toBe(first.id)

    registry.close(first.id)
    const reopened = registry.open('/projects/mangrove')
    expect(reopened.id).not.toBe(first.id)
    expect(registry.tabs.value[0]?.id).toBe(second.id)
    expect(reopened.name).toBe('mangrove')
    expect(Object.keys(reopened).sort()).toEqual(['id', 'name', 'path'])
  })

  it('chooses the next tab, then the previous tab, then no tab on active closure', () => {
    const registry = useRepositoryTabs()
    const first = registry.open('/a')
    const middle = registry.open('/b')
    const last = registry.open('/c')

    registry.activate(middle.id)
    registry.close(middle.id)
    expect(registry.activeId.value).toBe(last.id)
    registry.close(last.id)
    expect(registry.activeId.value).toBe(first.id)
    registry.close(first.id)
    expect(registry.tabs.value).toEqual([])
    expect(registry.activeId.value).toBeNull()
  })

  it('keeps the current selection when closing another tab or receiving an unknown id', () => {
    const registry = useRepositoryTabs()
    const first = registry.open('/a')
    const active = registry.open('/b')
    registry.close(first.id)
    registry.close('missing')
    expect(registry.activate('missing')).toBeNull()
    expect(registry.activeId.value).toBe(active.id)
    expect(registry.tabs.value).toEqual([active])
  })

  it('canonicalizes metadata without changing the identity or activation', () => {
    const registry = useRepositoryTabs()
    const tab = registry.open('/projects/link/')
    registry.update(tab.id, { path: '/projects/mangrove', name: 'Mangrove' })
    expect(registry.tabs.value).toEqual([
      { id: tab.id, path: '/projects/mangrove', name: 'Mangrove' },
    ])
    expect(registry.activeId.value).toBe(tab.id)
    expect(registry.open('/projects/mangrove').id).toBe(tab.id)
  })

  it('keeps tab identity and only locations when given a native repository response', () => {
    const registry = useRepositoryTabs()
    const tab = registry.open('/projects/mangrove')
    const nativeInfo = {
      id: 'native-session-id',
      path: '/projects/mangrove',
      name: 'mangrove',
      references: [{ name: 'refs/heads/main', sha: 'head' }],
      index: { indexed: 10000, complete: true, error: null },
    }
    registry.update(tab.id, nativeInfo)
    expect(registry.tabs.value).toEqual([
      { id: tab.id, path: nativeInfo.path, name: nativeInfo.name },
    ])
    expect(registry.activeId.value).toBe(tab.id)
    expect(registry.activate(tab.id)?.id).toBe(tab.id)
  })

  it('merges an active alias into the existing canonical tab', () => {
    const registry = useRepositoryTabs()
    const existing = registry.open('/projects/mangrove')
    const alias = registry.open('/projects/link')
    registry.update(alias.id, { path: '/projects/mangrove', name: 'mangrove' })
    expect(registry.tabs.value).toEqual([existing])
    expect(registry.activeId.value).toBe(existing.id)
  })

  it('does not steal focus when a background alias resolves to another tab', () => {
    const registry = useRepositoryTabs()
    const existing = registry.open('/projects/mangrove')
    const alias = registry.open('/projects/link')
    const active = registry.open('/projects/other')
    registry.update(alias.id, { path: '/projects/mangrove', name: existing.name })
    expect(registry.tabs.value).toEqual([existing, active])
    expect(registry.activeId.value).toBe(active.id)
  })

  it('ignores a late opening result after closure, even when the path is reopened', () => {
    const registry = useRepositoryTabs()
    const closed = registry.open('/projects/mangrove')
    registry.close(closed.id)
    const current = registry.open('/projects/mangrove')
    registry.update(closed.id, { path: '/projects/old-result', name: 'Old result' })
    expect(registry.tabs.value).toEqual([current])
    expect(registry.activeId.value).toBe(current.id)
  })

  it('derives provisional names from both platform separators and root paths', () => {
    const registry = useRepositoryTabs()
    expect(registry.open('/projects/mangrove/').name).toBe('mangrove')
    expect(registry.open('C:\\projects\\mangrove\\').name).toBe('mangrove')
    expect(registry.open('/').name).toBe('/')
  })
})
