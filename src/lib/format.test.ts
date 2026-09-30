import { describe, expect, it } from 'vite-plus/test'
import { fileStatusLabel, commitDate, commitDateTime, initials, shortDate } from './format'
import type { ChangedFile, FileStatus } from './types'

describe('file statuses from the native Git contract', () => {
  const cases: [FileStatus, string][] = [
    ['added', 'Ajouté'],
    ['deleted', 'Supprimé'],
    ['renamed', 'Renommé'],
    ['typechange', 'Type modifié'],
    ['unknown', 'Statut inconnu'],
  ]

  it.each(cases)('presents the %s status without reducing it to an initial', (status, label) => {
    const file: ChangedFile = {
      index: 0,
      path: 'src/history.ts',
      oldPath: status === 'renamed' ? 'src/old-history.ts' : null,
      status,
    }
    expect(fileStatusLabel(file.status)).toBe(label)
  })
})

describe('commit metadata formatting', () => {
  it.each([9_999_999_999_999, -9_999_999_999_999, NaN, Infinity])(
    'keeps an invalid timestamp %s renderable without a datetime attribute',
    (timestamp) => {
      expect(shortDate(timestamp)).toBe('Date inconnue')
      expect(commitDate(timestamp)).toBe('Date inconnue')
      expect(commitDateTime(timestamp)).toBeUndefined()
    },
  )
  it('retains normal dates and supplementary Unicode initials', () => {
    expect(commitDateTime(0)).toBe('1970-01-01T00:00:00.000Z')
    expect(shortDate(0)).not.toBe('Date inconnue')
    expect(initials('😀 Bob')).toBe('😀B')
    expect(initials('𝓐lice Martin')).toBe('𝓐M')
  })
})
