<script setup lang="ts">
import { ChevronDown, ChevronRight, FolderGit2, LockKeyhole, Archive } from '@lucide/vue'
import { commitDate, shortSha } from '../lib/format'
import type { WorktreeList, StashList } from '../lib/types'
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
</script>
<template>
  <div class="repository-context">
    <button class="group-title" :aria-expanded="worktreesExpanded" @click="emit('toggleWorktrees')">
      <ChevronDown v-if="worktreesExpanded" :size="12" /><ChevronRight v-else :size="12" />
      <span>Worktrees</span><small v-if="worktrees">{{ worktrees.worktrees.length }}</small>
    </button>
    <div v-if="worktreesExpanded" :aria-busy="worktreesLoading">
      <p v-if="worktreesError" class="context-notice text-danger" role="alert">
        {{ worktreesError }}
      </p>
      <p v-else-if="!worktrees && worktreesLoading" class="context-notice">
        Lecture des worktrees…
      </p>
      <button
        v-for="item in worktrees?.worktrees"
        :key="item.path"
        class="context-item"
        :disabled="!item.available"
        :aria-current="item.current ? 'location' : undefined"
        :title="[item.path, item.reason, item.locked].filter(Boolean).join('\n')"
        @click="emit('openWorktree', item.path)"
      >
        <FolderGit2 :size="13" class="shrink-0" />
        <span
          ><strong>{{ item.primary ? 'Principal' : item.name }}</strong
          ><small
            >{{ item.headName }} ·
            {{ item.current ? 'ouvert ici' : item.primary ? 'principal' : 'lié' }}</small
          ><small v-if="item.detached && item.head">{{ shortSha(item.head) }}</small
          ><small :title="item.path">{{ item.path }}</small
          ><small :class="{ 'text-warning': !item.available || item.locked !== null }"
            >{{ item.available ? 'Disponible' : 'Indisponible' }}
            <template v-if="item.locked !== null"> · verrouillé</template></small
          ><small v-if="item.locked" class="context-reason text-warning">{{ item.locked }}</small
          ><small v-if="item.reason" class="context-reason text-warning">{{
            item.reason
          }}</small></span
        >
        <LockKeyhole
          v-if="item.locked !== null"
          :size="12"
          :aria-label="`Verrouillé : ${item.locked}`"
          class="shrink-0 text-warning"
        />
      </button>
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
.context-item .context-reason {
  white-space: normal;
  overflow-wrap: anywhere;
}
.context-notice {
  margin: 8px 12px;
  line-height: 1.5;
  overflow-wrap: anywhere;
}
</style>
