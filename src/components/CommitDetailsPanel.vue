<script setup lang="ts">
import { computed } from 'vue'
import { AlertCircle, FileText, GitCommitHorizontal, GitMerge, ShieldCheck } from '@lucide/vue'
import DiffViewer from './DiffViewer.vue'
import type { ChangedFile, CommitDetails, FileDiff } from '../lib/types'
import { commitDate, count, filename, fileStatusLabel, initials, shortSha } from '../lib/format'

const props = defineProps<{
  details: CommitDetails | null
  loading: boolean
  error: string
  selectedFile: ChangedFile | null
  diff: FileDiff | null
  diffLoading: boolean
  diffError: string
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

function directory(path: string) {
  const lastSeparator = path.lastIndexOf('/')
  return lastSeparator < 0 ? '' : path.slice(0, lastSeparator + 1)
}

function statusClass(status: ChangedFile['status']) {
  if (status === 'added') return 'text-success'
  if (status === 'deleted') return 'text-danger'
  if (status === 'renamed') return 'text-renamed'
  if (status === 'unmodified' || status === 'unknown') return 'text-muted'
  return 'text-warning'
}

function selected(file: ChangedFile) {
  return props.selectedFile?.index === file.index && props.selectedFile?.path === file.path
}
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
      <span class="inline-flex items-center gap-1.5 text-[10px] text-muted"
        ><ShieldCheck :size="12" /> Lecture seule</span
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
      <section class="max-h-[38%] shrink-0 overflow-y-auto p-5 pb-4">
        <h2 class="mb-4 text-[15px] leading-relaxed font-medium [overflow-wrap:anywhere]">
          {{ title }}
        </h2>
        <p
          v-if="description"
          class="-mt-1.5 mb-[18px] text-xs leading-relaxed whitespace-pre-wrap text-muted [overflow-wrap:anywhere]"
        >
          {{ description }}
        </p>
        <div class="mb-[15px] flex items-center gap-2.5">
          <span
            class="grid size-8 shrink-0 place-items-center rounded-[10px] bg-accent/12 text-[11px] font-semibold text-accent"
            aria-hidden="true"
            >{{ initials(details.author) }}</span
          >
          <div class="flex min-w-0 flex-col gap-1">
            <strong class="truncate text-xs font-medium" :title="details.email">{{
              details.author
            }}</strong>
            <time
              class="text-[10px] text-muted"
              :datetime="new Date(details.time * 1000).toISOString()"
              >{{ commitDate(details.time) }}</time
            >
          </div>
        </div>
        <div class="flex items-start gap-3 rounded-md bg-bg px-[11px] py-2.5 text-[10px]">
          <span class="pt-px text-muted">SHA</span
          ><code class="font-mono leading-normal text-text/80 [overflow-wrap:anywhere]">{{
            details.sha
          }}</code>
        </div>
        <div
          v-if="details.parents.length > 1"
          class="parent-note mt-[11px] flex items-start gap-[7px] text-[10px] leading-relaxed text-muted"
        >
          <GitMerge :size="13" />
          <span
            >Commit de fusion · comparaison avec le premier parent
            <code>{{ shortSha(details.parents[0]!) }}</code></span
          >
        </div>
        <div
          v-else-if="!details.parents.length"
          class="parent-note mt-[11px] flex items-start gap-[7px] text-[10px] leading-relaxed text-muted"
        >
          <GitCommitHorizontal :size="13" /><span>Premier commit du dépôt</span>
        </div>
      </section>

      <section
        class="flex min-h-0 flex-col border-t border-border"
        :class="diffMode === 'compact' ? 'max-h-[30%] shrink-0' : 'flex-1'"
        aria-label="Fichiers modifiés"
      >
        <div
          class="section-heading flex min-h-10 shrink-0 items-center justify-between gap-2.5 px-[18px]"
        >
          <h3>
            Fichiers modifiés
            <span
              class="inline-flex h-[17px] items-center rounded bg-surface px-[5px] text-[9px] text-muted"
              >{{ count(details.fileCount) }}</span
            >
          </h3>
          <span
            v-if="diff && !diffError"
            class="flex shrink-0 gap-2 font-mono text-[10px]"
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
        <div class="min-h-0 overflow-y-auto px-[7px] pb-[9px]">
          <button
            v-for="file in details.files"
            :key="file.index"
            class="flex w-full cursor-pointer items-center gap-2.5 rounded-[5px] border border-transparent bg-transparent px-2.5 py-[9px] text-left text-text hover:bg-surface focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent aria-pressed:border-accent/25 aria-pressed:bg-accent/10 disabled:cursor-default disabled:opacity-100"
            :disabled="loading"
            :aria-pressed="selected(file)"
            :title="file.oldPath ? `${file.oldPath} → ${file.path}` : file.path"
            @click="emit('selectFile', file)"
          >
            <FileText :size="15" class="shrink-0 opacity-85" :class="statusClass(file.status)" />
            <span class="flex min-w-0 flex-1 flex-col gap-[3px]"
              ><strong class="truncate text-[11px] font-normal">{{ filename(file.path) }}</strong
              ><small v-if="directory(file.path)" class="truncate text-[9px] text-muted">{{
                directory(file.path)
              }}</small></span
            >
            <span class="shrink-0 text-[9px]" :class="statusClass(file.status)">{{
              fileStatusLabel(file.status)
            }}</span>
          </button>
          <p
            v-if="!details.files.length"
            class="mx-[11px] my-1.5 text-[11px] leading-relaxed text-muted"
          >
            Aucun fichier modifié dans cette comparaison.
          </p>
          <p
            v-if="details.files.length < details.fileCount"
            class="mx-[11px] my-1.5 text-[11px] leading-relaxed text-muted"
          >
            Les {{ count(details.files.length) }} premiers fichiers sur
            {{ count(details.fileCount) }} sont affichés.
          </p>
        </div>
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
          <span v-if="selectedFile" class="font-mono text-[9px] tracking-[.7px] text-muted"
            >DIFF</span
          >
        </div>
        <DiffViewer :file="selectedFile" :diff="diff" :loading="diffLoading" :error="diffError" />
      </section>
    </template>
  </aside>
</template>

<style scoped>
@reference '../style.css';

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
  @apply m-0 flex min-w-0 items-center gap-2 text-[11px] font-medium;
}
.diff-heading h3 {
  @apply m-0 min-w-0 truncate text-[11px] font-medium;
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
  @apply cursor-pointer rounded border border-transparent bg-transparent px-2 py-1 text-[10px] text-muted hover:bg-surface aria-pressed:border-accent/30 aria-pressed:bg-accent/10 aria-pressed:text-accent;
}
</style>
