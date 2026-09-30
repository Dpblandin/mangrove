<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, useId, watch } from 'vue'
import { useVirtualizer } from '@tanstack/vue-virtual'
import { useDiffHighlighting } from '../composables/useDiffHighlighting'
import { diffFontSize } from '../lib/fontSize'
import { commitDate, shortSha } from '../lib/format'
import type { BlameGroup, ChangedFile, FileBlame, FileContent, FileDiff } from '../lib/types'

const props = defineProps<{
  file: ChangedFile
  content: FileContent
  blame?: FileBlame | null
  showBlame: boolean
}>()
const emit = defineEmits<{ inspect: [sha: string]; parent: [] }>()
const scroll = ref<HTMLElement>()
const activeLine = ref(0)
const id = useId()
const patch = computed<FileDiff | null>(() =>
  props.content.kind === 'text'
    ? {
        binary: false,
        truncated: props.content.truncated,
        additions: 0,
        deletions: 0,
        hunks: [
          {
            header: '',
            lines: props.content.lines.map((content, index) => ({
              kind: 'addition',
              oldLine: null,
              newLine: index + 1,
              content: content + '\n',
            })),
          },
        ],
      }
    : null,
)
const tokens = useDiffHighlighting(
  () => props.file,
  () => patch.value,
  undefined,
  8_000,
  () => scroll.value,
)
const lineHeight = computed(() => diffFontSize.value * 1.7)
const virtualizer = useVirtualizer(
  computed(() => ({
    count: props.content.lines.length,
    getScrollElement: () => scroll.value ?? null,
    estimateSize: () => lineHeight.value,
    overscan: 8,
  })),
)
const rows = computed(() => virtualizer.value.getVirtualItems())
const firstVisibleLine = computed(() =>
  Math.ceil((virtualizer.value.scrollOffset ?? 0) / lineHeight.value),
)
const groups = computed(() => {
  const map: (BlameGroup | undefined)[] = Array(props.content.lines.length)
  for (const group of props.blame?.groups ?? [])
    for (
      let line = Math.max(0, group.startLine - 1);
      line < Math.min(map.length, group.startLine - 1 + group.lines);
      line++
    )
      map[line] = group
  return map
})
const activeDescendant = computed(() =>
  props.showBlame && rows.value.some((row) => row.index === activeLine.value)
    ? `${id}-line-${activeLine.value}`
    : undefined,
)
let sizeRevision = 0
watch(
  [() => props.content, lineHeight],
  async ([content, height], [previousContent, previousHeight]) => {
    const revision = ++sizeRevision
    const offset = content !== previousContent ? 0 : (scroll.value?.scrollTop ?? 0)
    if (content !== previousContent) activeLine.value = 0
    if (height !== previousHeight) virtualizer.value.measure()
    await nextTick()
    if (revision === sizeRevision && scroll.value)
      virtualizer.value.scrollToOffset((offset / previousHeight) * height)
  },
  { flush: 'post' },
)
onBeforeUnmount(() => sizeRevision++)

function inspectLine(line: number) {
  const group = groups.value[line]
  if (!group) return
  activeLine.value = line
  scroll.value?.focus({ preventScroll: true })
  emit('inspect', group.sha)
}

function keydown(event: KeyboardEvent) {
  if (!props.showBlame || event.altKey || event.metaKey || event.ctrlKey) return
  const page = Math.max(
    1,
    Math.floor((scroll.value?.clientHeight ?? lineHeight.value) / lineHeight.value),
  )
  let line: number
  switch (event.key) {
    case 'ArrowDown':
      line = activeLine.value + 1
      break
    case 'ArrowUp':
      line = activeLine.value - 1
      break
    case 'PageDown':
      line = activeLine.value + page
      break
    case 'PageUp':
      line = activeLine.value - page
      break
    case 'Home':
      line = 0
      break
    case 'End':
      line = props.content.lines.length - 1
      break
    case 'Enter':
    case ' ':
      event.preventDefault()
      inspectLine(activeLine.value)
      return
    default:
      return
  }
  event.preventDefault()
  activeLine.value = Math.max(0, Math.min(props.content.lines.length - 1, line))
  virtualizer.value.scrollToIndex(activeLine.value, { align: 'auto' })
}
const unavailable = computed(
  () =>
    ({
      binary: 'Fichier binaire ou encodage non UTF-8 : aucun affichage texte ni blame.',
      deleted: 'Ce fichier est absent à la révision sélectionnée.',
      submodule: 'Sous-module : son contenu appartient à un autre dépôt.',
      large: 'Fichier supérieur à 2 Mio : son contenu et son blame ne sont pas chargés.',
      unsupported: 'Ce type d’objet Git ne peut pas être affiché comme un fichier.',
      text: '',
    })[props.content.kind],
)
</script>

<template>
  <div v-if="content.kind !== 'text'" class="file-state">
    <p>{{ unavailable }}</p>
    <button
      v-if="content.kind === 'deleted' && content.parentSha"
      class="file-action"
      @click="emit('parent')"
    >
      Voir la version du premier parent
    </button>
  </div>
  <div v-else-if="!content.lines.length && !content.truncated" class="file-state">
    Fichier vide à cette révision.
  </div>
  <div
    v-else
    ref="scroll"
    class="file-content-scroll"
    tabindex="0"
    :role="showBlame ? 'listbox' : 'region'"
    :aria-label="
      showBlame
        ? 'Blame du fichier, flèches pour parcourir les lignes, Entrée pour consulter le commit'
        : 'Contenu du fichier, défilement horizontal et vertical'
    "
    :aria-activedescendant="activeDescendant"
    @keydown="keydown"
  >
    <div
      class="file-code"
      :style="{
        height: virtualizer.getTotalSize() + 'px',
        '--number-width': String(content.lines.length).length + 2 + 'ch',
      }"
    >
      <div
        v-for="row in rows"
        :key="row.index"
        :id="`${id}-line-${row.index}`"
        class="file-code-line"
        :class="{ 'file-active-line': showBlame && activeLine === row.index }"
        :role="showBlame ? 'option' : undefined"
        :aria-selected="showBlame ? activeLine === row.index : undefined"
        :aria-posinset="showBlame ? row.index + 1 : undefined"
        :aria-setsize="showBlame ? content.lines.length : undefined"
        :style="{ height: row.size + 'px', transform: 'translateY(' + row.start + 'px)' }"
      >
        <button
          v-if="showBlame && groups[row.index]"
          class="blame-attribution"
          tabindex="-1"
          :class="{ boundary: groups[row.index]!.boundary }"
          :aria-label="
            'Consulter le commit ' +
            shortSha(groups[row.index]!.sha) +
            ' de ' +
            groups[row.index]!.author +
            ', ' +
            commitDate(groups[row.index]!.time)
          "
          :title="
            groups[row.index]!.author +
            ' · ' +
            commitDate(groups[row.index]!.time) +
            (groups[row.index]!.boundary ? ' · Frontière du parcours' : '')
          "
          @click="inspectLine(row.index)"
        >
          <template
            v-if="groups[row.index]!.startLine === row.index + 1 || row.index === firstVisibleLine"
          >
            <span>{{ shortSha(groups[row.index]!.sha) }}</span
            ><strong>{{ groups[row.index]!.author }}</strong
            ><small>{{ commitDate(groups[row.index]!.time) }}</small> </template
          ><span v-else class="blame-continuation" aria-hidden="true">│</span>
        </button>
        <span v-else-if="showBlame" class="blame-attribution text-muted">—</span>
        <span class="file-line-number" aria-hidden="true">{{ row.index + 1 }}</span>
        <code
          ><template v-if="tokens?.[0]?.[row.index]"
            ><span
              v-for="(token, index) in tokens[0]![row.index]"
              :key="index"
              :class="'syntax-' + token.kind"
              >{{ token.content }}</span
            ></template
          ><template v-else>{{ content.lines[row.index] }}</template></code
        >
      </div>
    </div>
    <p v-if="content.truncated" class="file-notice">
      Aperçu limité à 2 000 lignes et 256 Kio de texte. Le blame concerne uniquement les lignes
      affichées.
    </p>
    <p v-else-if="content.size && !content.endsWithNewline" class="file-notice">
      Pas de saut de ligne à la fin du fichier.
    </p>
  </div>
</template>

<style scoped>
.file-content-scroll {
  min-height: 0;
  flex: 1;
  overflow: auto;
}
.file-content-scroll:focus-visible {
  outline: 1px solid var(--accent);
  outline-offset: -1px;
}
.file-code {
  position: relative;
  min-width: max-content;
  font: var(--diff-font-size, 11px)/1.7 var(--mono);
}
.file-code-line {
  position: absolute;
  top: 0;
  left: 0;
  display: flex;
  min-width: 100%;
  white-space: pre;
}
.file-code-line code {
  padding-right: 20px;
  font: inherit;
  color: var(--text);
}
.file-content-scroll:focus-visible .file-active-line {
  background: var(--surface);
}
.file-line-number {
  width: var(--number-width);
  padding-right: 8px;
  margin-right: 12px;
  flex-shrink: 0;
  color: var(--muted);
  text-align: right;
  border-right: 1px solid var(--border);
  user-select: none;
}
.blame-attribution {
  display: flex;
  width: 240px;
  flex-shrink: 0;
  align-items: center;
  gap: 8px;
  padding: 0 8px;
  border: 0;
  border-right: 1px solid var(--border);
  background: var(--panel);
  color: var(--muted);
  font: inherit;
  cursor: pointer;
  text-align: left;
  overflow: hidden;
}
.blame-attribution strong {
  max-width: 90px;
  overflow: hidden;
  text-overflow: ellipsis;
  font-weight: 400;
  color: var(--text);
}
.blame-attribution small {
  overflow: hidden;
  text-overflow: ellipsis;
  font: inherit;
}
.blame-attribution:hover,
.blame-attribution:focus-visible {
  background: var(--surface);
  color: var(--accent);
}
.blame-attribution.boundary {
  border-left: 2px solid var(--warning);
}
.blame-continuation {
  opacity: 0.35;
}
.file-state {
  display: flex;
  flex: 1;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 12px;
  padding: 24px;
  text-align: center;
  color: var(--muted);
}
.file-action {
  padding: 7px 12px;
  border: 1px solid var(--border);
  border-radius: 4px;
  background: var(--surface);
  color: var(--text);
  cursor: pointer;
}
.file-notice {
  position: sticky;
  left: 0;
  max-width: 100%;
  padding: 12px 16px;
  font-size: calc(11px + var(--font-size-offset));
  color: var(--warning);
}
.syntax-keyword {
  color: var(--syntax-keyword);
}
.syntax-string {
  color: var(--syntax-string);
}
.syntax-comment {
  color: var(--syntax-comment);
}
.syntax-number {
  color: var(--syntax-number);
}
.syntax-type {
  color: var(--syntax-type);
}
.syntax-function {
  color: var(--syntax-function);
}
.syntax-punctuation {
  color: var(--syntax-punctuation);
}
.syntax-property {
  color: var(--syntax-property);
}
</style>
