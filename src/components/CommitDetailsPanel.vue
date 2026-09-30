<script setup lang="ts">
import { computed } from 'vue'
import {
  AlertCircle,
  FileText,
  GitCommitHorizontal,
  GitMerge,
  LoaderCircle,
  ShieldCheck,
} from '@lucide/vue'
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
}>()

const emit = defineEmits<{
  selectFile: [file: ChangedFile]
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

    <div v-if="loading" class="panel-state" role="status">
      <LoaderCircle class="animate-spin motion-reduce:animate-none" :size="24" />
      <span>Lecture du commit…</span>
    </div>
    <div v-else-if="error" class="panel-state" role="alert">
      <AlertCircle :size="24" class="text-danger" />
      <strong>Impossible de lire le commit</strong>
      <p>{{ error }}</p>
    </div>
    <div v-else-if="!details" class="panel-state">
      <span
        class="mb-2 grid size-16 place-items-center rounded-[18px] border border-border bg-surface text-accent"
        ><GitCommitHorizontal :size="30" :stroke-width="1.4"
      /></span>
      <strong>Chaque commit a son histoire</strong>
      <p>Sélectionnez un commit pour découvrir son message, ses fichiers et ses modifications.</p>
    </div>

    <template v-else>
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
        class="flex max-h-[30%] min-h-0 shrink-0 flex-col border-t border-border"
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
            v-if="diff && !diffLoading && !diffError"
            class="flex shrink-0 gap-2 font-mono text-[10px]"
            aria-label="Modifications du fichier sélectionné"
          >
            <span class="text-success">+{{ count(diff.additions) }}</span>
            <span class="text-danger">−{{ count(diff.deletions) }}</span>
          </span>
        </div>
        <div class="overflow-y-auto px-[7px] pb-[9px]">
          <button
            v-for="file in details.files"
            :key="file.index"
            class="flex w-full cursor-pointer items-center gap-2.5 rounded-[5px] border border-transparent bg-transparent px-2.5 py-[9px] text-left text-text hover:bg-surface focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent aria-pressed:border-accent/25 aria-pressed:bg-accent/10"
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
        class="flex min-h-[110px] flex-1 flex-col border-t border-border bg-bg"
        aria-label="Modifications du fichier"
        :aria-busy="diffLoading"
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
        <div v-if="diffLoading" class="diff-state" role="status">
          <LoaderCircle class="animate-spin motion-reduce:animate-none" :size="20" /><span
            >Lecture des modifications…</span
          >
        </div>
        <div v-else-if="diffError" class="diff-state" role="alert">
          <AlertCircle :size="20" class="text-danger" />
          <p>{{ diffError }}</p>
        </div>
        <div v-else-if="!selectedFile" class="diff-state">
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
            v-if="selectedFile.oldPath && selectedFile.oldPath !== selectedFile.path"
            class="border-b border-border px-[17px] py-[13px] text-[11px] leading-relaxed text-muted [overflow-wrap:anywhere]"
          >
            Renommé depuis <code class="font-mono text-[10px]">{{ selectedFile.oldPath }}</code>
          </div>
          <div
            v-for="(hunk, hunkIndex) in diff.hunks"
            :key="hunkIndex"
            class="diff-hunk min-w-max font-mono text-[10px] leading-5"
          >
            <div
              class="hunk-header min-w-full border-t border-border bg-surface px-[13px] py-[5px] whitespace-pre text-accent"
            >
              {{ hunk.header.trimEnd() }}
            </div>
            <div
              v-for="(line, lineIndex) in hunk.lines"
              :key="lineIndex"
              class="diff-line flex min-h-5 min-w-max whitespace-pre"
              :class="line.kind"
            >
              <span
                class="inline-block w-[30px] shrink-0 border-r border-border pr-[5px] text-right text-muted select-none"
                aria-hidden="true"
                >{{ line.oldLine }}</span
              >
              <span
                class="inline-block w-[30px] shrink-0 border-r border-border pr-[5px] text-right text-muted select-none"
                aria-hidden="true"
                >{{ line.newLine }}</span
              >
              <span
                class="line-sign inline-block w-5 shrink-0 text-center text-muted select-none"
                aria-hidden="true"
                >{{ line.kind === 'addition' ? '+' : line.kind === 'deletion' ? '−' : ' ' }}</span
              >
              <code class="pr-[15px] [font:inherit]">{{ line.content.replace(/\r?\n$/, '') }}</code>
            </div>
          </div>
          <div
            v-if="!diff.hunks.length && !diff.truncated"
            class="px-[17px] py-[13px] text-[11px] leading-relaxed text-muted [overflow-wrap:anywhere]"
          >
            Aucune différence de contenu texte. Le changement peut concerner le nom ou les
            permissions du fichier.
          </div>
          <div
            v-if="diff.truncated"
            class="sticky left-0 flex max-w-full items-start gap-2 bg-bg px-[17px] py-[13px] text-[10px] leading-relaxed text-warning"
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
.diff-state {
  @apply flex flex-1 flex-col items-center justify-center gap-2.5 p-[22px] text-center text-[11px] text-muted;
}
.diff-state p {
  @apply m-0 max-w-[260px] leading-relaxed [overflow-wrap:anywhere];
}
.diff-state strong {
  @apply font-medium text-text;
}
.diff-hunk:first-child .hunk-header {
  border-top: 0;
}
.diff-line.addition {
  background: color-mix(in srgb, var(--success) 9%, var(--bg));
}
.diff-line.deletion {
  background: color-mix(in srgb, var(--danger) 9%, var(--bg));
}
.addition .line-sign,
.addition code {
  color: var(--success);
}
.deletion .line-sign,
.deletion code {
  color: var(--danger);
}
</style>
