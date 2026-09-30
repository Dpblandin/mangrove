<script setup lang="ts">
import { AlertCircle, FileText } from '@lucide/vue'
import type { ChangedFile, FileDiff } from '../lib/types'

defineProps<{
  file: ChangedFile | null
  diff: FileDiff | null
  loading: boolean
  error: string
}>()
</script>

<template>
  <div
    class="diff-viewer flex min-h-0 min-w-0 flex-1 flex-col bg-bg"
    role="region"
    aria-label="Modifications du fichier"
    :aria-busy="loading"
  >
    <template v-if="!loading || diff">
      <div v-if="error" class="diff-state" role="alert">
        <AlertCircle :size="20" class="text-danger" />
        <p>{{ error }}</p>
      </div>
      <div v-else-if="!file" class="diff-state">
        <FileText :size="22" :stroke-width="1.5" />
        <p>Sélectionnez un fichier pour voir ses modifications.</p>
      </div>
      <div v-else-if="diff?.binary" class="diff-state">
        <FileText :size="24" :stroke-width="1.5" /><strong>Fichier binaire</strong>
        <p>Le contenu de ce fichier ne peut pas être affiché sous forme de diff texte.</p>
      </div>
      <div
        v-else-if="diff"
        class="min-h-0 flex-1 overflow-auto pb-3 focus-visible:outline-1 focus-visible:-outline-offset-1 focus-visible:outline-accent"
        tabindex="0"
        aria-label="Diff du fichier, défilement horizontal et vertical"
      >
        <div
          v-if="file.oldPath && file.oldPath !== file.path"
          class="border-b border-border px-[17px] py-[13px] text-[11px] leading-relaxed text-muted [overflow-wrap:anywhere]"
        >
          Renommé depuis <code class="font-mono text-[10px]">{{ file.oldPath }}</code>
        </div>
        <div
          v-for="(hunk, hunkIndex) in diff.hunks"
          :key="hunkIndex"
          class="diff-hunk min-w-max font-mono"
        >
          <div
            class="hunk-header min-w-full border-t border-border bg-surface px-[13px] py-[5px] whitespace-pre text-accent"
          >
            {{ hunk.header.trimEnd() }}
          </div>
          <div
            v-for="(line, lineIndex) in hunk.lines"
            :key="lineIndex"
            class="diff-line flex min-h-5 min-w-max whitespace-pre"
            :class="line.kind"
          >
            <span
              class="inline-block w-[30px] shrink-0 border-r border-border pr-[5px] text-right text-muted select-none"
              aria-hidden="true"
              >{{ line.oldLine }}</span
            >
            <span
              class="inline-block w-[30px] shrink-0 border-r border-border pr-[5px] text-right text-muted select-none"
              aria-hidden="true"
              >{{ line.newLine }}</span
            >
            <span
              class="line-sign inline-block w-5 shrink-0 text-center text-muted select-none"
              aria-hidden="true"
              >{{ line.kind === 'addition' ? '+' : line.kind === 'deletion' ? '−' : ' ' }}</span
            >
            <code class="pr-[15px] [font:inherit]">{{ line.content.replace(/\r?\n$/, '') }}</code>
          </div>
        </div>
        <div
          v-if="!diff.hunks.length && !diff.truncated"
          class="px-[17px] py-[13px] text-[11px] leading-relaxed text-muted [overflow-wrap:anywhere]"
        >
          Aucune différence de contenu texte. Le changement peut concerner le nom ou les permissions
          du fichier.
        </div>
        <div
          v-if="diff.truncated"
          class="sticky left-0 flex max-w-full items-start gap-2 bg-bg px-[17px] py-[13px] text-[10px] leading-relaxed text-warning"
        >
          <AlertCircle :size="14" class="mt-0.5 shrink-0" /><span
            >Aperçu limité pour préserver la réactivité. Le fichier est volumineux ou le diff
            dépasse la limite d’affichage.</span
          >
        </div>
      </div>
      <div v-else class="diff-state">
        <span>Le diff de ce fichier n’est pas disponible.</span>
      </div>
    </template>
  </div>
</template>

<style scoped>
@reference '../style.css';

.diff-state {
  @apply flex flex-1 flex-col items-center justify-center gap-2.5 p-[22px] text-center text-[11px] text-muted;
}
.diff-state p {
  @apply m-0 max-w-[260px] leading-relaxed [overflow-wrap:anywhere];
}
.diff-state strong {
  @apply font-medium text-text;
}
.diff-hunk {
  font-size: var(--diff-font-size, 11px);
  line-height: var(--diff-line-height, 20px);
}
.diff-hunk:first-child .hunk-header {
  border-top: 0;
}
.diff-line.addition {
  background: color-mix(in srgb, var(--success) 9%, var(--bg));
}
.diff-line.deletion {
  background: color-mix(in srgb, var(--danger) 9%, var(--bg));
}
.addition .line-sign,
.addition code {
  color: var(--success);
}
.deletion .line-sign,
.deletion code {
  color: var(--danger);
}
</style>
