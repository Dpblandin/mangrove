import { describe, expect, it } from 'vite-plus/test'
import { MAX_SYNTAX_TOKENS, type HighlightedDiff } from './diffSyntax'
import { tokenizeDiff } from './diffTokenizer'
import type { FileDiff } from './types'

type DiffLine = FileDiff['hunks'][number]['lines'][number]
function hunk(lines: [DiffLine['kind'], string][]): FileDiff['hunks'][number] {
  return {
    header: '@@ header must stay outside the tokenizer @@',
    lines: lines.map(([kind, content], index) => ({
      kind,
      content,
      oldLine: kind === 'addition' ? null : index + 1,
      newLine: kind === 'deletion' ? null : index + 1,
    })),
  }
}

function text(tokens: HighlightedDiff) {
  return tokens.map((lines) => lines.map((line) => line.map((token) => token.content).join('')))
}

describe('diff tokenization', () => {
  it('preserves displayed Unicode, whitespace, line endings and HTML-looking content exactly', async () => {
    const hunks = [
      hunk([
        ['context', '\tconst prénom = "é😀";  \r\n'],
        ['deletion', '\tconst markup = "<script>alert(1)</script>";\n'],
        ['addition', 'const trailing = "  ";\t'],
        ['context', '\n'],
        ['context', '\r'],
        ['context', 'embedded\nnewline\n\n'],
      ]),
    ]
    const before = structuredClone(hunks)
    const tokens = await tokenizeDiff('src/main.ts', hunks)
    expect(tokens).not.toBeNull()
    expect(text(tokens!)).toEqual(
      hunks.map((block) => block.lines.map((line) => line.content.replace(/\r?\n$/, ''))),
    )
    expect(hunks).toEqual(before)
    expect(
      tokens!.flat(2).some((token) => token.kind === 'keyword' && token.content.includes('const')),
    ).toBe(true)
    expect(
      tokens!.flat(2).some((token) => token.kind === 'string' && token.content.includes('é😀')),
    ).toBe(true)
  })

  it('keeps multiline comments on the old side from contaminating additions or the displayed current context', async () => {
    const tokens = await tokenizeDiff('src/main.ts', [
      hunk([
        ['deletion', '/* old comment left open\n'],
        ['addition', 'const value = 42;\n'],
        ['context', 'const current = value;\n'],
        ['deletion', 'old comment continued\n'],
        ['addition', '// new comment\n'],
      ]),
    ])
    expect(tokens).not.toBeNull()
    expect(
      tokens![0]![3]!.some(
        (token) => token.kind === 'comment' && token.content.includes('continued'),
      ),
    ).toBe(true)
    for (const index of [1, 2]) {
      expect(
        tokens![0]![index]!.some(
          (token) => token.kind === 'keyword' && token.content.includes('const'),
        ),
      ).toBe(true)
      expect(tokens![0]![index]!.some((token) => token.kind === 'comment')).toBe(false)
    }
    expect(tokens![0]![1]!.some((token) => token.kind === 'number' && token.content === '42')).toBe(
      true,
    )
  })

  it('preserves multiline template strings separately and resets grammar state between disconnected hunks', async () => {
    const tokens = await tokenizeDiff('main.ts', [
      hunk([
        ['deletion', 'const old = `old first\n'],
        ['addition', 'const current = `new first\n'],
        ['deletion', 'old second`;\n'],
        ['addition', 'new second`;\n'],
        ['deletion', '/* unfinished old comment\n'],
      ]),
      hunk([['deletion', 'const unrelated = 5;\n']]),
    ])
    expect(tokens).not.toBeNull()
    expect(
      tokens![0]![2]!.some(
        (token) => token.kind === 'string' && token.content.includes('old second'),
      ),
    ).toBe(true)
    expect(
      tokens![0]![3]!.some(
        (token) => token.kind === 'string' && token.content.includes('new second'),
      ),
    ).toBe(true)
    expect(
      tokens![1]![0]!.some((token) => token.kind === 'keyword' && token.content.includes('const')),
    ).toBe(true)
    expect(tokens![1]![0]!.some((token) => token.kind === 'comment')).toBe(false)
  })

  it.each([
    ['main.ts', 'const value: string = "hello";'],
    ['main.js', 'function hello() { return 42; }'],
    ['main.tsx', 'const view = <Button title="hello" />;'],
    ['main.jsx', 'const view = <Button title="hello" />;'],
    ['App.vue', '<script setup lang="ts">\nconst value = "hello"\n</script>'],
    ['index.html', '<div title="hello">Content</div>'],
    ['style.css', '.button { color: red; }'],
    ['style.scss', '$color: red;\n.button { color: $color; }'],
    ['package.json', '{ "name": "hello", "count": 42 }'],
    ['config.yaml', 'name: "hello"\ncount: 42'],
    ['main.php', '<?php function hello() { return 42; }'],
    ['lib.rs', 'fn hello() -> i32 { 42 }'],
    ['main.py', 'def hello():\n    return 42'],
    ['.bashrc', 'echo "hello"'],
    ['query.sql', 'SELECT name FROM users WHERE id = 42;'],
    ['README.md', '# Title\nA `code` example'],
    ['Cargo.toml', '[package]\nname = "hello"'],
    ['config.xml', '<root name="hello">42</root>'],
    ['Dockerfile', 'FROM alpine\nRUN echo "hello"'],
    ['main.c', 'int main(void) { return 42; }'],
    ['main.hpp', 'class Example { public: int value = 42; };'],
    ['Main.cs', 'public class Example { public string Value = "hello"; }'],
    ['Main.java', 'public class Example { public int value = 42; }'],
    ['main.kt', 'val greeting: String = "hello"'],
    ['main.go', 'package main\nfunc main() { println("hello") }'],
    ['Gemfile', 'gem "rails"\n# application dependencies'],
    ['main.swift', 'let greeting: String = "hello"'],
    ['App.svelte', '<script lang="ts">\nconst value = "hello"\n</script>'],
    ['index.astro', '---\nconst value = "hello"\n---\n<div>{value}</div>'],
    ['style.less', '@color: red;\n.button { color: @color; }'],
    ['tsconfig.jsonc', '// compiler options\n{ "strict": true }'],
    ['config.json5', '{ name: "hello", count: 42 }'],
    ['query.graphql', 'query Hello { users { name } }'],
    ['.env.local', 'GREETING="hello"\n# local configuration'],
    ['settings.ini', '[settings]\nname="hello"'],
    ['messages.properties', 'greeting=hello\n# translations'],
    ['CMakeLists.txt', 'project(hello)\nset(VALUE "hello")'],
    ['Makefile', '# build the application\nall:\n\techo "hello"'],
    ['Cargo.lock', 'version = 4\n[[package]]\nname = "hello"'],
    ['composer.lock', '{ "packages": [], "content-hash": "hello" }'],
    ['changes.patch', 'diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n+hello'],
  ])(
    'loads the explicit grammar for %s and produces exact semantic tokens',
    async (path, source) => {
      const hunks = [hunk(source.split('\n').map((line) => ['addition', line + '\n']))]
      const tokens = await tokenizeDiff(path, hunks)
      expect(tokens).not.toBeNull()
      expect(text(tokens!)).toEqual([source.split('\n')])
      expect(tokens!.flat(2).some((token) => token.kind !== 'plain')).toBe(true)
    },
  )

  it('colors Vue script hunks without their opening tag, preserving both sides and every displayed byte', async () => {
    const partial = hunk([
      ['context', 'let frame = 0'],
      ['addition', 'let resolution: MediaQueryList | undefined'],
      ['addition', 'function resolutionChanged() {'],
      ['addition', "  resolution?.removeEventListener('change', resolutionChanged)"],
      ['addition', '}'],
    ])
    partial.header = '@@ -16,3 +16,7 @@ const canvas = ref<HTMLCanvasElement>()'
    const tokens = (await tokenizeDiff('GraphCanvas.vue', [partial]))!
    expect(text(tokens)).toEqual([partial.lines.map((line) => line.content)])
    const flat = tokens.flat(2)
    expect(
      flat.some((token) => token.kind === 'keyword' && token.content.includes('function')),
    ).toBe(true)
    expect(
      flat.some((token) => token.kind === 'type' && token.content.includes('MediaQueryList')),
    ).toBe(true)
    expect(flat.some((token) => token.kind === 'string' && token.content.includes('change'))).toBe(
      true,
    )
    expect(flat.some((token) => token.content.includes('<script'))).toBe(false)
  })

  it('recognizes missing embedded delimiters without contaminating disconnected template or style hunks', async () => {
    const hunks = [
      hunk([
        ['addition', '  resolutionChanged()'],
        ['context', '</script>'],
      ]),
      hunk([['addition', '<button :title="label">Save</button>']]),
      hunk([
        ['addition', '.button { color: red; }'],
        ['context', '</style>'],
      ]),
    ]
    hunks[1]!.header = '@@ -80,3 +90,3 @@ function previousScriptBlock() {'
    const tokens = (await tokenizeDiff('App.vue', hunks))!
    expect(text(tokens)).toEqual(hunks.map((block) => block.lines.map((line) => line.content)))
    for (const rows of tokens) expect(rows[0]!.some((token) => token.kind !== 'plain')).toBe(true)
    expect(
      tokens[1]![0]!.some((token) => token.kind === 'keyword' && token.content.includes('button')),
    ).toBe(true)
  })

  it('colors PHP code fragments without a <?php tag while leaving template text intact', async () => {
    const hunks = [
      hunk([['addition', 'public function hello(): string { return "hello"; }']]),
      hunk([['addition', '<div title="hello">Content</div>']]),
    ]
    const tokens = (await tokenizeDiff('Reader.php', hunks))!
    expect(text(tokens)).toEqual(hunks.map((block) => block.lines.map((line) => line.content)))
    expect(
      tokens[0]![0]!.some(
        (token) => token.kind === 'keyword' && token.content.includes('function'),
      ),
    ).toBe(true)
    expect(
      tokens[0]![0]!.some((token) => token.kind === 'string' && token.content.includes('hello')),
    ).toBe(true)
    expect(
      tokens[1]![0]!.some((token) => token.kind === 'keyword' && token.content.includes('div')),
    ).toBe(true)
  })

  it('falls back to plain text for unknown, empty or oversized input', async () => {
    expect(await tokenizeDiff('unknown.txt', [hunk([['context', 'text\n']])])).toBeNull()
    expect(await tokenizeDiff('main.ts', [])).toBeNull()
    expect(
      await tokenizeDiff('main.ts', [hunk([['addition', 'x'.repeat(20 * 1_024 + 1)]])]),
    ).toBeNull()
  })

  it('merges adjacent equal kinds and bounds the resulting spans without changing source text', async () => {
    const line = `[${Array.from({ length: 110 }, () => '1').join(',')}]\n`
    const hunks = [hunk(Array.from({ length: 100 }, () => ['addition', line]))]
    const before = structuredClone(hunks)
    expect(await tokenizeDiff('large.json', hunks)).toBeNull()
    expect(hunks).toEqual(before)
    const tokens = await tokenizeDiff('small.json', [hunk([['addition', line]])])
    expect(tokens).not.toBeNull()
    const count = tokens!.flat(2).length
    expect(count).toBeLessThan(MAX_SYNTAX_TOKENS)
    for (const block of tokens!) {
      for (const row of block) {
        expect(
          row.every((token, index) => index === 0 || token.kind !== row[index - 1]!.kind),
        ).toBe(true)
      }
    }
    expect(text(tokens!)).toEqual([[line.slice(0, -1)]])
  })
})

it('does not put newline annotations inside reconstructed code or color their words', async () => {
  const hunks = [
    hunk([
      ['deletion', 'const value = 1;'],
      ['annotation', '\\ No newline at end of file'],
      ['addition', 'const value = 2;'],
      ['annotation', '\\ No newline at end of file'],
    ]),
  ]
  const tokens = (await tokenizeDiff('a.ts', hunks))!
  expect(tokens[0]![1]).toEqual([{ content: '\\ No newline at end of file', kind: 'plain' }])
  expect(tokens[0]![3]).toEqual(tokens[0]![1])
  const without = (await tokenizeDiff('a.ts', [
    hunk([
      ['deletion', 'const value = 1;'],
      ['addition', 'const value = 2;'],
    ]),
  ]))!
  expect(tokens[0]![0]).toEqual(without[0]![0])
  expect(tokens[0]![2]).toEqual(without[0]![1])
})
