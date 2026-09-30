<script setup lang="ts">
import { computed, useId } from 'vue'
import { AlertCircle, Check, FileText, FolderGit2 } from '@lucide/vue'
import ChangedFileList from './ChangedFileList.vue'
import DiffViewer from './DiffViewer.vue'
import DiffFontControls from './DiffFontControls.vue'
import { count, filename } from '../lib/format'
import type {
  ChangedFile,
  FileDiff,
  WorkingTreeArea,
  WorkingTreeFile,
  WorkingTreeStatus,
} from '../lib/types'
import type { HighlightedDiff } from '../lib/diffSyntax'

const props = defineProps<{
  status: WorkingTreeStatus | null
  selectedFile: WorkingTreeFile | null
  selectedArea: WorkingTreeArea | null
  diff: FileDiff | null
  highlighted: HighlightedDiff | null
  loading: boolean
  diffLoading: boolean
  error: string
  diffMode: 'central' | 'compact'
}>()
const emit = defineEmits<{
  selectFile: [area: WorkingTreeArea, file: WorkingTreeFile]
  diffModeChanged: [mode: 'central' | 'compact']
}>()
const id = useId()
function selectLocalFile(area: WorkingTreeArea, file: ChangedFile) {
  const target = props.status?.[area].find((candidate) => candidate === file)
  if (target) emit('selectFile', area, target)
}
const sections = computed(() => [
  {
    area: 'unstaged' as const,
    title: 'Non indexés',
    comparison: 'Comparaison avec l’index',
    empty: 'Aucun fichier non indexé.',
    icon: FileText,
    files: props.status?.unstaged ?? [],
    fileCount: props.status?.unstagedCount ?? 0,
  },
  {
    area: 'staged' as const,
    title: 'Indexés',
    comparison: 'Comparaison avec HEAD',
    empty: 'Aucun fichier indexé.',
    icon: Check,
    files: props.status?.staged ?? [],
    fileCount: props.status?.stagedCount ?? 0,
  },
])
const smallerSection = computed(() => {
  const [unstaged, staged] = sections.value
  return unstaged.files.length < staged.files.length ? unstaged : staged
})
const diffError = computed(() =>
  props.selectedFile && !props.diff && !props.loading ? props.error : '',
)
</script>

<template>
  <aside
    class="working-tree-panel flex h-full min-h-0 flex-col border-l border-border bg-panel text-text"
    aria-label="Modifications locales"
    :aria-busy="loading || diffLoading"
  >
    <header
      class="panel-header flex h-12 shrink-0 items-center gap-2 border-b border-border px-[19px]"
    >
      <FolderGit2 :size="16" class="shrink-0 text-muted" aria-hidden="true" />
      <h2 class="min-w-0 flex-1 truncate" title="Modifications locales">Modifications locales</h2>
    </header>

    <div v-if="error && !loading && !status" class="panel-state" role="alert">
      <AlertCircle :size="24" class="text-danger" aria-hidden="true" />
      <strong>Impossible de lire les modifications locales</strong>
      <p>{{ error }}</p>
    </div>
    <div v-else-if="!status && !loading" class="panel-state">
      <FolderGit2 :size="28" class="text-accent" aria-hidden="true" />
      <strong>Fichiers non commités</strong>
      <p>
        Ouvrez un dépôt pour consulter les changements de son index et de ses fichiers de travail.
      </p>
    </div>
    <div v-else-if="status && !status.available" class="panel-state">
      <FolderGit2 :size="28" class="text-muted" aria-hidden="true" />
      <strong>Ce dépôt n’a pas d’arbre de travail</strong>
      <p>Consultez ses commits pour voir les fichiers versionnés.</p>
    </div>

    <template v-else-if="status">
      <div
        v-if="error && !loading && !diffLoading"
        class="error-notice"
        role="alert"
        aria-label="Erreur de lecture des modifications locales"
        tabindex="0"
      >
        <AlertCircle :size="15" class="mt-0.5 shrink-0 text-danger" aria-hidden="true" />
        <p>{{ error }}</p>
      </div>
      <div class="mode-controls" role="group" aria-label="Affichage du diff">
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

      <div
        class="file-sections min-h-0"
        :class="{
          compact: diffMode === 'compact',
          'small-unstaged': smallerSection.area === 'unstaged',
          'small-empty': !smallerSection.files.length,
        }"
        :style="{ '--small-section-lines': Math.min(6, smallerSection.files.length) }"
      >
        <section
          v-for="section in sections"
          :key="section.area"
          class="file-section flex min-h-0 flex-col"
          :aria-labelledby="`${id}-${section.area}-title`"
        >
          <header class="section-heading flex shrink-0 items-center gap-2.5 px-[18px]">
            <component
              :is="section.icon"
              :size="14"
              class="shrink-0 text-muted"
              aria-hidden="true"
            />
            <div class="min-w-0 flex-1">
              <h3 :id="`${id}-${section.area}-title`">{{ section.title }}</h3>
              <p>{{ section.comparison }}</p>
            </div>
            <span class="file-count shrink-0 rounded bg-surface px-[5px] text-muted">
              {{ count(section.fileCount) }}
            </span>
          </header>
          <ChangedFileList
            v-if="section.files.length"
            :files="section.files"
            :file-count="section.fileCount"
            :commit-sha="`working-tree:${section.area}`"
            :selected-file="selectedArea === section.area ? selectedFile : null"
            :loading="loading"
            :aria-label="`Fichiers ${section.title.toLocaleLowerCase()}`"
            @select-file="selectLocalFile(section.area, $event)"
          />
          <p v-else class="empty-section text-muted">{{ section.empty }}</p>
        </section>
      </div>

      <section
        v-if="diffMode === 'compact'"
        class="compact-diff flex min-h-[110px] min-w-0 flex-1 flex-col border-t border-border bg-reading"
        aria-label="Modifications du fichier local"
      >
        <header
          class="diff-heading flex min-h-10 shrink-0 items-center gap-2 border-b border-border bg-panel px-[18px]"
        >
          <FileText :size="13" class="shrink-0 text-muted" aria-hidden="true" />
          <h3 class="min-w-0 flex-1 truncate" :title="selectedFile?.path">
            {{ selectedFile ? filename(selectedFile.path) : 'Aperçu des modifications' }}
          </h3>
          <span v-if="selectedFile && diff" class="diff-counts flex shrink-0 gap-2 font-mono">
            <span class="text-success">+{{ count(diff.additions) }}</span>
            <span class="text-danger">−{{ count(diff.deletions) }}</span>
          </span>
          <DiffFontControls />
        </header>
        <DiffViewer
          :file="selectedFile"
          :diff="diff"
          :highlighted="highlighted"
          :loading="diffLoading"
          :error="diffError"
        />
      </section>
    </template>
  </aside>
</template>

<style scoped>
.panel-header h2 {
  margin: 0;
  font-size: calc(12px + var(--font-size-offset, 1px));
  font-weight: 600;
}
.panel-state {
  display: flex;
  flex: 1;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 12px;
  padding: 30px;
  text-align: center;
  color: var(--muted);
  font-size: calc(11px + var(--font-size-offset, 1px));
}
.panel-state strong {
  color: var(--text);
  font-size: calc(13px + var(--font-size-offset, 1px));
  font-weight: 500;
}
.panel-state p {
  max-width: 280px;
  margin: 0;
  line-height: 1.6;
  overflow-wrap: anywhere;
}
.error-notice {
  display: flex;
  flex-shrink: 0;
  align-items: flex-start;
  gap: 8px;
  max-height: 100px;
  overflow: auto;
  padding: 10px 18px;
  border-bottom: 1px solid var(--border);
  color: var(--danger);
  font-size: calc(10px + var(--font-size-offset, 1px));
  line-height: 1.5;
}
.error-notice p {
  margin: 0;
  overflow-wrap: anywhere;
}
.mode-controls {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  gap: 4px;
  min-height: 42px;
  padding: 7px 18px;
  border-bottom: 1px solid var(--border);
}
.diff-mode-button {
  border: 1px solid transparent;
  border-radius: 4px;
  padding: 4px 8px;
  background: transparent;
  color: var(--muted);
  font-size: calc(10px + var(--font-size-offset, 1px));
  cursor: pointer;
}
.diff-mode-button:hover {
  background: var(--surface);
}
.diff-mode-button[aria-pressed='true'] {
  border-color: var(--accent-border);
  background: var(--accent-soft);
  color: var(--accent);
}
.file-sections {
  --small-section-height: calc(
    48px + var(--small-section-lines) *
      var(--file-row-height, calc(32px + 2 * var(--font-size-offset, 1px))) + 9px
  );
  display: grid;
  flex: 1;
  grid-template-rows: minmax(0, 1fr) min(50%, var(--small-section-height));
}
.file-sections.compact {
  flex: 0 0 44%;
}
.file-sections.small-unstaged {
  grid-template-rows: min(50%, var(--small-section-height)) minmax(0, 1fr);
}
.file-sections.small-empty {
  --small-section-height: 94px;
}
.file-section + .file-section {
  border-top: 1px solid var(--border);
}
.section-heading {
  height: 48px;
}
.section-heading h3,
.diff-heading h3 {
  margin: 0;
  font-size: calc(11px + var(--font-size-offset, 1px));
  font-weight: 500;
}
.section-heading p {
  overflow: hidden;
  margin: 2px 0 0;
  color: var(--muted);
  font-size: calc(9px + var(--font-size-offset, 1px));
  text-overflow: ellipsis;
  white-space: nowrap;
}
.file-count {
  font-size: calc(9px + var(--font-size-offset, 1px));
  line-height: 1.8;
}
.empty-section {
  overflow: auto;
  margin: 0;
  padding: 9px 18px 14px;
  font-size: calc(11px + var(--font-size-offset, 1px));
  line-height: 1.5;
}
.diff-counts {
  font-size: calc(10px + var(--font-size-offset, 1px));
}
</style>
