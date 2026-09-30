<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { Focus } from '@lucide/vue'
import type { Reference } from '../lib/types'

const props = defineProps<{
  references: Reference[]
  soloRefs?: ReadonlySet<string>
}>()
const emit = defineEmits<{ solo: [reference: Reference] }>()
const menu = ref<{
  references: Reference[]
  x: number
  y: number
  origin: HTMLElement | null
} | null>(null)
const root = ref<HTMLElement>()
const branchesByName = computed(
  () =>
    new Map(
      props.references
        .filter((reference) => reference.kind === 'local' || reference.kind === 'remote')
        .map((reference) => [reference.name, reference]),
    ),
)

function close(restore = false) {
  if (restore && menu.value?.origin?.isConnected) menu.value.origin.focus({ preventScroll: true })
  menu.value = null
}

function currentBranch(reference: Reference) {
  const current = branchesByName.value.get(reference.name)
  return current?.sha === reference.sha ? current : undefined
}

function open(event: MouseEvent | KeyboardEvent, references: Reference[], anchor?: HTMLElement) {
  const branches = references.flatMap((reference) => {
    const current = currentBranch(reference)
    return current ? [current] : []
  })
  if (!branches.length) return
  event.preventDefault()
  event.stopPropagation()
  const target = event.currentTarget instanceof HTMLElement ? event.currentTarget : null
  // The history uses a single listbox focus with aria-activedescendant. Opening
  // a menu on an unselected row must not leave DOM focus on that row afterward.
  const origin =
    target?.closest<HTMLElement>('[role="listbox"]') ??
    target?.closest<HTMLElement>('button, [tabindex]') ??
    target
  const rect = (anchor ?? target)?.getBoundingClientRect()
  menu.value = {
    references: branches,
    origin,
    x: 'clientX' in event ? event.clientX : (rect?.left ?? 0),
    y: 'clientY' in event ? event.clientY : (rect?.bottom ?? 0),
  }
  const opened = menu.value
  void nextTick(() => {
    if (menu.value !== opened || !root.value) return
    const bounds = root.value.getBoundingClientRect()
    opened.x = Math.max(8, Math.min(opened.x, window.innerWidth - bounds.width - 8))
    opened.y = Math.max(8, Math.min(opened.y, window.innerHeight - bounds.height - 8))
    root.value.querySelector('button')?.focus({ preventScroll: true })
  })
}

function select(reference: Reference) {
  const current = currentBranch(reference)
  close(true)
  if (current) emit('solo', current)
}

function keyboard(event: KeyboardEvent) {
  if (event.key === 'Escape') {
    event.preventDefault()
    close(true)
  } else if (event.key === 'Tab') {
    close(true)
  } else if (['ArrowUp', 'ArrowDown', 'Home', 'End'].includes(event.key)) {
    event.preventDefault()
    const buttons = [...(root.value?.querySelectorAll('button') ?? [])]
    const current = buttons.indexOf(document.activeElement as HTMLButtonElement)
    const index =
      event.key === 'Home'
        ? 0
        : event.key === 'End'
          ? buttons.length - 1
          : (current + (event.key === 'ArrowDown' ? 1 : -1) + buttons.length) % buttons.length
    buttons[index]?.focus()
  }
}

function outside(event: PointerEvent) {
  if (event.target instanceof Node && !root.value?.contains(event.target)) close()
}
function scrolled(event: Event) {
  if (!(event.target instanceof Node) || !root.value?.contains(event.target)) close(true)
}
const dismiss = () => close()
onMounted(() => {
  document.addEventListener('pointerdown', outside)
  document.addEventListener('scroll', scrolled, true)
  window.addEventListener('resize', dismiss)
})
onBeforeUnmount(() => {
  close()
  document.removeEventListener('pointerdown', outside)
  document.removeEventListener('scroll', scrolled, true)
  window.removeEventListener('resize', dismiss)
})
watch(() => props.references, dismiss, { flush: 'sync' })
defineExpose({
  open,
  close,
  isOpen: (name: string) => menu.value?.references.some((ref) => ref.name === name) ?? false,
})
</script>

<template>
  <Teleport to="body">
    <div
      v-if="menu"
      ref="root"
      class="branch-context-menu"
      role="menu"
      :aria-label="`Actions des branches ${menu.references.map((ref) => ref.label).join(', ')}`"
      :style="{ left: menu.x + 'px', top: menu.y + 'px' }"
      @keydown.stop="keyboard"
    >
      <button
        v-for="reference in menu.references"
        :key="reference.name"
        role="menuitem"
        @click="select(reference)"
      >
        <Focus :size="14" aria-hidden="true" />
        <span>{{
          soloRefs?.has(reference.name)
            ? 'Retirer du Solo'
            : soloRefs?.size
              ? 'Ajouter au Solo'
              : 'Solo'
        }}</span>
        <small v-if="menu.references.length > 1" :title="reference.name"
          >{{ reference.kind === 'local' ? 'Locale' : 'Distante' }} · {{ reference.label }}</small
        >
      </button>
    </div>
  </Teleport>
</template>

<style scoped>
.branch-context-menu {
  position: fixed;
  z-index: 100;
  min-width: min(210px, calc(100vw - 16px));
  width: max-content;
  max-width: calc(100vw - 16px);
  max-height: calc(100vh - 16px);
  overflow: auto;
  padding: 5px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--panel);
  box-shadow: 0 8px 25px #0008;
}
.branch-context-menu button {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 8px 10px;
  border: 0;
  border-radius: 4px;
  background: transparent;
  font-size: calc(11px + var(--font-size-offset));
  cursor: pointer;
}
.branch-context-menu button > svg,
.branch-context-menu button > span {
  flex-shrink: 0;
}
.branch-context-menu small {
  margin-left: auto;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--muted);
}
.branch-context-menu button:hover,
.branch-context-menu button:focus-visible {
  background: var(--accent-soft);
  outline: 1px solid var(--accent);
}
</style>
