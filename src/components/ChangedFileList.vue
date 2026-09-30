<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, useId, watch } from 'vue'
import { useVirtualizer } from '@tanstack/vue-virtual'
import { count, filename, fileStatusLabel } from '../lib/format'
import type { ChangedFile, WorkingTreeFile } from '../lib/types'
import { fontSizeOffset } from '../lib/fontSize'
import FileStatusIcon from './FileStatusIcon.vue'

const props = defineProps<{
  files: ChangedFile[]
  fileCount: number
  commitSha: string
  selectedFile: ChangedFile | null
  loading: boolean
}>()
const emit = defineEmits<{
  selectFile: [file: ChangedFile]
}>()

const rowHeight = computed(() => 32 + fontSizeOffset.value * 2)
const container = ref<HTMLElement>()
const activeIndex = ref(0)
const id = useId()
function fileKey(file: ChangedFile | WorkingTreeFile) {
  return 'key' in file ? file.key : file.index
}
const selectedIndex = computed(() =>
  props.selectedFile
    ? props.files.findIndex((file) => fileKey(file) === fileKey(props.selectedFile!))
    : -1,
)
const virtualizer = useVirtualizer(
  computed(() => {
    const files = props.files
    const sha = props.commitSha
    return {
      count: files.length,
      getScrollElement: () => container.value ?? null,
      estimateSize: () => rowHeight.value,
      getItemKey: (index: number) => `${sha}:${files[index] ? fileKey(files[index]!) : index}`,
      overscan: 6,
    }
  }),
)
const items = computed(() => virtualizer.value.getVirtualItems())
const rows = computed(() =>
  items.value.flatMap((item) => {
    const file = props.files[item.index]
    return file ? [{ item, file }] : []
  }),
)
const activeDescendant = computed(() =>
  rows.value.some(({ item }) => item.index === activeIndex.value)
    ? `${id}-file-${activeIndex.value}`
    : undefined,
)

watch(
  selectedIndex,
  (index) => {
    if (index >= 0) activeIndex.value = index
  },
  { immediate: true },
)
let sizeRevision = 0
watch(
  [() => props.commitSha, rowHeight],
  async ([sha, height], [previousSha, previousHeight]) => {
    const revision = ++sizeRevision
    const changedCommit = sha !== previousSha
    if (changedCommit) activeIndex.value = Math.max(0, selectedIndex.value)
    const offset = changedCommit ? 0 : (container.value?.scrollTop ?? 0)
    // Invalidate cached estimates only for typography changes. A new commit always starts at zero.
    if (height !== previousHeight) virtualizer.value.measure()
    await nextTick()
    if (revision === sizeRevision && sha === props.commitSha && container.value)
      virtualizer.value.scrollToOffset((offset / previousHeight) * height)
  },
  { flush: 'post' },
)
onBeforeUnmount(() => {
  sizeRevision += 1
})

function directory(path: string) {
  const lastSeparator = path.lastIndexOf('/')
  return lastSeparator < 0 ? '' : path.slice(0, lastSeparator + 1)
}

function choose(index: number) {
  const file = props.files[index]
  if (!file || props.loading) return
  activeIndex.value = index
  container.value?.focus({ preventScroll: true })
  virtualizer.value.scrollToIndex(index, { align: 'auto' })
  emit('selectFile', file)
}

function keydown(event: KeyboardEvent) {
  if (props.loading) return
  const page = Math.max(
    1,
    Math.floor((container.value?.clientHeight ?? rowHeight.value) / rowHeight.value),
  )
  let target: number
  switch (event.key) {
    case 'ArrowDown':
      target = activeIndex.value + 1
      break
    case 'ArrowUp':
      target = activeIndex.value - 1
      break
    case 'PageDown':
      target = activeIndex.value + page
      break
    case 'PageUp':
      target = activeIndex.value - page
      break
    case 'Home':
      target = 0
      break
    case 'End':
      target = props.files.length - 1
      break
    case 'Enter':
    case ' ':
      target = activeIndex.value
      break
    default:
      return
  }
  event.preventDefault()
  const bounded = Math.max(0, Math.min(props.files.length - 1, target))
  if (bounded === activeIndex.value && event.key !== 'Enter' && event.key !== ' ') return
  choose(bounded)
}
</script>

<template>
  <div
    ref="container"
    class="file-scroll min-h-0 flex-1 overflow-y-auto px-[7px] pb-[9px]"
    role="listbox"
    aria-label="Fichiers modifiés"
    :aria-activedescendant="activeDescendant"
    :aria-busy="loading"
    tabindex="0"
    @keydown="keydown"
  >
    <div class="relative min-w-0" :style="{ height: virtualizer.getTotalSize() + 'px' }">
      <button
        v-for="{ item, file } in rows"
        :key="String(item.key)"
        class="file-row absolute top-0 left-0 flex w-full cursor-pointer items-center gap-2 rounded-[5px] border border-transparent bg-transparent px-2.5 py-1 text-left text-text hover:bg-surface aria-selected:border-accent/25 aria-selected:bg-accent/10 disabled:cursor-default disabled:opacity-100"
        :class="{ 'file-keyboard-active': item.index === activeIndex }"
        :style="{ height: rowHeight + 'px', transform: 'translateY(' + item.start + 'px)' }"
        :id="`${id}-file-${item.index}`"
        type="button"
        role="option"
        tabindex="-1"
        :disabled="loading"
        :aria-selected="item.index === selectedIndex"
        :aria-posinset="item.index + 1"
        :aria-setsize="fileCount"
        :aria-label="file.path + ', ' + fileStatusLabel(file.status)"
        :title="
          (file.oldPath ? `${file.oldPath} → ${file.path}` : file.path) +
          ', ' +
          fileStatusLabel(file.status)
        "
        @click="choose(item.index)"
      >
        <FileStatusIcon :status="file.status" class="shrink-0" />
        <span
          class="flex min-w-0 flex-1 items-baseline text-[calc(11px_+_var(--font-size-offset))]"
        >
          <span v-if="directory(file.path)" class="min-w-0 truncate text-muted">{{
            directory(file.path)
          }}</span>
          <strong class="max-w-full shrink-0 truncate font-normal">{{
            filename(file.path)
          }}</strong>
        </span>
      </button>
    </div>
    <p
      v-if="!files.length"
      class="mx-[11px] my-1.5 text-[calc(11px_+_var(--font-size-offset))] leading-relaxed text-muted"
    >
      Aucun fichier modifié dans cette comparaison.
    </p>
    <p
      v-if="files.length < fileCount"
      class="mx-[11px] my-1.5 text-[calc(11px_+_var(--font-size-offset))] leading-relaxed text-muted"
    >
      Les {{ count(files.length) }} premiers fichiers sur {{ count(fileCount) }} sont affichés.
    </p>
  </div>
</template>

<style scoped>
.file-scroll {
  outline: none;
}
.file-scroll:focus-visible {
  box-shadow: inset 0 0 0 1px var(--accent);
}
.file-scroll:focus-visible .file-keyboard-active {
  outline: 1px solid var(--accent);
  outline-offset: -2px;
}
</style>
