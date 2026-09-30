<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, shallowRef, watch } from 'vue'
import { AlertCircle, FileText } from '@lucide/vue'
import { useDiffHighlighting } from '../composables/useDiffHighlighting'
import type { HighlightedDiff } from '../lib/diffSyntax'
import type { ChangedFile, FileDiff } from '../lib/types'

const props = defineProps<{
  file: ChangedFile | null
  diff: FileDiff | null
  loading: boolean
  error: string
  highlighted?: HighlightedDiff | null
}>()
const viewer = ref<HTMLElement>()
const lineNumberWidth = computed(() => {
  let largest = 1
  for (const hunk of props.diff?.hunks ?? [])
    for (const line of hunk.lines) largest = Math.max(largest, line.oldLine ?? 0, line.newLine ?? 0)
  return `${String(largest).length + 2}ch`
})
const generatedTokens = useDiffHighlighting(
  () => (props.highlighted === undefined ? props.file : null),
  () => (props.highlighted === undefined ? props.diff : null),
)
const tokens = shallowRef<HighlightedDiff | null>(null)
let deferredTokens: HighlightedDiff | null | undefined
let selectionDocument: Document | undefined

function selectionTouchesViewer() {
  const element = viewer.value
  const selection = element?.ownerDocument.getSelection()
  if (!element || !selection || selection.isCollapsed) return false
  for (let index = 0; index < selection.rangeCount; index++) {
    if (selection.getRangeAt(index).intersectsNode(element)) return true
  }
  return false
}

function publishTokens(value: HighlightedDiff | null) {
  // Replacing text nodes with token spans would invalidate the native text selection.
  if (selectionTouchesViewer()) deferredTokens = value
  else {
    deferredTokens = undefined
    tokens.value = value
  }
}

function selectionChanged() {
  if (deferredTokens !== undefined && !selectionTouchesViewer()) {
    tokens.value = deferredTokens
    deferredTokens = undefined
  }
}

watch(
  [() => props.file, () => props.diff],
  () => {
    tokens.value = null
    deferredTokens = undefined
  },
  { flush: 'sync' },
)
watch(
  () => (props.highlighted === undefined ? generatedTokens.value : props.highlighted),
  publishTokens,
  { immediate: true, flush: 'sync' },
)
onMounted(() => {
  selectionDocument = viewer.value?.ownerDocument
  selectionDocument?.addEventListener('selectionchange', selectionChanged)
})
onBeforeUnmount(() => {
  selectionDocument?.removeEventListener('selectionchange', selectionChanged)
  deferredTokens = undefined
})
</script>

<template>
  <div
    ref="viewer"
    class="diff-viewer flex min-h-0 min-w-0 flex-1 flex-col bg-reading"
    role="region"
    aria-label="Modifications du fichier"
    :aria-busy="loading"
  >
    <template v-if="!loading || diff">
      <div v-if="error" class="diff-state" role="alert">
        <AlertCircle :size="20" class="text-danger" />
        <p>{{ error }}</p>
      </div>
      <div v-else-if="!file" class="diff-state">
        <FileText :size="22" :stroke-width="1.5" />
        <p>Sélectionnez un fichier pour voir ses modifications.</p>
      </div>
      <div v-else-if="diff?.binary" class="diff-state">
        <FileText :size="24" :stroke-width="1.5" /><strong>Fichier binaire</strong>
        <p>Le contenu de ce fichier ne peut pas être affiché sous forme de diff texte.</p>
      </div>
      <div
        v-else-if="diff"
        class="min-h-0 flex-1 overflow-auto pb-3 focus-visible:outline-1 focus-visible:-outline-offset-1 focus-visible:outline-accent"
        tabindex="0"
        aria-label="Diff du fichier, défilement horizontal et vertical"
      >
        <div
          v-if="file.oldPath && file.oldPath !== file.path"
          class="border-b border-border px-[17px] py-[13px] text-[calc(11px_+_var(--font-size-offset))] leading-relaxed text-muted [overflow-wrap:anywhere]"
        >
          Renommé depuis
          <code class="font-mono text-[calc(10px_+_var(--font-size-offset))]">{{
            file.oldPath
          }}</code>
        </div>
        <div
          v-for="(hunk, hunkIndex) in diff.hunks"
          :key="hunkIndex"
          class="diff-hunk min-w-max font-mono"
          :style="{ '--line-number-width': lineNumberWidth }"
        >
          <div
            class="hunk-header min-w-full border-t border-border bg-surface px-[13px] py-[5px] whitespace-pre text-accent"
          >
            {{ hunk.header.trimEnd() }}
          </div>
          <div
            v-for="(line, lineIndex) in hunk.lines"
            :key="lineIndex"
            class="diff-line flex min-w-max whitespace-pre"
            :class="line.kind"
          >
            <span
              class="inline-block w-[var(--line-number-width)] shrink-0 border-r border-border pr-[5px] text-right text-muted select-none"
              aria-hidden="true"
              >{{ line.oldLine }}</span
            >
            <span
              class="inline-block w-[var(--line-number-width)] shrink-0 border-r border-border pr-[5px] text-right text-muted select-none"
              aria-hidden="true"
              >{{ line.newLine }}</span
            >
            <span
              class="line-sign inline-block w-5 shrink-0 text-center text-muted select-none"
              aria-hidden="true"
              >{{ line.kind === 'addition' ? '+' : line.kind === 'deletion' ? '−' : ' ' }}</span
            >
            <code class="pr-[15px] [font:inherit]"
              ><template v-if="tokens?.[hunkIndex]?.[lineIndex]"
                ><span
                  v-for="(token, tokenIndex) in tokens[hunkIndex]![lineIndex]"
                  :key="tokenIndex"
                  :class="`syntax-${token.kind}`"
                  >{{ token.content }}</span
                ></template
              ><template v-else>{{ line.content.replace(/\r?\n$/, '') }}</template></code
            >
          </div>
        </div>
        <div
          v-if="!diff.hunks.length && !diff.truncated"
          class="px-[17px] py-[13px] text-[calc(11px_+_var(--font-size-offset))] leading-relaxed text-muted [overflow-wrap:anywhere]"
        >
          Aucune différence de contenu texte. Le changement peut concerner le nom ou les permissions
          du fichier.
        </div>
        <div
          v-if="diff.truncated"
          class="sticky left-0 flex max-w-full items-start gap-2 bg-reading px-[17px] py-[13px] text-[calc(10px_+_var(--font-size-offset))] leading-relaxed text-warning"
        >
          <AlertCircle :size="14" class="mt-0.5 shrink-0" /><span
            >Aperçu limité pour préserver la réactivité. Le fichier est volumineux ou le diff
            dépasse la limite d’affichage.</span
          >
        </div>
      </div>
      <div v-else class="diff-state">
        <span>Le diff de ce fichier n’est pas disponible.</span>
      </div>
    </template>
  </div>
</template>

<style scoped>
@reference '../style.css';

.diff-state {
  @apply flex flex-1 flex-col items-center justify-center gap-2.5 p-[22px] text-center text-[calc(11px_+_var(--font-size-offset))] text-muted;
}
.diff-state p {
  @apply m-0 max-w-[260px] leading-relaxed [overflow-wrap:anywhere];
}
.diff-state strong {
  @apply font-medium text-text;
}
.diff-hunk {
  font-size: var(--diff-font-size, 11px);
  line-height: 1.7;
}
.diff-hunk:first-child .hunk-header {
  border-top: 0;
}
.diff-viewer ::selection {
  background: color-mix(in srgb, var(--accent) 15%, transparent);
  color: var(--text);
}
.diff-line.addition {
  background: var(--diff-addition-bg);
}
.diff-line.deletion {
  background: var(--diff-deletion-bg);
}
.diff-line.addition > span:not(.line-sign),
.diff-line.deletion > span:not(.line-sign) {
  color: var(--text);
}
.addition .line-sign {
  color: var(--success);
}
.diff-line code {
  color: var(--text);
}
.deletion .line-sign {
  color: var(--danger);
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
