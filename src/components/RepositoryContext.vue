<script setup lang="ts">
import {
  ChevronDown,
  ChevronRight,
  FolderGit2,
  House,
  Check,
  LockKeyhole,
  TriangleAlert,
  Archive,
} from '@lucide/vue'
import { commitDate, shortSha } from '../lib/format'
import type { WorktreeInfo, WorktreeList, StashList } from '../lib/types'
defineProps<{
  worktrees: WorktreeList | null
  stashes: StashList | null
  worktreesExpanded: boolean
  stashesExpanded: boolean
  worktreesLoading: boolean
  stashesLoading: boolean
  worktreesError: string
  stashesError: string
  selectedStash: string
}>()
const emit = defineEmits<{
  toggleWorktrees: []
  toggleStashes: []
  openWorktree: [path: string]
  selectStash: [sha: string]
}>()

function worktreeLabel(item: WorktreeInfo) {
  return item.detached && item.head ? `${item.headName} · ${shortSha(item.head)}` : item.headName
}

function worktreeDescription(item: WorktreeInfo) {
  return [
    item.path,
    item.primary ? 'Worktree principal' : 'Worktree lié',
    item.current ? 'Ouvert ici' : '',
    item.locked !== null ? `Verrouillé : ${item.locked}` : '',
    item.reason ?? (item.available ? '' : 'Indisponible'),
  ]
    .filter(Boolean)
    .join('\n')
}
</script>
<template>
  <div class="repository-context">
    <button class="group-title" :aria-expanded="worktreesExpanded" @click="emit('toggleWorktrees')">
      <ChevronDown v-if="worktreesExpanded" :size="12" /><ChevronRight v-else :size="12" />
      <FolderGit2 :size="13" />
      <span>Worktrees</span><small v-if="worktrees">{{ worktrees.worktrees.length }}</small>
    </button>
    <div v-if="worktreesExpanded" :aria-busy="worktreesLoading">
      <p v-if="worktreesError" class="context-notice text-danger" role="alert">
        {{ worktreesError }}
      </p>
      <p v-else-if="!worktrees && worktreesLoading" class="context-notice">
        Lecture des worktrees…
      </p>
      <div
        v-for="item in worktrees?.worktrees"
        :key="item.path"
        :title="`${worktreeLabel(item)}\n${worktreeDescription(item)}`"
      >
        <button
          class="context-item worktree-item"
          :class="{ current: item.current }"
          :disabled="!item.available"
          :aria-current="item.current ? 'location' : undefined"
          :aria-label="`${worktreeLabel(item)} · ${worktreeDescription(item)}`"
          @click="emit('openWorktree', item.path)"
        >
          <component :is="item.primary ? House : FolderGit2" :size="13" aria-hidden="true" />
          <span
            ><strong>{{ worktreeLabel(item) }}</strong></span
          >
          <TriangleAlert
            v-if="!item.available"
            :size="12"
            class="text-warning"
            aria-hidden="true"
          />
          <LockKeyhole
            v-if="item.locked !== null"
            :size="12"
            class="text-warning"
            aria-hidden="true"
          />
          <Check v-if="item.current" :size="12" aria-hidden="true" />
        </button>
      </div>
      <p v-if="worktrees?.truncated" class="context-notice">
        Les 256 premiers worktrees sont affichés.
      </p>
    </div>
    <button class="group-title" :aria-expanded="stashesExpanded" @click="emit('toggleStashes')">
      <ChevronDown v-if="stashesExpanded" :size="12" /><ChevronRight v-else :size="12" />
      <span>Stashes</span><small v-if="stashes">{{ stashes.stashes.length }}</small>
    </button>
    <div v-if="stashesExpanded" :aria-busy="stashesLoading">
      <p v-if="stashesError" class="context-notice text-danger" role="alert">{{ stashesError }}</p>
      <p v-else-if="!stashes && stashesLoading" class="context-notice">Lecture des stashes…</p>
      <p v-else-if="stashes && !stashes.stashes.length" class="context-notice">
        Aucun stash dans ce dépôt.
      </p>
      <button
        v-for="item in stashes?.stashes"
        :key="item.sha"
        class="context-item"
        :class="{ selected: item.sha === selectedStash }"
        :title="item.message"
        @click="emit('selectStash', item.sha)"
      >
        <Archive :size="13" class="shrink-0" /><span
          ><strong>stash@{ {{ item.index }} } · {{ item.message.split('\n')[0] }}</strong
          ><small>{{ item.author }} · {{ commitDate(item.time) }}</small
          ><small>Base {{ shortSha(item.baseSha) }}</small></span
        >
      </button>
      <p v-if="stashes?.truncated" class="context-notice">
        Les 128 stashes les plus récents sont affichés.
      </p>
    </div>
  </div>
</template>
<style scoped>
.repository-context {
  border-top: 1px solid var(--border);
  margin-top: 12px;
  padding-top: 7px;
}
.context-item {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  width: 100%;
  border: 0;
  border-radius: 4px;
  padding: 8px 12px;
  background: transparent;
  text-align: left;
  cursor: pointer;
}
.context-item:hover,
.context-item.selected {
  background: var(--accent-soft);
}
.context-item:disabled {
  cursor: default;
  opacity: 0.7;
}
.context-item:focus-visible {
  outline: 1px solid var(--accent);
  outline-offset: -1px;
}
.context-item > span {
  display: flex;
  flex: 1;
  min-width: 0;
  flex-direction: column;
  gap: 4px;
}
.context-item strong,
.context-item small {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.context-item strong {
  font-size: calc(11px + var(--font-size-offset));
  font-weight: 500;
}
.context-item small,
.context-notice {
  font-size: calc(9px + var(--font-size-offset));
  color: var(--muted);
}
.context-item .text-warning {
  color: var(--warning);
}
.worktree-item {
  align-items: center;
  height: 28px;
  padding: 0 15px 0 26px;
  border-radius: 0;
}
.worktree-item > svg {
  flex-shrink: 0;
}
.worktree-item.current {
  color: var(--success);
  background: color-mix(in srgb, var(--success) 18%, transparent);
}
.worktree-item.current strong {
  font-weight: 600;
}
.context-notice {
  margin: 8px 12px;
  line-height: 1.5;
  overflow-wrap: anywhere;
}
</style>
