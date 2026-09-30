import { describe, expect, it } from 'vite-plus/test'
import { fileStatusLabel } from './format'
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
