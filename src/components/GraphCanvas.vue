<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import type { CommitRow } from '../lib/types'

const props = defineProps<{
  items: { row: CommitRow; start: number }[]
  scrollTop: number
  width: number
  height: number
  rowHeight: number
  selectedSha: string
  filtered: boolean
  laneOffset: number
}>()
const canvas = ref<HTMLCanvasElement>()
let palette: { colors: string[]; node: string; selectedNode: string; halo: string } | undefined
let themeObserver: MutationObserver | undefined
let frame = 0

function readPalette() {
  const style = getComputedStyle(document.documentElement)
  const token = (name: string) => style.getPropertyValue(name).trim()
  palette = {
    colors: Array.from({ length: 8 }, (_, index) => token(`--graph-${index}`)),
    node: token('--bg'),
    selectedNode: token('--text'),
    halo: token('--accent'),
  }
}

function draw() {
  const element = canvas.value
  if (!element || !palette) return
  const colors = palette.colors
  const ratio = window.devicePixelRatio || 1
  element.width = Math.round(props.width * ratio)
  element.height = Math.round(props.height * ratio)
  const context = element.getContext('2d')
  if (!context) return
  context.scale(ratio, ratio)
  context.lineWidth = 1.65
  context.lineCap = 'round'
  const x = (lane: number) => 22 + (lane - props.laneOffset) * 18
  for (const { row, start } of props.items) {
    const top = start - props.scrollTop
    const middle = top + props.rowHeight / 2
    const bottom = top + props.rowHeight
    if (!props.filtered) {
      for (const edge of row.graph.edges) {
        const from = x(edge.from)
        const to = x(edge.to)
        const y1 = edge.kind === 'parent' ? middle : top
        const y2 = edge.kind === 'incoming' ? middle : bottom
        context.strokeStyle = colors[edge.color % colors.length]!
        context.globalAlpha = 0.68
        context.beginPath()
        context.moveTo(from, y1)
        if (from === to) context.lineTo(to, y2)
        else context.bezierCurveTo(from, middle, to, middle, to, y2)
        context.stroke()
      }
    }
    context.globalAlpha = 1
    const nodeX = props.filtered ? 22 : x(row.graph.lane)
    const selected = row.sha === props.selectedSha
    if (selected) {
      context.fillStyle = palette.halo
      context.globalAlpha = 0.18
      context.beginPath()
      context.arc(nodeX, middle, 9, 0, Math.PI * 2)
      context.fill()
      context.globalAlpha = 1
    }
    context.fillStyle = selected ? palette.selectedNode : palette.node
    context.strokeStyle = colors[row.graph.color % colors.length]!
    context.lineWidth = selected ? 2.3 : 1.9
    context.beginPath()
    context.arc(nodeX, middle, row.parents.length > 1 ? 4.8 : 3.8, 0, Math.PI * 2)
    context.fill()
    context.stroke()
    context.lineWidth = 1.65
  }
}

function schedule() {
  cancelAnimationFrame(frame)
  frame = requestAnimationFrame(draw)
}
onMounted(() => {
  readPalette()
  schedule()
  themeObserver = new MutationObserver(() => {
    readPalette()
    schedule()
  })
  themeObserver.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ['data-theme'],
  })
})
watch(
  () => [
    props.items,
    props.scrollTop,
    props.width,
    props.height,
    props.selectedSha,
    props.filtered,
    props.laneOffset,
  ],
  schedule,
)
onBeforeUnmount(() => {
  cancelAnimationFrame(frame)
  themeObserver?.disconnect()
})
</script>

<template>
  <canvas
    ref="canvas"
    class="graph-canvas"
    :style="{ width: width + 'px', height: height + 'px' }"
    aria-hidden="true"
  />
</template>

<style scoped>
.graph-canvas {
  position: absolute;
  inset: 0 auto auto 0;
  pointer-events: none;
  z-index: 2;
}
</style>
