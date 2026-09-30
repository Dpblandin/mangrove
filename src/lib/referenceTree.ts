import type { Reference } from './types'

export type ReferenceNode =
  | { kind: 'reference'; path: string; label: string; reference: Reference }
  | { kind: 'folder'; path: string; label: string; count: number; children: ReferenceNode[] }

export interface ReferenceTreeRow {
  node: ReferenceNode
  depth: number
}

interface Folder {
  path: string
  label: string
  count: number
  folders: Map<string, Folder>
  references: ReferenceNode[]
}

const ordering = new Intl.Collator('fr', { numeric: true, sensitivity: 'base' })

export function buildReferenceTree(references: readonly Reference[]): ReferenceNode[] {
  const root: Folder = { path: '', label: '', count: 0, folders: new Map(), references: [] }
  for (const reference of references) {
    const segments = reference.label.split('/')
    let parent = root
    for (const label of segments.slice(0, -1)) {
      let folder = parent.folders.get(label)
      if (!folder) {
        folder = {
          path: parent.path ? `${parent.path}/${label}` : label,
          label,
          count: 0,
          folders: new Map(),
          references: [],
        }
        parent.folders.set(label, folder)
      }
      folder.count++
      parent = folder
    }
    parent.references.push({
      kind: 'reference',
      path: reference.name,
      label: segments.at(-1)!,
      reference,
    })
  }

  function children(folder: Folder): ReferenceNode[] {
    return [
      ...Array.from(folder.folders.values(), (entry): ReferenceNode => ({
        kind: 'folder',
        path: entry.path,
        label: entry.label,
        count: entry.count,
        children: children(entry),
      })),
      ...folder.references,
    ].sort((a, b) => ordering.compare(a.label, b.label))
  }
  return children(root)
}

export function visibleReferenceRows(
  tree: readonly ReferenceNode[],
  collapsed: ReadonlySet<string>,
): ReferenceTreeRow[] {
  const rows: ReferenceTreeRow[] = []
  function visit(nodes: readonly ReferenceNode[], depth: number) {
    for (const node of nodes) {
      rows.push({ node, depth })
      if (node.kind === 'folder' && !collapsed.has(node.path)) visit(node.children, depth + 1)
    }
  }
  visit(tree, 0)
  return rows
}
