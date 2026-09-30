<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
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
const menu = ref<{ reference: Reference; x: number; y: number; origin: HTMLElement | null } | null>(
  null,
)
const menuButton = ref<HTMLButtonElement>()
function closeMenu(restore = false) {
  if (restore) menu.value?.origin?.focus()
  menu.value = null
}
function outside(event: PointerEvent) {
  if (!(event.target instanceof Element) || !event.target.closest('.reference-menu')) closeMenu()
}
function canSolo(reference: Reference) {
  return reference.kind === 'local' || reference.kind === 'remote'
}
function openMenu(event: MouseEvent | KeyboardEvent, reference: Reference) {
  if (!canSolo(reference)) return
  event.preventDefault()
  const origin = event.currentTarget instanceof HTMLElement ? event.currentTarget : null
  const rect = origin?.getBoundingClientRect()
  menu.value = {
    reference,
    origin,
    x: Math.max(
      0,
      Math.min('clientX' in event ? event.clientX : (rect?.left ?? 0), window.innerWidth - 220),
    ),
    y: Math.max(
      0,
      Math.min('clientY' in event ? event.clientY : (rect?.bottom ?? 0), window.innerHeight - 55),
    ),
  }
  void nextTick(() => menuButton.value?.focus())
}
function selectSolo() {
  if (!menu.value) return
  const reference = menu.value.reference
  closeMenu(true)
  emit('solo', reference)
}
function menuKeyboard(event: KeyboardEvent) {
  if (event.key === 'Escape') {
    event.preventDefault()
    event.stopPropagation()
    closeMenu(true)
  } else if (event.key === 'Tab') {
    closeMenu(true)
  } else if (['ArrowUp', 'ArrowDown', 'Home', 'End'].includes(event.key)) {
    event.preventDefault()
    menuButton.value?.focus()
  }
}
onMounted(() => document.addEventListener('pointerdown', outside))
onBeforeUnmount(() => document.removeEventListener('pointerdown', outside))
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
watch(rows, () => closeMenu())

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
            canSolo(node.reference) ? menu?.reference.name === node.reference.name : undefined
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
  <Teleport to="body">
    <div
      v-if="menu"
      class="reference-menu"
      role="menu"
      :aria-label="`Actions de la branche ${menu.reference.label}`"
      :style="{ left: menu.x + 'px', top: menu.y + 'px' }"
      @keydown="menuKeyboard"
    >
      <button ref="menuButton" role="menuitem" @click="selectSolo">
        <Focus :size="14" />{{
          soloRefs?.has(menu.reference.name)
            ? 'Retirer du Solo'
            : soloRefs?.size
              ? 'Ajouter au Solo'
              : 'Solo'
        }}
      </button>
    </div>
  </Teleport>
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
.reference-menu {
  position: fixed;
  z-index: 100;
  min-width: 210px;
  padding: 5px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--panel);
  box-shadow: 0 8px 25px #0008;
}
.reference-menu button {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 8px 10px;
  border: 0;
  border-radius: 4px;
  background: transparent;
  font-size: 12px;
  cursor: pointer;
}
.reference-menu button:hover,
.reference-menu button:focus-visible {
  background: var(--accent-soft);
  outline: 1px solid var(--accent);
}
</style>
