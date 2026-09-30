<script setup lang="ts">
import { computed } from 'vue'
import { AlertCircle, FileText, GitCommitHorizontal, GitMerge } from '@lucide/vue'
import DiffViewer from './DiffViewer.vue'
import DiffFontControls from './DiffFontControls.vue'
import ChangedFileList from './ChangedFileList.vue'
import type { ChangedFile, CommitDetails, FileDiff } from '../lib/types'
import type { HighlightedDiff } from '../lib/diffSyntax'
import { commitDate, commitDateTime, count, filename, initials, shortSha } from '../lib/format'

const props = defineProps<{
  details: CommitDetails | null
  loading: boolean
  error: string
  selectedFile: ChangedFile | null
  diff: FileDiff | null
  diffLoading: boolean
  diffError: string
  highlighted: HighlightedDiff | null
  diffMode: 'central' | 'compact'
}>()

const emit = defineEmits<{
  selectFile: [file: ChangedFile]
  diffModeChanged: [mode: 'central' | 'compact']
}>()

const title = computed(() => props.details?.message.split('\n')[0] || 'Sans message')
const description = computed(
  () => props.details?.message.split('\n').slice(1).join('\n').trim() || '',
)
</script>

<template>
  <aside
    class="details-panel flex h-full min-h-0 flex-col border-l border-border bg-panel text-text"
    aria-label="Détails du commit"
    :aria-busy="loading"
  >
    <header
      class="flex h-12 shrink-0 items-center justify-between border-b border-border px-[19px]"
    >
      <span class="flex items-center gap-2 text-xs font-semibold"
        ><GitCommitHorizontal :size="16" class="text-muted" /> Commit</span
      >
    </header>

    <div v-if="error && !loading" class="panel-state" role="alert">
      <AlertCircle :size="24" class="text-danger" />
      <strong>Impossible de lire le commit</strong>
      <p>{{ error }}</p>
    </div>
    <div v-else-if="!details && !loading" class="panel-state">
      <span
        class="mb-2 grid size-16 place-items-center rounded-[18px] border border-border bg-surface text-accent"
        ><GitCommitHorizontal :size="30" :stroke-width="1.4"
      /></span>
      <strong>Chaque commit a son histoire</strong>
      <p>Sélectionnez un commit pour découvrir son message, ses fichiers et ses modifications.</p>
    </div>

    <template v-else-if="details">
      <section class="commit-summary shrink-0">
        <div
          :key="details.sha"
          class="commit-message min-h-0 overflow-y-auto"
          role="region"
          aria-label="Message du commit"
          tabindex="0"
        >
          <h2
            class="m-0 text-[calc(15px_+_var(--font-size-offset))] leading-relaxed font-medium [overflow-wrap:anywhere]"
          >
            {{ title }}
          </h2>
          <p
            v-if="description"
            class="mt-2.5 mb-0 text-xs leading-relaxed whitespace-pre-wrap text-muted [overflow-wrap:anywhere]"
          >
            {{ description }}
          </p>
        </div>
        <div class="flex min-w-0 items-center gap-2.5">
          <span
            class="grid size-8 shrink-0 place-items-center rounded-[10px] bg-accent/12 text-[calc(11px_+_var(--font-size-offset))] font-semibold text-accent"
            aria-hidden="true"
            >{{ initials(details.author) }}</span
          >
          <div class="flex min-w-0 flex-col gap-1">
            <strong class="truncate text-xs font-medium" :title="details.email">{{
              details.author
            }}</strong>
            <time
              class="truncate text-[calc(10px_+_var(--font-size-offset))] text-muted"
              :datetime="commitDateTime(details.time)"
              >{{ commitDate(details.time) }}</time
            >
          </div>
        </div>
        <div
          class="flex min-w-0 items-center gap-3 rounded-md bg-bg px-[11px] text-[calc(10px_+_var(--font-size-offset))]"
        >
          <span class="shrink-0 text-muted">SHA</span
          ><code
            class="commit-sha min-w-0 overflow-x-auto font-mono leading-normal whitespace-nowrap text-text/80"
            tabindex="0"
            aria-label="SHA complet du commit"
            >{{ details.sha }}</code
          >
        </div>
        <div
          class="parent-note min-h-0 overflow-y-auto text-[calc(10px_+_var(--font-size-offset))] leading-normal text-muted"
        >
          <div v-if="details.parents.length > 1" class="flex items-start gap-[7px]">
            <GitMerge :size="13" />
            <span
              >Commit de fusion · comparaison avec le premier parent
              <code>{{ shortSha(details.parents[0]!) }}</code></span
            >
          </div>
          <div v-else-if="!details.parents.length" class="flex items-start gap-[7px]">
            <GitCommitHorizontal :size="13" /><span>Premier commit du dépôt</span>
          </div>
        </div>
      </section>

      <section
        class="flex min-h-0 flex-col border-t border-border"
        :class="diffMode === 'compact' ? 'h-[30%] shrink' : 'flex-1'"
        aria-label="Fichiers modifiés"
      >
        <div
          class="section-heading flex min-h-10 shrink-0 items-center justify-between gap-2.5 px-[18px]"
        >
          <h3>
            Fichiers modifiés
            <span
              class="inline-flex h-[17px] items-center rounded bg-surface px-[5px] text-[calc(9px_+_var(--font-size-offset))] text-muted"
              >{{ count(details.fileCount) }}</span
            >
          </h3>
          <span
            v-if="diff && !diffError"
            class="flex shrink-0 gap-2 font-mono text-[calc(10px_+_var(--font-size-offset))]"
            aria-label="Modifications du fichier sélectionné"
          >
            <span class="text-success">+{{ count(diff.additions) }}</span>
            <span class="text-danger">−{{ count(diff.deletions) }}</span>
          </span>
        </div>
        <div
          class="flex shrink-0 items-center gap-1 border-b border-border px-[18px] pb-2"
          role="group"
          aria-label="Affichage du diff"
        >
          <button
            type="button"
            class="diff-mode-button"
            :aria-pressed="diffMode === 'central'"
            @click="emit('diffModeChanged', 'central')"
          >
            Vue centrale
          </button>
          <button
            type="button"
            class="diff-mode-button"
            :aria-pressed="diffMode === 'compact'"
            @click="emit('diffModeChanged', 'compact')"
          >
            Vue compacte
          </button>
        </div>
        <ChangedFileList
          :files="details.files"
          :file-count="details.fileCount"
          :commit-sha="details.sha"
          :selected-file="selectedFile"
          :loading="loading"
          @select-file="emit('selectFile', $event)"
        />
      </section>

      <section
        v-if="diffMode === 'compact'"
        class="flex min-h-[110px] flex-1 flex-col border-t border-border bg-reading"
        aria-label="Modifications du fichier"
      >
        <div
          class="diff-heading flex min-h-10 shrink-0 items-center justify-between gap-2.5 border-b border-border bg-panel px-[18px]"
        >
          <h3>
            <FileText :size="13" />
            {{ selectedFile ? filename(selectedFile.path) : 'Aperçu des modifications' }}
          </h3>
          <DiffFontControls />
        </div>
        <DiffViewer
          :file="selectedFile"
          :diff="diff"
          :loading="diffLoading"
          :error="diffError"
          :highlighted="highlighted"
        />
      </section>
    </template>
  </aside>
</template>

<style scoped>
@reference '../style.css';

.commit-summary {
  display: grid;
  height: calc(clamp(220px, 36%, 290px) + var(--font-size-offset) * 3);
  grid-template-rows:
    minmax(0, 1fr)
    calc(38px + var(--font-size-offset) * 3)
    calc(36px + var(--font-size-offset) * 2)
    calc(30px + var(--font-size-offset) * 2);
  gap: 10px;
  padding: 20px 20px 16px;
}
.commit-message {
  scrollbar-gutter: stable;
}
.commit-message:focus-visible,
.commit-sha:focus-visible {
  outline: 1px solid var(--accent);
  outline-offset: 3px;
}
.panel-state {
  @apply flex flex-1 flex-col items-center justify-center gap-3 p-[30px] text-center text-xs text-muted;
}
.panel-state strong {
  @apply text-sm font-medium text-text;
}
.panel-state p {
  @apply m-0 max-w-[280px] leading-relaxed [overflow-wrap:anywhere];
}
.section-heading h3 {
  @apply m-0 flex min-w-0 items-center gap-2 text-[calc(11px_+_var(--font-size-offset))] font-medium;
}
.diff-heading h3 {
  @apply m-0 min-w-0 truncate text-[calc(11px_+_var(--font-size-offset))] font-medium;
}
.diff-heading h3 svg {
  @apply mr-[5px] inline-block align-[-2px] text-muted;
}
.parent-note svg {
  @apply mt-0.5 shrink-0;
}
.parent-note code {
  @apply font-mono text-text;
}
.diff-mode-button {
  @apply cursor-pointer rounded border border-transparent bg-transparent px-2 py-1 text-[calc(10px_+_var(--font-size-offset))] text-muted hover:bg-surface aria-pressed:border-accent/30 aria-pressed:bg-accent/10 aria-pressed:text-accent;
}
</style>
