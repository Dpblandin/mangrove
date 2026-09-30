import type { FileDiff } from './types'

export type SyntaxKind =
  | 'plain'
  | 'keyword'
  | 'string'
  | 'comment'
  | 'number'
  | 'type'
  | 'function'
  | 'punctuation'
  | 'property'

export type SyntaxToken = { content: string; kind: SyntaxKind }
export type HighlightedDiff = SyntaxToken[][][]
export type SyntaxRequest = { id: number; path: string; hunks: FileDiff['hunks'] }
export type SyntaxResponse = { id: number; tokens: HighlightedDiff | null }

/** Maximum rendered spans after adjacent tokens of the same kind are merged. */
export const MAX_SYNTAX_TOKENS = 20_000

const extensions: Record<string, string> = {
  ts: 'typescript',
  mts: 'typescript',
  cts: 'typescript',
  js: 'javascript',
  mjs: 'javascript',
  cjs: 'javascript',
  tsx: 'tsx',
  jsx: 'jsx',
  vue: 'vue',
  html: 'html',
  htm: 'html',
  css: 'css',
  scss: 'scss',
  json: 'json',
  yaml: 'yaml',
  yml: 'yaml',
  php: 'php',
  phtml: 'php',
  rs: 'rust',
  py: 'python',
  pyi: 'python',
  sh: 'shellscript',
  bash: 'shellscript',
  zsh: 'shellscript',
  sql: 'sql',
  md: 'markdown',
  markdown: 'markdown',
  toml: 'toml',
  xml: 'xml',
  svg: 'xml',
}
const shellNames = new Set([
  '.bashrc',
  '.bash_profile',
  '.bash_login',
  '.bash_logout',
  '.zshrc',
  '.zprofile',
  '.zshenv',
  '.zlogin',
  '.zlogout',
  '.profile',
])

export function syntaxLanguage(path: string): string | null {
  const name = path.split(/[\\/]/).at(-1)?.toLowerCase() ?? ''
  if (name === 'dockerfile' || name.startsWith('dockerfile.') || name.endsWith('.dockerfile'))
    return 'docker'
  if (shellNames.has(name)) return 'shellscript'
  const separator = name.lastIndexOf('.')
  const extension = name.slice(separator + 1)
  return separator < 0 || !Object.hasOwn(extensions, extension)
    ? null
    : (extensions[extension] ?? null)
}

/** Bounds count UTF-16 units (JS string length), before any grammar is loaded. */
export function canHighlight(hunks: FileDiff['hunks']): boolean {
  if (hunks.length > 2_000) return false
  let lines = 0
  let characters = 0
  for (const hunk of hunks) {
    for (const line of hunk.lines) {
      lines += 1
      characters += line.content.length
      if (lines > 2_000 || characters > 256 * 1_024 || line.content.length > 20 * 1_024)
        return false
    }
  }
  return lines > 0
}
