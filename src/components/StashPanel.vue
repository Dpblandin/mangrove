<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { Archive, AlertCircle, FileText } from '@lucide/vue'
import ChangedFileList from './ChangedFileList.vue'
import DiffViewer from './DiffViewer.vue'
import DiffFontControls from './DiffFontControls.vue'
import { commitDate, count, shortSha } from '../lib/format'
import type { ChangedFile, FileDiff, StashArea, StashDetails } from '../lib/types'
import type { HighlightedDiff } from '../lib/diffSyntax'
const props = defineProps<{
  details: StashDetails | null
  loading: boolean
  error: string
  selectedFile: ChangedFile | null
  selectedArea: StashArea | null
  diff: FileDiff | null
  highlighted: HighlightedDiff | null
  diffLoading: boolean
  diffError: string
  diffMode: 'central' | 'compact'
}>()
const emit = defineEmits<{
  selectFile: [area: StashArea, file: ChangedFile]
  areaChanged: []
  diffModeChanged: [mode: 'central' | 'compact']
}>()
const area = ref<StashArea>('worktree')
const labels = { worktree: 'Répertoire de travail', index: 'Index', untracked: 'Non suivis' }
const group = computed(() => props.details?.groups.find((group) => group.area === area.value))
function selectArea(next: StashArea) {
  if (next === area.value) return
  area.value = next
  emit('areaChanged')
}
watch(
  () => props.details?.sha,
  () => {
    area.value = props.details?.groups.find((group) => group.files.length)?.area ?? 'worktree'
  },
  { immediate: true },
)
watch(
  () => props.selectedArea,
  (selected) => {
    if (selected) area.value = selected
  },
)
</script>
<template>
  <aside
    class="details-panel flex h-full min-h-0 flex-col border-l border-border bg-panel text-text"
    aria-label="Détails du stash"
    :aria-busy="loading || diffLoading"
  >
    <header
      class="flex h-12 shrink-0 items-center gap-2 border-b border-border px-[19px] text-xs font-semibold"
    >
      <Archive :size="16" class="text-muted" />Stash
    </header>
    <div v-if="error" class="p-5 text-xs text-danger" role="alert">
      <AlertCircle :size="20" />
      <p>{{ error }}</p>
    </div>
    <p v-else-if="!details" class="p-5 text-xs text-muted">
      {{ loading ? 'Lecture du stash…' : 'Ce stash est indisponible. Actualisez le dépôt.' }}
    </p>
    <template v-else>
      <section class="shrink-0 space-y-2 border-b border-border p-4 text-xs">
        <h2
          class="m-0 max-h-24 overflow-auto text-sm font-medium whitespace-pre-wrap wrap-anywhere"
        >
          {{ details.message }}
        </h2>
        <p class="text-muted">{{ details.author }} · {{ commitDate(details.time) }}</p>
        <p class="text-muted" :title="details.baseSha">
          Base <code>{{ shortSha(details.baseSha) }}</code> · lecture seule
        </p>
        <div class="flex gap-1" role="group" aria-label="Emplacement du diff">
          <button
            v-for="mode in ['central', 'compact'] as const"
            :key="mode"
            class="stash-tab"
            :aria-pressed="diffMode === mode"
            @click="emit('diffModeChanged', mode)"
          >
            {{ mode === 'central' ? 'Diff central' : 'Vue compacte' }}
          </button>
        </div>
      </section>
      <div
        class="flex shrink-0 flex-wrap gap-1 border-b border-border p-2"
        role="group"
        aria-label="Zones du stash"
      >
        <button
          v-for="item in details.groups"
          :key="item.area"
          class="stash-tab"
          :aria-pressed="area === item.area"
          @click="selectArea(item.area)"
        >
          {{ labels[item.area] }} <small>{{ count(item.fileCount) }}</small>
        </button>
      </div>
      <template v-if="group">
        <p
          class="m-0 shrink-0 border-b border-border px-3 py-2 text-[10px] leading-relaxed text-muted"
          :title="`${group.oldSha ?? 'arbre vide'} → ${group.newSha}`"
        >
          {{ group.comparison }}
        </p>
        <ChangedFileList
          :files="group.files"
          :file-count="group.fileCount"
          :commit-sha="`${details.sha}:${area}`"
          :selected-file="selectedArea === area ? selectedFile : null"
          :loading="loading"
          @select-file="emit('selectFile', area, $event)"
        />
      </template>
      <section
        v-if="diffMode === 'compact'"
        class="flex min-h-[110px] flex-[1.2] flex-col border-t border-border bg-reading"
      >
        <header class="flex h-10 shrink-0 items-center gap-2 border-b border-border px-3 text-xs">
          <FileText :size="13" /><span class="min-w-0 flex-1 truncate">{{
            selectedFile?.path ?? 'Diff du stash'
          }}</span
          ><DiffFontControls />
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
.stash-tab {
  border: 1px solid transparent;
  border-radius: 4px;
  padding: 5px 7px;
  background: transparent;
  color: var(--muted);
  font-size: calc(10px + var(--font-size-offset));
  cursor: pointer;
}
.stash-tab[aria-pressed='true'] {
  border-color: var(--accent-border);
  color: var(--accent);
  background: var(--accent-soft);
}
.stash-tab:focus-visible {
  outline: 1px solid var(--accent);
}
</style>
