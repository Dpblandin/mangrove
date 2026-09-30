<script setup lang="ts">
import { Minus, Plus } from '@lucide/vue'
import {
  diffFontSize,
  MAX_DIFF_FONT_SIZE,
  MIN_DIFF_FONT_SIZE,
  setDiffFontSize,
} from '../lib/fontSize'
</script>

<template>
  <div class="diff-font-controls" role="group" aria-label="Taille du texte du diff">
    <button
      type="button"
      class="icon-button"
      aria-label="Diminuer la taille du texte du diff"
      title="Diminuer la taille du texte du diff"
      :disabled="diffFontSize <= MIN_DIFF_FONT_SIZE"
      @click="setDiffFontSize(diffFontSize - 1)"
    >
      <Minus :size="14" aria-hidden="true" />
    </button>
    <span class="diff-font-value" aria-live="polite" aria-atomic="true">{{ diffFontSize }} px</span>
    <button
      type="button"
      class="icon-button"
      aria-label="Augmenter la taille du texte du diff"
      title="Augmenter la taille du texte du diff"
      :disabled="diffFontSize >= MAX_DIFF_FONT_SIZE"
      @click="setDiffFontSize(diffFontSize + 1)"
    >
      <Plus :size="14" aria-hidden="true" />
    </button>
  </div>
</template>

<style scoped>
.diff-font-controls {
  display: inline-flex;
  flex-shrink: 0;
  align-items: center;
  gap: 2px;
}
.diff-font-value {
  min-width: 32px;
  text-align: center;
  font-size: calc(10px + var(--font-size-offset));
  font-variant-numeric: tabular-nums;
  color: var(--muted);
}
.icon-button:disabled {
  cursor: default;
  opacity: 0.4;
}
.icon-button:disabled:hover {
  background: transparent;
  color: var(--muted);
}
</style>
