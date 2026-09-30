import { ArrowUpRight, CircleHelp, Copy, Equal, Minus, Pencil, Plus, Replace } from '@lucide/vue'
import type { Component } from 'vue'
import type { FileStatus } from './types'

const presentations = {
  added: { icon: Plus, colorClass: 'text-success' },
  deleted: { icon: Minus, colorClass: 'text-danger' },
  modified: { icon: Pencil, colorClass: 'text-warning' },
  renamed: { icon: ArrowUpRight, colorClass: 'text-renamed' },
  copied: { icon: Copy, colorClass: 'text-warning' },
  typechange: { icon: Replace, colorClass: 'text-warning' },
  unmodified: { icon: Equal, colorClass: 'text-muted' },
  unknown: { icon: CircleHelp, colorClass: 'text-muted' },
} satisfies Record<FileStatus, { icon: Component; colorClass: string }>

export function fileStatusPresentation(status: FileStatus) {
  return presentations[status] ?? presentations.unknown
}
