import { describe, expect, it } from 'vite-plus/test'
import { buildReferenceTree, visibleReferenceRows } from './referenceTree'
import type { Reference } from './types'

function branch(label: string, kind: Reference['kind'] = 'local'): Reference {
  return {
    name: `refs/${kind === 'remote' ? 'remotes' : 'heads'}/${label}`,
    label,
    kind,
    sha: label,
  }
}

describe('reference folders', () => {
  const references = [
    branch('main'),
    branch('feature/api/auth'),
    branch('feature/api/search'),
    branch('feature/ui'),
    branch('backup/main'),
  ]

  it('groups nested branch names while retaining complete reference identities', () => {
    const rows = visibleReferenceRows(buildReferenceTree(references), new Set())
    expect(rows.map(({ node, depth }) => [node.label, depth])).toEqual([
      ['backup', 0],
      ['main', 1],
      ['feature', 0],
      ['api', 1],
      ['auth', 2],
      ['search', 2],
      ['ui', 1],
      ['main', 0],
    ])
    const leaves = rows.flatMap(({ node }) =>
      node.kind === 'reference' ? [node.reference.name] : [],
    )
    expect(leaves).toContain('refs/heads/main')
    expect(leaves).toContain('refs/heads/backup/main')
    const feature = rows.find(({ node }) => node.path === 'feature')!.node
    expect(feature.kind === 'folder' && feature.count).toBe(3)
  })

  it('folds one subtree without hiding branches in other folders', () => {
    const tree = buildReferenceTree(references)
    const rows = visibleReferenceRows(tree, new Set(['feature/api']))
    expect(rows.map(({ node }) => node.label)).toEqual([
      'backup',
      'main',
      'feature',
      'api',
      'ui',
      'main',
    ])
    expect(visibleReferenceRows(tree, new Set(['feature'])).map(({ node }) => node.label)).toEqual([
      'backup',
      'main',
      'feature',
      'main',
    ])
    expect(
      visibleReferenceRows(tree, new Set()).filter(({ node }) => node.kind === 'reference'),
    ).toHaveLength(5)
  })

  it('keeps remote and branch folder paths nested and independently foldable', () => {
    const tree = buildReferenceTree([
      branch('origin/feature/api', 'remote'),
      branch('origin/main', 'remote'),
      branch('upstream/main', 'remote'),
    ])
    expect(visibleReferenceRows(tree, new Set(['origin'])).map(({ node }) => node.label)).toEqual([
      'origin',
      'upstream',
      'main',
    ])
    expect(
      visibleReferenceRows(tree, new Set(['origin/feature'])).map(({ node }) => node.label),
    ).toEqual(['origin', 'feature', 'main', 'upstream', 'main'])
  })

  it('shows matching branches under their full ancestry when filtering references', () => {
    const matches = references.filter((reference) => reference.label.includes('api/auth'))
    const rows = visibleReferenceRows(buildReferenceTree(matches), new Set())
    expect(rows.map(({ node }) => node.label)).toEqual(['feature', 'api', 'auth'])
    expect(rows.flatMap(({ node }) => (node.kind === 'folder' ? [node.count] : []))).toEqual([1, 1])
  })
})
