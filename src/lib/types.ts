export interface Reference {
  name: string
  label: string
  kind: 'local' | 'remote' | 'tag' | 'other'
  sha: string
}

export interface IndexProgress {
  indexed: number
  complete: boolean
  error: string | null
}

export interface RepositoryInfo {
  id: string
  name: string
  path: string
  head: string | null
  headName: string
  references: Reference[]
  remotes: string[]
  index: IndexProgress
}

export interface GraphEdge {
  from: number
  to: number
  color: number
  kind: 'through' | 'parent' | 'incoming'
}

export interface GraphRow {
  lane: number
  color: number
  width: number
  edges: GraphEdge[]
}

export interface CommitRow {
  sha: string
  summary: string
  author: string
  email: string
  time: number
  parents: string[]
  position: number
  graph: GraphRow
}

export interface CommitPage {
  commits: CommitRow[]
  total: number
  offset: number
  index: IndexProgress
}

export type FileStatus =
  | 'added'
  | 'deleted'
  | 'modified'
  | 'renamed'
  | 'copied'
  | 'typechange'
  | 'unmodified'
  | 'unknown'

export interface ChangedFile {
  index: number
  path: string
  oldPath: string | null
  status: FileStatus
}

export interface CommitDetails {
  sha: string
  message: string
  author: string
  email: string
  time: number
  parents: string[]
  files: ChangedFile[]
  fileCount: number
}

export interface FileDiff {
  binary: boolean
  truncated: boolean
  additions: number
  deletions: number
  hunks: {
    header: string
    lines: {
      kind: 'addition' | 'deletion' | 'context'
      oldLine: number | null
      newLine: number | null
      content: string
    }[]
  }[]
}
