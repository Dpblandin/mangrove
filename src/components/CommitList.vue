<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useVirtualizer } from '@tanstack/vue-virtual'
import { ArrowLeft, ArrowRight, GitBranch, GitCommitHorizontal, SearchX, Tag } from '@lucide/vue'
import GraphCanvas from './GraphCanvas.vue'
import { initials, shortDate, shortSha } from '../lib/format'
import type { CommitRow, Reference } from '../lib/types'
import { PAGE_SIZE } from '../composables/useRepository'

const props = defineProps<{
  pages: Map<number, CommitRow[]>
  total: number
  selectedSha: string
  selectedIndex: number
  references: Reference[]
  filtered: boolean
  loading: boolean
  indexing: boolean
  sessionId: string
}>()
const emit = defineEmits<{
  range: [start: number, end: number]
  select: [row: CommitRow, index: number]
  navigate: [direction: -1 | 1]
}>()
const ROW_HEIGHT = 44
const container = ref<HTMLElement>()
const height = ref(500)
const graphWidth = ref(116)
const scrollTop = ref(0)
const laneOffset = ref(0)
const maxLanes = ref(1)
const visibleLanes = computed(() => Math.max(1, Math.floor((graphWidth.value - 30) / 18)))
const virtualizer = useVirtualizer(
  computed(() => ({
    count: props.total,
    getScrollElement: () => container.value ?? null,
    estimateSize: () => ROW_HEIGHT,
    overscan: 8,
  })),
)
const items = computed(() => virtualizer.value.getVirtualItems())
const referenceMap = computed(() => {
  const map = new Map<string, Reference[]>()
  for (const reference of props.references) {
    const existing = map.get(reference.sha) ?? []
    existing.push(reference)
    map.set(reference.sha, existing)
  }
  return map
})
function row(index: number) {
  return props.pages.get(Math.floor(index / PAGE_SIZE) * PAGE_SIZE)?.[index % PAGE_SIZE]
}
const graphRows = computed(() =>
  items.value.flatMap((item) => {
    const commit = row(item.index)
    return commit ? [{ row: commit, start: item.start }] : []
  }),
)
const activeDescendant = computed(() =>
  items.value.some(
    (item) => item.index === props.selectedIndex && row(item.index)?.sha === props.selectedSha,
  )
    ? `commit-option-${props.selectedIndex}`
    : undefined,
)
watch(graphRows, (rows) => {
  maxLanes.value = Math.max(maxLanes.value, ...rows.map(({ row }) => row.graph.width))
})
watch(
  () => props.sessionId,
  () => {
    laneOffset.value = 0
    maxLanes.value = 1
  },
)
watch(
  () => items.value.map((item) => item.index).join(','),
  () => {
    const first = items.value[0]
    const last = items.value.at(-1)
    if (first && last) emit('range', first.index, last.index)
  },
)
let observer: ResizeObserver | undefined
onMounted(() => {
  observer = new ResizeObserver(() => {
    if (!container.value) return
    height.value = container.value.clientHeight
    graphWidth.value =
      Number.parseFloat(getComputedStyle(container.value).getPropertyValue('--graph-width')) || 116
  })
  if (container.value) observer.observe(container.value)
})
onBeforeUnmount(() => observer?.disconnect())

function scrollToIndex(index: number) {
  virtualizer.value.scrollToIndex(index, { align: 'auto' })
}
defineExpose({ scrollToIndex })

function chooseRow(commit: CommitRow, index: number) {
  container.value?.focus({ preventScroll: true })
  emit('select', commit, index)
}

function keydown(event: KeyboardEvent) {
  if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return
  event.preventDefault()
  emit('navigate', event.key === 'ArrowDown' ? 1 : -1)
}
</script>

<template>
  <div class="history">
    <div class="commit-grid column-headings">
      <span class="graph-heading">
        <span>Graphe</span>
        <span v-if="maxLanes > visibleLanes && !filtered" class="graph-navigation">
          <button
            class="graph-pan"
            :disabled="laneOffset === 0"
            aria-label="Décaler le graphe vers la gauche"
            @click="laneOffset = Math.max(0, laneOffset - 2)"
          >
            <ArrowLeft :size="10" />
          </button>
          <button
            class="graph-pan"
            :disabled="laneOffset + visibleLanes >= maxLanes"
            aria-label="Décaler le graphe vers la droite"
            @click="laneOffset = Math.min(maxLanes - visibleLanes, laneOffset + 2)"
          >
            <ArrowRight :size="10" />
          </button>
        </span> </span
      ><span>Message du commit</span> <span class="author-column">Auteur</span
      ><span class="date-column">Date</span><span class="sha-column">SHA</span>
    </div>
    <div class="list-viewport">
      <div
        ref="container"
        class="commit-scroll"
        role="listbox"
        aria-label="Historique des commits"
        :aria-activedescendant="activeDescendant"
        tabindex="0"
        @scroll="scrollTop = container?.scrollTop ?? 0"
        @keydown="keydown"
      >
        <div class="virtual-spacer" :style="{ height: virtualizer.getTotalSize() + 'px' }">
          <template v-for="item in items" :key="String(item.key)">
            <button
              v-if="row(item.index)"
              class="commit-grid commit-row"
              :class="{ selected: row(item.index)!.sha === selectedSha }"
              :style="{ transform: 'translateY(' + item.start + 'px)' }"
              role="option"
              :id="`commit-option-${item.index}`"
              tabindex="-1"
              :aria-selected="row(item.index)!.sha === selectedSha"
              :aria-posinset="item.index + 1"
              :aria-setsize="total"
              :aria-label="row(item.index)!.summary + ', ' + row(item.index)!.author"
              @click="chooseRow(row(item.index)!, item.index)"
            >
              <span class="graph-cell"></span>
              <span class="commit-subject" :title="row(item.index)!.summary">
                <span
                  v-for="reference in (referenceMap.get(row(item.index)!.sha) ?? []).slice(0, 2)"
                  :key="reference.name"
                  class="ref-badge"
                  :class="{ tag: reference.kind === 'tag' }"
                  :title="reference.label"
                >
                  <Tag v-if="reference.kind === 'tag'" :size="10" /><GitBranch v-else :size="10" />
                  <span class="ref-label">{{ reference.label }}</span>
                </span>
                <span class="subject-text">{{ row(item.index)!.summary || '(sans message)' }}</span>
                <GitCommitHorizontal
                  v-if="row(item.index)!.parents.length > 1"
                  :size="13"
                  class="merge-icon"
                  aria-label="Commit de merge"
                />
              </span>
              <span class="author-column commit-author" :title="row(item.index)!.email">
                <span class="mini-avatar">{{ initials(row(item.index)!.author) }}</span>
                <span>{{ row(item.index)!.author }}</span>
              </span>
              <span class="date-column">{{ shortDate(row(item.index)!.time) }}</span>
              <span class="sha-column">{{ shortSha(row(item.index)!.sha) }}</span>
            </button>
            <div
              v-else
              class="commit-grid commit-row skeleton-row"
              :style="{ transform: 'translateY(' + item.start + 'px)' }"
              aria-hidden="true"
            >
              <span></span><span class="skeleton"></span
              ><span class="author-column skeleton"></span>
            </div>
          </template>
        </div>
      </div>
      <GraphCanvas
        :items="graphRows"
        :scroll-top="scrollTop"
        :height="height"
        :width="graphWidth"
        :row-height="ROW_HEIGHT"
        :selected-sha="selectedSha"
        :filtered="filtered"
        :lane-offset="laneOffset"
      />
      <div v-if="total === 0" class="history-empty">
        <template v-if="loading || indexing">
          <span class="spinner"></span><strong>Lecture de l’historique…</strong>
          <span>Les commits apparaissent au fil de l’indexation.</span>
        </template>
        <template v-else-if="filtered">
          <SearchX :size="30" /><strong>Aucun commit trouvé</strong
          ><span>Essayez un autre message, auteur ou SHA.</span>
        </template>
        <template v-else>
          <GitCommitHorizontal :size="32" /><strong>Ce dépôt n’a pas encore de commit</strong>
          <span>Son historique apparaîtra ici.</span>
        </template>
      </div>
    </div>
  </div>
</template>

<style scoped>
.history {
  --graph-width: 116px;
  display: flex;
  flex: 1;
  flex-direction: column;
  min-height: 0;
}
.commit-grid {
  display: grid;
  grid-template-columns: var(--graph-width) minmax(0, 1fr) 132px 72px 72px;
  align-items: center;
  gap: 12px;
}
.column-headings {
  height: 35px;
  flex: none;
  padding-right: 14px;
  border-block: 1px solid var(--border);
  font-size: 10px;
  letter-spacing: 0.045em;
  color: var(--muted);
}
.graph-heading {
  padding-left: 20px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 4px;
}
.graph-navigation {
  display: inline-flex;
  gap: 2px;
}
.graph-pan {
  display: grid;
  place-items: center;
  width: 14px;
  height: 18px;
  padding: 0;
  background: transparent;
  border: 0;
  cursor: pointer;
  color: var(--muted);
}
.graph-pan:disabled {
  cursor: default;
  opacity: 0.3;
}
.list-viewport {
  position: relative;
  flex: 1;
  min-height: 0;
  overflow: hidden;
}
.commit-scroll {
  height: 100%;
  overflow: auto;
  outline: none;
}
.commit-scroll:focus-visible {
  box-shadow: inset 0 0 0 1px var(--accent);
}
.virtual-spacer {
  position: relative;
  min-width: 0;
}
.commit-row {
  position: absolute;
  top: 0;
  left: 0;
  width: 100%;
  height: 44px;
  text-align: left;
  padding: 0 14px 0 0;
  background: transparent;
  border: 0;
  border-bottom: 1px solid color-mix(in srgb, var(--border) 30%, transparent);
  cursor: pointer;
  font-size: 11px;
  color: var(--muted);
}
.commit-row:hover {
  background: var(--hover);
}
.commit-row.selected {
  background: var(--selection);
  box-shadow: inset 2px 0 0 var(--accent);
  border-block: 1px solid var(--accent-border);
}
.commit-row.selected .subject-text {
  color: var(--text);
}
.commit-subject {
  display: flex;
  min-width: 0;
  overflow: hidden;
  align-items: center;
  gap: 7px;
}
.subject-text {
  flex: 1;
  min-width: 80px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  font-size: 12px;
  color: var(--text);
}
.ref-badge {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  max-width: 136px;
  min-width: 0;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  border: 1px solid var(--accent-border);
  border-radius: 4px;
  color: var(--accent);
  background: var(--accent-soft);
  padding: 3px 5px;
  font-size: 9px;
  flex-shrink: 1;
}
.ref-badge > svg {
  flex-shrink: 0;
}
.ref-label {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
}
.ref-badge.tag {
  color: var(--warning);
  background: color-mix(in srgb, var(--warning) 8%, transparent);
  border-color: color-mix(in srgb, var(--warning) 28%, var(--border));
}
.commit-author {
  display: flex;
  align-items: center;
  gap: 6px;
  overflow: hidden;
}
.commit-author > span:last-child {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.mini-avatar {
  height: 21px;
  width: 21px;
  border-radius: 50%;
  background: var(--surface);
  color: var(--text);
  display: grid;
  place-items: center;
  font-size: 8px;
  flex: none;
}
.sha-column {
  font-family: var(--mono);
  font-size: 10px;
  color: var(--muted);
  text-align: right;
}
.merge-icon {
  color: var(--muted);
  flex-shrink: 0;
}
.skeleton-row {
  pointer-events: none;
  padding-right: 30px;
}
.skeleton-row .skeleton {
  height: 8px;
  width: 75%;
}
.history-empty {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  justify-content: center;
  align-items: center;
  gap: 13px;
  pointer-events: none;
  color: var(--muted);
  font-size: 12px;
}
.history-empty strong {
  font-weight: 500;
  color: var(--text);
}
@container history (max-width: 620px) {
  .history {
    --graph-width: 92px;
  }
  .commit-grid {
    grid-template-columns: var(--graph-width) minmax(0, 1fr) 64px;
    gap: 9px;
  }
  .author-column,
  .date-column {
    display: none;
  }
}
@container history (max-width: 480px) {
  .ref-badge {
    display: none;
  }
}
</style>
