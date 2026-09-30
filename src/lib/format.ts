import type { FileStatus } from './types'

const date = new Intl.DateTimeFormat('fr-FR', { day: '2-digit', month: 'short' })
const fullDate = new Intl.DateTimeFormat('fr-FR', { dateStyle: 'long', timeStyle: 'short' })
const number = new Intl.NumberFormat('fr-FR')

export const count = (value: number) => number.format(value)
export const shortDate = (timestamp: number) => date.format(new Date(timestamp * 1000))
export const commitDate = (timestamp: number) => fullDate.format(new Date(timestamp * 1000))
export const shortSha = (sha: string) => sha.slice(0, 7)
export function initials(name: string) {
  return name
    .trim()
    .split(/\s+/)
    .slice(0, 2)
    .map((part) => part[0] ?? '')
    .join('')
    .toUpperCase()
}
export function filename(path: string) {
  return path.split('/').at(-1) ?? path
}
const fileStatusLabels: Record<FileStatus, string> = {
  added: 'Ajouté',
  deleted: 'Supprimé',
  modified: 'Modifié',
  renamed: 'Renommé',
  copied: 'Copié',
  typechange: 'Type modifié',
  unmodified: 'Inchangé',
  unknown: 'Statut inconnu',
}
export function fileStatusLabel(status: FileStatus) {
  return fileStatusLabels[status] ?? fileStatusLabels.unknown
}
export function provider(remotes: string[]) {
  if (remotes.some((url) => /github[.:/]/i.test(url))) return 'GitHub'
  if (remotes.some((url) => /gitlab[.:/]/i.test(url))) return 'GitLab'
  if (remotes.some((url) => /bitbucket[.:/]/i.test(url))) return 'Bitbucket'
  return 'Dépôt local'
}
