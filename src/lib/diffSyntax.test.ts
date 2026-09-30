import { describe, expect, it } from 'vite-plus/test'
import { canHighlight, syntaxLanguage } from './diffSyntax'
import type { FileDiff } from './types'

function hunks(content: string, count = 1): FileDiff['hunks'] {
  return [
    {
      header: '@@ header is not source code @@',
      lines: Array.from({ length: count }, () => ({
        kind: 'context',
        oldLine: 1,
        newLine: 1,
        content,
      })),
    },
  ]
}

describe('syntax language detection', () => {
  it.each([
    ['src/main.TS', 'typescript'],
    ['src/module.mts', 'typescript'],
    ['src/module.cts', 'typescript'],
    ['src/main.js', 'javascript'],
    ['src/module.mjs', 'javascript'],
    ['src/module.cjs', 'javascript'],
    ['src/main.tsx', 'tsx'],
    ['src/main.jsx', 'jsx'],
    ['src/App.vue', 'vue'],
    ['index.html', 'html'],
    ['src/style.css', 'css'],
    ['src/style.scss', 'scss'],
    ['package.json', 'json'],
    ['.gitlab-ci.yml', 'yaml'],
    ['config.yaml', 'yaml'],
    ['src/main.php', 'php'],
    ['src/lib.rs', 'rust'],
    ['scripts/main.py', 'python'],
    ['scripts/build.sh', 'shellscript'],
    ['home/.zshrc', 'shellscript'],
    ['C:\\home\\.bash_profile', 'shellscript'],
    ['db/query.sql', 'sql'],
    ['README.md', 'markdown'],
    ['Cargo.toml', 'toml'],
    ['config.xml', 'xml'],
    ['icon.svg', 'xml'],
    ['Dockerfile', 'docker'],
    ['docker/Dockerfile.dev', 'docker'],
    ['docker/build.Dockerfile', 'docker'],
  ])('detects %s', (path, expected) => {
    expect(syntaxLanguage(path)).toBe(expected)
  })

  it('leaves unknown names and extensions plain, without matching parent directories', () => {
    for (const path of [
      'README',
      '',
      'data.txt',
      'data.unknown',
      'src.ts/no-extension',
      'Dockerfile/README',
      'data.constructor',
      'data.__proto__',
    ])
      expect(syntaxLanguage(path)).toBeNull()
  })
})

describe('syntax input limits', () => {
  it('rejects empty diffs and bounds lines across all hunks', () => {
    expect(canHighlight([])).toBe(false)
    expect(canHighlight([{ header: 'empty', lines: [] }])).toBe(false)
    expect(canHighlight(hunks('', 2_000))).toBe(true)
    expect(canHighlight([...hunks('', 1_000), ...hunks('', 1_001)])).toBe(false)
    expect(canHighlight(Array.from({ length: 2_001 }, () => ({ header: '', lines: [] })))).toBe(
      false,
    )
  })

  it('bounds individual and cumulative input before grammar loading, counting Unicode as UTF-16', () => {
    expect(canHighlight(hunks('x'.repeat(20 * 1_024)))).toBe(true)
    expect(canHighlight(hunks('x'.repeat(20 * 1_024 + 1)))).toBe(false)
    expect(canHighlight(hunks('x'.repeat(1_024), 256))).toBe(true)
    expect(canHighlight(hunks('x'.repeat(1_024), 257))).toBe(false)
    expect(canHighlight(hunks('😀'.repeat(10 * 1_024 + 1)))).toBe(false)
  })
})
