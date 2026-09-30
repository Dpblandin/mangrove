<script setup lang="ts">
import { computed } from 'vue'
import { fileStatusPresentation } from '../lib/fileStatus'
import type { FileStatus } from '../lib/types'

const props = withDefaults(defineProps<{ status: FileStatus; size?: number }>(), { size: 15 })
const presentation = computed(() => fileStatusPresentation(props.status))
</script>

<template>
  <component
    :is="presentation.icon"
    :size="size"
    :class="presentation.colorClass"
    aria-hidden="true"
    focusable="false"
  />
</template>
