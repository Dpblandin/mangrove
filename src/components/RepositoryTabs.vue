<script setup lang="ts">
import { nextTick, ref, watch } from 'vue'
import { FolderOpen, GitBranch, Plus, X } from '@lucide/vue'
import type { RepositoryTab } from '../composables/useRepositoryTabs'

const props = defineProps<{ tabs: RepositoryTab[]; activeId: string | null }>()
const emit = defineEmits<{ select: [id: string]; close: [id: string]; open: [] }>()
const buttons = new Map<string, HTMLButtonElement>()
const openButton = ref<HTMLButtonElement>()

function setButton(id: string, element: unknown) {
  if (element instanceof HTMLButtonElement) buttons.set(id, element)
  else buttons.delete(id)
}

async function keyboard(event: KeyboardEvent, id: string) {
  if (event.altKey || event.ctrlKey || event.metaKey) return
  const index = props.tabs.findIndex((tab) => tab.id === id)
  if (index === -1 || props.tabs.length === 0) return
  let target: number
  if (event.key === 'ArrowLeft') target = (index - 1 + props.tabs.length) % props.tabs.length
  else if (event.key === 'ArrowRight') target = (index + 1) % props.tabs.length
  else if (event.key === 'Home') target = 0
  else if (event.key === 'End') target = props.tabs.length - 1
  else return
  event.preventDefault()
  const tab = props.tabs[target]!
  emit('select', tab.id)
  await nextTick()
  buttons.get(tab.id)?.focus({ preventScroll: true })
}

async function closeTab(event: MouseEvent, id: string) {
  const wasActive = id === props.activeId
  const focused = event.currentTarget === document.activeElement
  emit('close', id)
  await nextTick()
  if (wasActive || focused) {
    const target = props.activeId ? buttons.get(props.activeId) : openButton.value
    target?.focus({ preventScroll: true })
  }
}

watch(
  () => props.activeId,
  async (id) => {
    await nextTick()
    if (id) buttons.get(id)?.scrollIntoView({ block: 'nearest', inline: 'nearest' })
  },
  { immediate: true },
)
</script>

<template>
  <nav class="repository-tabs" aria-label="Dépôts ouverts">
    <div class="tab-strip" role="tablist" aria-label="Dépôts">
      <div
        v-for="tab in tabs"
        :key="tab.id"
        class="repository-tab"
        :class="{ active: tab.id === activeId }"
        role="presentation"
      >
        <button
          :ref="(element) => setButton(tab.id, element)"
          class="tab-select"
          :id="tab.id"
          role="tab"
          aria-controls="repository-tab-panel"
          :aria-selected="tab.id === activeId"
          :tabindex="tab.id === activeId ? 0 : -1"
          :title="tab.path ?? 'Ouvrir un dépôt'"
          @click="emit('select', tab.id)"
          @keydown="keyboard($event, tab.id)"
        >
          <FolderOpen v-if="tab.path === null" :size="14" aria-hidden="true" />
          <GitBranch v-else :size="14" aria-hidden="true" />
          <span>{{ tab.name }}</span>
        </button>
        <button
          class="tab-close"
          :aria-label="
            tab.path === null ? 'Fermer le nouvel onglet' : `Fermer le dépôt ${tab.name}`
          "
          :tabindex="tab.id === activeId ? 0 : -1"
          :title="`Fermer ${tab.name}`"
          @click="closeTab($event, tab.id)"
        >
          <X :size="13" aria-hidden="true" />
        </button>
      </div>
    </div>
    <button
      ref="openButton"
      class="tab-open"
      aria-label="Nouvel onglet"
      title="Nouvel onglet"
      @click="emit('open')"
    >
      <Plus :size="17" aria-hidden="true" />
    </button>
  </nav>
</template>

<style scoped>
.repository-tabs {
  display: flex;
  flex: none;
  align-items: stretch;
  min-width: 0;
  min-height: 38px;
  padding: 0 12px;
  border-bottom: 1px solid var(--border);
  background: var(--bg);
}
.tab-strip {
  display: flex;
  min-width: 0;
  overflow-x: auto;
  scrollbar-width: thin;
}
.repository-tab {
  display: flex;
  flex: none;
  position: relative;
  align-items: center;
  min-width: 100px;
  max-width: 260px;
  border-right: 1px solid var(--border);
  color: var(--muted);
}
.repository-tab.active {
  background: var(--surface);
  color: var(--text);
  box-shadow: inset 0 2px 0 var(--accent);
}
.tab-select,
.tab-close,
.tab-open {
  display: inline-flex;
  flex: none;
  align-items: center;
  justify-content: center;
  border: 0;
  background: transparent;
  color: inherit;
  cursor: pointer;
}
.tab-select {
  flex: 1;
  min-width: 0;
  gap: 7px;
  padding: 10px 8px 10px 12px;
  text-align: left;
  font-size: calc(12px + var(--font-size-offset));
  font-weight: 500;
}
.tab-select svg {
  flex: none;
}
.tab-select span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.tab-close {
  width: 24px;
  height: 26px;
  margin-right: 5px;
  border-radius: 3px;
}
.tab-open {
  align-self: center;
  width: 32px;
  height: 30px;
  margin-left: 5px;
  border-radius: 3px;
  color: var(--muted);
}
.tab-select:hover,
.tab-close:hover,
.tab-open:hover {
  background: var(--hover);
  color: var(--text);
}
.tab-select:focus-visible,
.tab-close:focus-visible,
.tab-open:focus-visible {
  outline: 1px solid var(--accent);
  outline-offset: -2px;
}
</style>
