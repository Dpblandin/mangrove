<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import {
  Check,
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

const props = defineProps<{
  references: Reference[]
  collapsed: ReadonlySet<string>
  filter: string
  currentName?: string
  selectedSha: string | null
}>()
const emit = defineEmits<{
  select: [reference: Reference]
  toggleFolder: [path: string]
}>()
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
      <button
        v-else
        class="reference-item"
        :style="{ paddingLeft: `min(${26 + depth * 14}px, 35%)` }"
        :class="{
          current: node.reference.name === currentName,
          highlighted: node.reference.sha === selectedSha,
        }"
        :title="node.reference.label"
        :aria-label="node.reference.label"
        @click="emit('select', node.reference)"
      >
        <component :is="icons[node.reference.kind]" :size="13" />
        <span>{{ node.label }}</span>
        <Check v-if="node.reference.name === currentName" :size="12" />
      </button>
    </template>
  </div>
</template>
