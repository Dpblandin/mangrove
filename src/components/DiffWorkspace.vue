<script setup lang="ts">
import { ArrowLeft, FileText, X } from '@lucide/vue'
import DiffViewer from './DiffViewer.vue'
import DiffFontControls from './DiffFontControls.vue'
import { count } from '../lib/format'
import type { ChangedFile, FileDiff } from '../lib/types'
import type { HighlightedDiff } from '../lib/diffSyntax'

defineProps<{
  file: ChangedFile | null
  diff: FileDiff | null
  loading: boolean
  error: string
  highlighted?: HighlightedDiff | null
}>()
const emit = defineEmits<{ close: [] }>()
</script>

<template>
  <section class="diff-workspace" aria-label="Diff du fichier sélectionné">
    <header class="file-heading">
      <FileText :size="15" class="shrink-0 text-muted" />
      <h1 :title="file?.path">{{ file?.path ?? 'Modifications du fichier' }}</h1>
      <DiffFontControls />
      <button
        class="icon-button"
        aria-label="Fermer le diff"
        title="Fermer le diff · Échap"
        @click="emit('close')"
      >
        <X :size="16" />
      </button>
    </header>
    <div class="diff-toolbar">
      <button class="history-button" @click="emit('close')">
        <ArrowLeft :size="13" /> Historique
      </button>
      <span class="diff-label">Diff</span>
      <span v-if="diff && !error" class="diff-statistics" aria-label="Modifications du fichier">
        <span class="text-success">+{{ count(diff.additions) }}</span>
        <span class="text-danger">−{{ count(diff.deletions) }}</span>
      </span>
    </div>
    <DiffViewer
      :file="file"
      :diff="diff"
      :loading="loading"
      :error="error"
      :highlighted="highlighted"
    />
  </section>
</template>

<style scoped>
.diff-workspace {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  background: var(--reading-bg);
}
.file-heading {
  display: flex;
  align-items: center;
  min-height: 40px;
  gap: 9px;
  padding: 0 16px;
  border-bottom: 1px solid var(--border);
  background: var(--panel);
}
.file-heading h1 {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: calc(12px + var(--font-size-offset));
  font-weight: 500;
}
.diff-toolbar {
  display: flex;
  align-items: center;
  min-height: 40px;
  gap: 12px;
  padding: 0 16px;
  border-bottom: 1px solid var(--border);
}
.history-button {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  border: 1px solid var(--border);
  border-radius: 4px;
  padding: 4px 7px;
  background: var(--surface);
  color: var(--text);
  font: inherit;
  font-size: calc(10px + var(--font-size-offset));
  cursor: pointer;
}
.history-button:hover {
  border-color: var(--accent);
}
.history-button:focus-visible {
  outline: 1px solid var(--accent);
  outline-offset: 2px;
}
.diff-label {
  color: var(--accent);
  font-size: calc(11px + var(--font-size-offset));
  font-weight: 500;
}
.diff-statistics {
  display: flex;
  gap: 8px;
  margin-left: auto;
  font-family: var(--mono);
  font-size: calc(10px + var(--font-size-offset));
}
</style>
