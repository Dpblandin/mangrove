<script setup lang="ts">
import { computed, ref } from 'vue'
import { FilePenLine } from '@lucide/vue'
import { count } from '../lib/format'
import { fileStatusPresentation } from '../lib/fileStatus'
import type { FileStatus, WorkingTreeStatus } from '../lib/types'
import FileStatusIcon from './FileStatusIcon.vue'

const props = defineProps<{ status: WorkingTreeStatus | null; selected: boolean }>()
const emit = defineEmits<{ select: []; navigate: [] }>()
const button = ref<HTMLButtonElement>()
defineExpose({ focus: () => button.value?.focus({ preventScroll: true }) })
const changeCount = computed(
  () => (props.status?.stagedCount ?? 0) + (props.status?.unstagedCount ?? 0),
)
const truncated = computed(
  () =>
    props.status &&
    (props.status.staged.length < props.status.stagedCount ||
      props.status.unstaged.length < props.status.unstagedCount),
)
const statusOrder: FileStatus[] = [
  'modified',
  'added',
  'deleted',
  'renamed',
  'copied',
  'typechange',
  'unknown',
  'unmodified',
]
const counts = computed(() => {
  const totals = new Map<FileStatus, number>()
  for (const files of [props.status?.staged ?? [], props.status?.unstaged ?? []]) {
    for (const file of files) totals.set(file.status, (totals.get(file.status) ?? 0) + 1)
  }
  return statusOrder.flatMap((status) => {
    const total = totals.get(status) ?? 0
    return total ? [{ status, total }] : []
  })
})
</script>

<template>
  <button
    ref="button"
    class="working-tree-row"
    :class="{ selected }"
    type="button"
    :aria-pressed="selected"
    :aria-label="`Modifications non commitées, ${count(changeCount)} changements`"
    @click="emit('select')"
    @keydown.down.prevent="emit('navigate')"
  >
    <FilePenLine :size="17" class="working-icon" />
    <strong>Modifications non commitées</strong>
    <span v-if="!truncated" class="change-counts" aria-hidden="true">
      <span
        v-for="{ status, total } in counts"
        :key="status"
        :class="fileStatusPresentation(status).colorClass"
      >
        <FileStatusIcon :status="status" :size="13" />
        {{ count(total) }}
      </span>
    </span>
    <span class="working-count">{{ count(changeCount) }}</span>
  </button>
</template>

<style scoped>
.working-tree-row {
  display: flex;
  flex: none;
  align-items: center;
  min-height: 44px;
  gap: 10px;
  padding: 9px 16px 9px 20px;
  border: 0;
  border-bottom: 1px solid var(--border);
  background: var(--accent-soft);
  color: var(--text);
  cursor: pointer;
  text-align: left;
  font-size: calc(12px + var(--font-size-offset));
}
.working-tree-row:hover {
  background: var(--hover);
}
.working-tree-row.selected {
  background: var(--selection);
  box-shadow: inset 2px 0 0 var(--accent);
}
.working-tree-row:focus-visible {
  outline: 1px solid var(--accent);
  outline-offset: -2px;
}
.working-icon {
  flex: none;
  color: var(--accent);
}
.working-tree-row strong {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-weight: 500;
}
.change-counts {
  display: flex;
  gap: 10px;
  font-family: var(--mono);
  font-size: calc(10px + var(--font-size-offset));
  white-space: nowrap;
}
.change-counts > span {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}
.working-count {
  margin-left: auto;
  color: var(--muted);
  font-family: var(--mono);
  font-size: calc(10px + var(--font-size-offset));
}
</style>
