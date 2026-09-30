<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import {
  Check,
  Focus,
  ChevronDown,
  ChevronRight,
  Folder,
  FolderOpen,
  GitBranch,
  Globe,
  Layers,
  Tag,
} from '@lucide/vue'
import { buildReferenceTree, visibleReferenceRows } from '../lib/referenceTree'
import { count } from '../lib/format'
import type { Reference } from '../lib/types'
import BranchContextMenu from './BranchContextMenu.vue'

const props = defineProps<{
  references: Reference[]
  collapsed: ReadonlySet<string>
  filter: string
  currentName?: string
  soloRefs?: ReadonlySet<string>
  selectedSha: string | null
}>()
const emit = defineEmits<{
  select: [reference: Reference]
  toggleFolder: [path: string]
  solo: [reference: Reference]
}>()
const branchMenu = ref<InstanceType<typeof BranchContextMenu>>()
function canSolo(reference: Reference) {
  return reference.kind === 'local' || reference.kind === 'remote'
}
function openMenu(event: MouseEvent | KeyboardEvent, reference: Reference) {
  branchMenu.value?.open(event, [reference])
}
const icons = { local: GitBranch, remote: Globe, tag: Tag, other: Layers }
const searchCollapsed = ref(new Set<string>())
watch(
  () => props.filter,
  () => {
    searchCollapsed.value = new Set()
  },
)
const searching = computed(() => Boolean(props.filter.trim()))
const closed = computed(() => (searching.value ? searchCollapsed.value : props.collapsed))
const tree = computed(() => buildReferenceTree(props.references))
const rows = computed(() => visibleReferenceRows(tree.value, closed.value))
watch(rows, () => branchMenu.value?.close())

function toggle(path: string) {
  if (!searching.value) {
    emit('toggleFolder', path)
    return
  }
  const next = new Set(searchCollapsed.value)
  if (next.has(path)) next.delete(path)
  else next.add(path)
  searchCollapsed.value = next
}
</script>

<template>
  <div class="reference-items">
    <template v-for="{ node, depth } in rows" :key="`${node.kind}:${node.path}`">
      <button
        v-if="node.kind === 'folder'"
        class="reference-item reference-folder"
        :style="{ paddingLeft: `min(${12 + depth * 14}px, calc(35% - 14px))` }"
        :aria-expanded="!closed.has(node.path)"
        :aria-label="`Dossier ${node.path}`"
        :title="node.path"
        @click="toggle(node.path)"
      >
        <ChevronRight v-if="closed.has(node.path)" :size="12" />
        <ChevronDown v-else :size="12" />
        <Folder v-if="closed.has(node.path)" :size="13" />
        <FolderOpen v-else :size="13" />
        <span>{{ node.label }}</span
        ><small>{{ count(node.count) }}</small>
      </button>
      <div v-else class="reference-leaf">
        <button
          class="reference-item"
          :style="{ paddingLeft: `min(${26 + depth * 14}px, 35%)` }"
          :class="{
            current: node.reference.name === currentName,
            highlighted: node.reference.sha === selectedSha,
            soloed: soloRefs?.has(node.reference.name),
          }"
          :title="node.reference.label"
          :aria-label="node.reference.label"
          :aria-haspopup="canSolo(node.reference) ? 'menu' : undefined"
          :aria-expanded="
            canSolo(node.reference) ? branchMenu?.isOpen(node.reference.name) : undefined
          "
          @click="emit('select', node.reference)"
          @contextmenu="openMenu($event, node.reference)"
          @keydown.shift.f10="openMenu($event, node.reference)"
          @keydown="$event.key === 'ContextMenu' && openMenu($event, node.reference)"
        >
          <component :is="icons[node.reference.kind]" :size="13" />
          <span>{{ node.label }}</span>
          <Check v-if="node.reference.name === currentName" :size="12" />
        </button>
        <button
          v-if="canSolo(node.reference)"
          class="solo-toggle"
          :aria-pressed="soloRefs?.has(node.reference.name) ?? false"
          :aria-label="`${soloRefs?.has(node.reference.name) ? 'Retirer du Solo' : 'Solo'} : ${node.reference.label}`"
          :title="soloRefs?.has(node.reference.name) ? 'Retirer du Solo' : 'Solo / ajouter au Solo'"
          @click="emit('solo', node.reference)"
        >
          <Focus :size="13" />
        </button>
      </div>
    </template>
  </div>
  <BranchContextMenu
    ref="branchMenu"
    :references="references"
    :solo-refs="soloRefs"
    @solo="emit('solo', $event)"
  />
</template>
<style scoped>
.reference-leaf {
  display: flex;
  align-items: center;
  min-width: 0;
}
.reference-leaf .reference-item {
  flex: 1;
  min-width: 0;
}
.solo-toggle {
  display: grid;
  place-items: center;
  flex-shrink: 0;
  width: 24px;
  height: 26px;
  border: 0;
  border-radius: 4px;
  background: transparent;
  color: var(--muted);
  cursor: pointer;
  opacity: 0;
  pointer-events: none;
}
.reference-leaf:hover .solo-toggle,
.reference-leaf:focus-within .solo-toggle {
  opacity: 1;
  pointer-events: auto;
}
@media (hover: none) {
  .solo-toggle {
    opacity: 1;
    pointer-events: auto;
  }
}
.solo-toggle:hover,
.solo-toggle[aria-pressed='true'],
.soloed {
  color: var(--warning);
  background: color-mix(in srgb, var(--warning) 12%, transparent);
}
.solo-toggle:focus-visible {
  outline: 1px solid var(--accent);
}
</style>
