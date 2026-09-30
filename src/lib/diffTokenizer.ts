import { createHighlighterCore, type HighlighterCore, type ThemedToken } from 'shiki/core'
import { createJavaScriptRegexEngine } from 'shiki/engine/javascript'
import {
  canHighlight,
  MAX_SYNTAX_TOKENS,
  syntaxLanguage,
  type HighlightedDiff,
  type SyntaxKind,
  type SyntaxToken,
} from './diffSyntax'
import type { FileDiff } from './types'

// Explicit imports keep unsupported grammars and bundled themes out of the worker.
const languages = {
  typescript: () => import('shiki/langs/typescript.mjs'),
  javascript: () => import('shiki/langs/javascript.mjs'),
  tsx: () => import('shiki/langs/tsx.mjs'),
  jsx: () => import('shiki/langs/jsx.mjs'),
  vue: () => import('shiki/langs/vue.mjs'),
  html: () => import('shiki/langs/html.mjs'),
  css: () => import('shiki/langs/css.mjs'),
  scss: () => import('shiki/langs/scss.mjs'),
  json: () => import('shiki/langs/json.mjs'),
  yaml: () => import('shiki/langs/yaml.mjs'),
  php: () => import('shiki/langs/php.mjs'),
  rust: () => import('shiki/langs/rust.mjs'),
  python: () => import('shiki/langs/python.mjs'),
  shellscript: () => import('shiki/langs/shellscript.mjs'),
  sql: () => import('shiki/langs/sql.mjs'),
  markdown: () => import('shiki/langs/markdown.mjs'),
  toml: () => import('shiki/langs/toml.mjs'),
  xml: () => import('shiki/langs/xml.mjs'),
  docker: () => import('shiki/langs/docker.mjs'),
}
type Language = keyof typeof languages

// These colors encode semantic kinds only. Actual light/dark colors belong to CSS.
const markers: Record<SyntaxKind, string> = {
  plain: '#000000',
  keyword: '#000001',
  string: '#000002',
  comment: '#000003',
  number: '#000004',
  type: '#000005',
  function: '#000006',
  punctuation: '#000007',
  property: '#000008',
}
const kinds = new Map(Object.entries(markers).map(([kind, marker]) => [marker, kind as SyntaxKind]))
const scopes: Partial<Record<SyntaxKind, string[]>> = {
  keyword: ['keyword', 'storage', 'constant.language'],
  string: ['string', 'constant.character', 'markup.inline.raw', 'markup.fenced_code'],
  comment: ['comment'],
  number: ['constant.numeric'],
  type: [
    'entity.name.type',
    'entity.name.class',
    'support.type',
    'support.class',
    'entity.name.tag',
    'markup.heading',
  ],
  function: ['entity.name.function', 'support.function'],
  punctuation: ['punctuation'],
  property: [
    'variable.other.property',
    'variable.other.object.property',
    'support.type.property-name',
    'support.variable.property-name',
    'entity.other.attribute-name',
    'meta.object-literal.key',
    'support.type.property-name.json',
    'entity.name.tag.yaml',
  ],
}
let highlighterPromise: Promise<HighlighterCore> | undefined
const pendingLanguages = new Map<Language, Promise<void>>()

async function highlighterFor(language: Language) {
  highlighterPromise ??= createHighlighterCore({
    langs: [],
    themes: [
      {
        name: 'mangrove-kinds',
        settings: [
          { settings: { foreground: markers.plain, background: '#ffffff' } },
          ...Object.entries(scopes).map(([kind, scope]) => ({
            scope,
            settings: { foreground: markers[kind as SyntaxKind] },
          })),
        ],
      },
    ],
    engine: createJavaScriptRegexEngine(),
  }).catch((cause) => {
    highlighterPromise = undefined
    throw cause
  })
  const highlighter = await highlighterPromise
  if (!highlighter.getLoadedLanguages().includes(language)) {
    let pending = pendingLanguages.get(language)
    if (!pending) {
      pending = highlighter.loadLanguage(languages[language]()).finally(() => {
        pendingLanguages.delete(language)
      })
      pendingLanguages.set(language, pending)
    }
    await pending
  }
  return highlighter
}

function append(tokens: SyntaxToken[], content: string, kind: SyntaxKind) {
  if (!content) return
  const previous = tokens.at(-1)
  if (previous?.kind === kind) previous.content += content
  else tokens.push({ content, kind })
}

function tokenizeSide(highlighter: HighlighterCore, language: Language, lines: string[]) {
  if (!lines.length) return []
  const code = lines.join('\n')
  const tokens: ThemedToken[] = highlighter
    .codeToTokensBase(code, {
      lang: language,
      theme: 'mangrove-kinds',
      tokenizeMaxLineLength: 20 * 1_024,
      tokenizeTimeLimit: 500,
    })
    .flat()
  let tokenIndex = 0
  let start = 0
  return lines.map((line) => {
    const end = start + line.length
    const result: SyntaxToken[] = []
    let cursor = start
    while (tokenIndex < tokens.length) {
      const token = tokens[tokenIndex]!
      const tokenEnd = token.offset + token.content.length
      if (tokenEnd <= cursor) {
        tokenIndex++
        continue
      }
      if (token.offset >= end) break
      const from = Math.max(cursor, token.offset)
      const to = Math.min(end, tokenEnd)
      // Shiki omits line separators. Preserve any separators embedded in a
      // displayed diff line as plain text, along with every Unicode/blank byte.
      append(result, code.slice(cursor, from), 'plain')
      append(result, code.slice(from, to), kinds.get(token.color ?? '') ?? 'plain')
      cursor = to
      if (tokenEnd <= end) tokenIndex++
      else break
    }
    append(result, code.slice(cursor, end), 'plain')
    start = end + 1
    return result
  })
}

export async function tokenizeDiff(
  path: string,
  hunks: FileDiff['hunks'],
): Promise<HighlightedDiff | null> {
  const language = syntaxLanguage(path)
  if (!language || !(language in languages) || !canHighlight(hunks)) return null
  try {
    const highlighter = await highlighterFor(language as Language)
    const result: HighlightedDiff = []
    let tokenCount = 0
    for (const hunk of hunks) {
      const lines = hunk.lines.map((line) => line.content.replace(/\r?\n$/, ''))
      const old = tokenizeSide(
        highlighter,
        language as Language,
        lines.filter((_, index) => hunk.lines[index]!.kind !== 'addition'),
      )
      const current = tokenizeSide(
        highlighter,
        language as Language,
        lines.filter((_, index) => hunk.lines[index]!.kind !== 'deletion'),
      )
      let oldIndex = 0
      let currentIndex = 0
      const highlighted: SyntaxToken[][] = []
      for (const line of hunk.lines) {
        const oldTokens = line.kind === 'addition' ? undefined : old[oldIndex++]
        const currentTokens = line.kind === 'deletion' ? undefined : current[currentIndex++]
        const tokens = currentTokens ?? oldTokens ?? []
        tokenCount += tokens.length
        if (tokenCount > MAX_SYNTAX_TOKENS) return null
        highlighted.push(tokens)
      }
      result.push(highlighted)
    }
    return result
  } catch {
    // Highlighting is optional; the caller retains the original diff on failure.
    return null
  }
}
