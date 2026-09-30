<script setup lang="ts">
import { onBeforeUnmount, shallowRef } from 'vue'

const props = defineProps<{
  value: number
  min: number
  max: number
  direction: -1 | 1
  label: string
  controls: string
}>()
const emit = defineEmits<{
  resize: [value: number]
  start: []
  finish: [cancelled: boolean]
  reset: []
}>()
const drag = shallowRef<{ pointer: number; x: number; width: number; target: HTMLElement } | null>(
  null,
)

function update(value: number) {
  emit('resize', Math.max(props.min, Math.min(props.max, value)))
}

function pointerdown(event: PointerEvent) {
  if (event.button !== 0 || !event.isPrimary) return
  const target = event.currentTarget as HTMLElement
  event.preventDefault()
  target.focus({ preventScroll: true })
  target.setPointerCapture(event.pointerId)
  drag.value = { pointer: event.pointerId, x: event.clientX, width: props.value, target }
  emit('start')
}

function pointermove(event: PointerEvent) {
  const active = drag.value
  if (!active || active.pointer !== event.pointerId) return
  event.preventDefault()
  update(active.width + (event.clientX - active.x) * props.direction)
}

function finish(cancelled: boolean, event?: PointerEvent) {
  const active = drag.value
  if (!active || (event && active.pointer !== event.pointerId)) return
  drag.value = null
  if (active.target.hasPointerCapture(active.pointer))
    active.target.releasePointerCapture(active.pointer)
  emit('finish', cancelled)
}

function keyboard(event: KeyboardEvent) {
  if (event.key === 'Escape' && drag.value) {
    event.preventDefault()
    finish(true)
    return
  }
  const step = event.shiftKey ? 50 : 10
  let value: number
  if (event.key === 'ArrowLeft') value = props.value - step * props.direction
  else if (event.key === 'ArrowRight') value = props.value + step * props.direction
  else if (event.key === 'Home') value = props.min
  else if (event.key === 'End') value = props.max
  else return
  event.preventDefault()
  update(value)
}

onBeforeUnmount(() => finish(true))
</script>

<template>
  <div
    class="pane-divider"
    :class="{ dragging: drag }"
    role="separator"
    aria-orientation="vertical"
    :aria-label="label"
    :aria-controls="controls"
    :aria-valuemin="Math.round(min)"
    :aria-valuemax="Math.round(max)"
    :aria-valuenow="Math.round(value)"
    :aria-valuetext="`${Math.round(value)} pixels`"
    title="Glisser ou utiliser les flèches pour redimensionner. Double clic pour réinitialiser."
    tabindex="0"
    @pointerdown="pointerdown"
    @pointermove="pointermove"
    @pointerup="finish(false, $event)"
    @pointercancel="finish(true, $event)"
    @lostpointercapture="finish(true, $event)"
    @keydown="keyboard"
    @dblclick.prevent="emit('reset')"
  ></div>
</template>

<style scoped>
.pane-divider {
  position: relative;
  z-index: 3;
  width: 6px;
  flex: none;
  align-self: stretch;
  background: var(--bg);
  cursor: col-resize;
  touch-action: none;
  user-select: none;
}
.pane-divider::after {
  content: '';
  position: absolute;
  inset: 0;
  width: 1px;
  margin-inline: auto;
  background: var(--border);
}
.pane-divider:hover::after,
.pane-divider:focus-visible::after,
.pane-divider.dragging::after {
  background: var(--accent);
}
.pane-divider:focus-visible {
  outline: 1px solid var(--accent);
  outline-offset: -1px;
}
</style>
