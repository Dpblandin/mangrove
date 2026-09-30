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
  svelte: () => import('shiki/langs/svelte.mjs'),
  astro: () => import('shiki/langs/astro.mjs'),
  html: () => import('shiki/langs/html.mjs'),
  css: () => import('shiki/langs/css.mjs'),
  scss: () => import('shiki/langs/scss.mjs'),
  less: () => import('shiki/langs/less.mjs'),
  json: () => import('shiki/langs/json.mjs'),
  jsonc: () => import('shiki/langs/jsonc.mjs'),
  json5: () => import('shiki/langs/json5.mjs'),
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
  c: () => import('shiki/langs/c.mjs'),
  cpp: () => import('shiki/langs/cpp.mjs'),
  csharp: () => import('shiki/langs/csharp.mjs'),
  java: () => import('shiki/langs/java.mjs'),
  kotlin: () => import('shiki/langs/kotlin.mjs'),
  go: () => import('shiki/langs/go.mjs'),
  ruby: () => import('shiki/langs/ruby.mjs'),
  swift: () => import('shiki/langs/swift.mjs'),
  graphql: () => import('shiki/langs/graphql.mjs'),
  ini: () => import('shiki/langs/ini.mjs'),
  properties: () => import('shiki/langs/properties.mjs'),
  cmake: () => import('shiki/langs/cmake.mjs'),
  make: () => import('shiki/langs/make.mjs'),
  dotenv: () => import('shiki/langs/dotenv.mjs'),
  diff: () => import('shiki/langs/diff.mjs'),
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
  keyword: [
    'keyword',
    'storage',
    'constant.language',
    'entity.other.attribute-name',
    'entity.name.tag',
  ],
  string: [
    'string',
    'constant.character',
    'markup.inline.raw',
    'markup.fenced_code',
    'punctuation.definition.string',
  ],
  comment: ['comment', 'punctuation.definition.comment'],
  number: ['constant.numeric'],
  type: [
    'entity.name.type',
    'entity.name.class',
    'support.type',
    'support.class',
    'markup.heading',
  ],
  function: ['entity.name.function', 'support.function'],
  punctuation: ['punctuation'],
  property: [
    'variable.other.property',
    'variable.other.object.property',
    'support.type.property-name',
    'support.variable.property-name',
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

// A patch often starts inside an embedded block, without its opening delimiter.
// Seed only recognizable fragments; never insert that delimiter into displayed tokens.
function fragmentPrefix(language: Language, lines: string[], header: string) {
  if (!['vue', 'svelte', 'astro'].includes(language)) return ''
  const code = lines.join('\n')
  const context = header.replace(/^@@.*?@@\s*/, '')
  const script = [context, ...lines].some((line) =>
    /^\s*(?:(?:import|export|const|let|var|function|class|interface|type|return|async|if|for|while|throw)\b|[\w$][\w$.?]*\s*(?:\(|=(?!=)))/.test(
      line,
    ),
  )
  const block = code.match(/^[\t ]*<(\/?)(script|style|template)\b[^\n>]*>/im)
  if (block && block[1] !== '/') return ''
  if (block?.[2]?.toLowerCase() === 'template') return ''
  if (!block && /^\s*<\/?[a-z][\w:-]*(?:\s|>|\/)/i.test(code)) return ''
  const style =
    block?.[2]?.toLowerCase() === 'style' ||
    (!script && /^\s*(?:[.#&][^{\n]+\{|[\w-]+\s*:\s*[^{}\n]+;)/m.test(code))
  if (style) return '<style lang="scss">\n'
  if (script || block?.[2]?.toLowerCase() === 'script')
    return language === 'astro' ? '---\n' : '<script lang="ts">\n'
  return ''
}

function tokenizeSide(
  highlighter: HighlighterCore,
  language: Language,
  lines: string[],
  header: string,
) {
  if (!lines.length) return []
  // PHP's grammar starts in code. Its bundled HTML grammar handles a separate
  // template fragment whose opening PHP block is outside the patch.
  const fragmentLanguage =
    language === 'php' && /^\s*<\/?[a-z!]/i.test(lines.join('\n')) ? 'html' : language
  const prefix = fragmentPrefix(language, lines, header)
  const code = prefix + lines.join('\n')
  const tokens: ThemedToken[] = highlighter
    .codeToTokensBase(code, {
      lang: fragmentLanguage,
      theme: 'mangrove-kinds',
      tokenizeMaxLineLength: 20 * 1_024,
      tokenizeTimeLimit: 500,
    })
    .flat()
  let tokenIndex = 0
  let start = prefix.length
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
        lines.filter((_, index) => ['context', 'deletion'].includes(hunk.lines[index]!.kind)),
        hunk.header,
      )
      const current = tokenizeSide(
        highlighter,
        language as Language,
        lines.filter((_, index) => ['context', 'addition'].includes(hunk.lines[index]!.kind)),
        hunk.header,
      )
      let oldIndex = 0
      let currentIndex = 0
      const highlighted: SyntaxToken[][] = []
      for (const line of hunk.lines) {
        if (line.kind === 'annotation') {
          highlighted.push([{ content: line.content, kind: 'plain' }])
          tokenCount++
          if (tokenCount > MAX_SYNTAX_TOKENS) return null
          continue
        }
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
