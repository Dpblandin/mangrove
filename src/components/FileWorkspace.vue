<script setup lang="ts">
import { computed } from 'vue'
import { ArrowLeft, FileText, X } from '@lucide/vue'
import DiffViewer from './DiffViewer.vue'
import DiffFontControls from './DiffFontControls.vue'
import FileContentViewer from './FileContentViewer.vue'
import FileStatusIcon from './FileStatusIcon.vue'
import { useFileBrowser, type FileView } from '../composables/useFileBrowser'
import { useDiffHighlighting } from '../composables/useDiffHighlighting'
import { commitDate, shortSha } from '../lib/format'
import type { ChangedFile, FileDiff } from '../lib/types'
import type { HighlightedDiff } from '../lib/diffSyntax'

const props = defineProps<{
  sessionId: string
  sha: string
  intentSha: string
  file: ChangedFile | null
  diff: FileDiff | null
  loading: boolean
  error: string
  highlighted?: HighlightedDiff | null
}>()
const emit = defineEmits<{ close: []; jumpCommit: [sha: string] }>()
const source = computed(() =>
  props.file && props.sha === props.intentSha && !props.loading
    ? { sessionId: props.sessionId, sha: props.sha, file: props.file, initialDiff: props.diff }
    : null,
)
const browser = useFileBrowser(() => source.value)
const {
  target,
  mode,
  content,
  history,
  blame,
  diff,
  loading,
  error,
  previousCursors,
  inspectedCommit,
  inspecting,
  inspectionError,
} = browser
const views: { id: FileView; label: string }[] = [
  { id: 'diff', label: 'Diff' },
  { id: 'file', label: 'Fichier' },
  { id: 'history', label: 'Historique du fichier' },
  { id: 'blame', label: 'Blame' },
]
const original = computed(
  () =>
    !target.value ||
    (target.value.sha === props.sha && target.value.file.fileKey === props.file?.fileKey),
)
const displayedDiff = computed(() => diff.value ?? (original.value ? props.diff : null))
const initialDiff = computed(() => original.value && displayedDiff.value === props.diff)
const viewError = computed(
  () => error.value || (mode.value === 'diff' && original.value && !diff.value ? props.error : ''),
)
const revisionTokens = useDiffHighlighting(
  () => (mode.value === 'diff' && !initialDiff.value ? (target.value?.file ?? null) : null),
  () => (mode.value === 'diff' && !initialDiff.value ? displayedDiff.value : null),
  undefined,
  8_000,
)
async function parentVersion() {
  if (!content.value?.parentSha || !target.value?.file.fileKey) return
  await browser.selectRevision(
    {
      sha: content.value.parentSha,
      path: target.value.file.path,
      fileKey: target.value.file.fileKey,
      status: 'unmodified',
      oldPath: null,
    },
    mode.value,
  )
}
</script>

<template>
  <section
    class="file-workspace"
    aria-label="Consultation du fichier sélectionné"
    :aria-busy="props.loading || loading"
  >
    <header class="file-heading">
      <FileText :size="15" class="shrink-0 text-muted" />
      <h1 :title="target?.file.path ?? file?.path">{{ target?.file.path ?? file?.path }}</h1>
      <code class="revision" :title="target?.sha ?? sha">{{ shortSha(target?.sha ?? sha) }}</code>
      <DiffFontControls />
      <button
        class="icon-button"
        aria-label="Fermer le fichier"
        title="Retour à l’historique du dépôt · Échap"
        @click="emit('close')"
      >
        <X :size="16" />
      </button>
    </header>
    <nav class="file-toolbar" aria-label="Vues du fichier">
      <button
        class="file-action"
        aria-label="Retour à l’historique du dépôt"
        @click="emit('close')"
      >
        <ArrowLeft :size="13" />
      </button>
      <button
        v-if="!original && source"
        class="file-action"
        :disabled="loading"
        @click="browser.restoreSelection()"
      >
        Retour à {{ shortSha(sha) }}
      </button>
      <button
        v-for="view in views"
        :key="view.id"
        class="file-action view-button"
        :aria-pressed="mode === view.id"
        :disabled="props.loading || !target?.file.fileKey"
        @click="browser.show(view.id)"
      >
        {{ view.label }}
      </button>
    </nav>
    <p v-if="viewError" class="file-notice error" role="alert">
      {{ viewError }}
      <button
        v-if="target?.file.fileKey"
        class="file-action"
        :disabled="loading"
        @click="browser.show(mode)"
      >
        Réessayer
      </button>
    </p>
    <template v-if="mode === 'diff'">
      <DiffViewer
        v-if="!viewError"
        :file="target?.file ?? file"
        :diff="displayedDiff"
        :loading="loading || props.loading"
        error=""
        :highlighted="initialDiff ? highlighted : revisionTokens"
      />
    </template>
    <template v-else-if="mode === 'history'">
      <p class="file-notice">
        Premier parent uniquement aux merges. Les renommages détectés sont suivis ; copies et
        renommages ambigus peuvent interrompre le suivi.
      </p>
      <div class="file-history-scroll" tabindex="0" aria-label="Historique du fichier">
        <article
          v-for="entry in history?.entries ?? []"
          :key="entry.sha"
          class="file-history-entry"
        >
          <FileStatusIcon :status="entry.status" />
          <div class="history-entry-text">
            <strong>{{ entry.summary }}</strong
            ><small
              >{{ entry.author }} · {{ commitDate(entry.time) }} · {{ shortSha(entry.sha)
              }}<span v-if="entry.merge"> · Merge, premier parent</span></small
            ><code v-if="entry.oldPath">{{ entry.oldPath }} → {{ entry.path }}</code>
          </div>
          <button
            class="file-action"
            :aria-label="'Voir le diff du commit ' + shortSha(entry.sha)"
            @click="browser.selectRevision(entry)"
          >
            Diff
          </button>
          <button
            class="file-action"
            :aria-label="'Voir le fichier au commit ' + shortSha(entry.sha)"
            @click="browser.selectRevision(entry, 'file')"
          >
            Fichier
          </button>
        </article>
        <p v-if="history && !history.entries.length" class="file-notice">
          Aucun changement dans les {{ history.scanned }} commits parcourus sur cette page.
        </p>
        <p v-if="history?.limitation" class="file-notice" role="status">{{ history.limitation }}</p>
      </div>
      <footer class="file-pagination">
        <button
          class="file-action"
          :disabled="loading || !previousCursors.length"
          @click="browser.page(-1)"
        >
          Page précédente</button
        ><span v-if="history"
          >{{ history.entries.length }} changements · {{ history.scanned }} commits parcourus</span
        ><button
          class="file-action"
          :disabled="loading || !history?.nextCursor"
          @click="browser.page(1)"
        >
          Page suivante
        </button>
      </footer>
    </template>
    <template v-else>
      <p v-if="mode === 'blame'" class="file-notice">
        Attributions sur le premier parent, jusqu’à 256 commits. Sélectionnez un groupe pour
        consulter son commit. Un bord jaune indique une frontière du parcours.
      </p>
      <div
        v-if="mode === 'blame' && (inspectedCommit || inspecting || inspectionError)"
        class="inspected-commit"
        :aria-busy="inspecting"
      >
        <button
          class="icon-button inspection-close"
          aria-label="Fermer le commit consulté"
          @click="browser.closeInspection()"
        >
          <X :size="13" />
        </button>
        <span v-if="inspecting" role="status">Lecture du commit…</span>
        <template v-if="inspectedCommit"
          ><div>
            <strong>{{ shortSha(inspectedCommit.sha) }}</strong> · {{ inspectedCommit.author }} ·
            {{ commitDate(inspectedCommit.time) }}
            <button class="file-action" @click="emit('jumpCommit', inspectedCommit.sha)">
              Voir dans le dépôt
            </button>
          </div>
          <pre>{{ inspectedCommit.message }}</pre>
        </template>
        <p v-if="inspectionError" role="alert">{{ inspectionError }}</p>
      </div>
      <p v-if="blame?.limitation && mode === 'blame'" class="file-notice" role="status">
        {{ blame.limitation }}
      </p>
      <FileContentViewer
        v-if="content && target"
        :file="target.file"
        :content="content"
        :blame="blame"
        :show-blame="mode === 'blame'"
        @inspect="browser.inspectCommit"
        @parent="parentVersion"
      />
    </template>
  </section>
</template>

<style scoped>
.file-workspace {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  background: var(--reading-bg);
}
.file-heading {
  display: flex;
  align-items: center;
  min-height: 40px;
  gap: 9px;
  padding: 0 16px;
  border-bottom: 1px solid var(--border);
  background: var(--panel);
}
.file-heading h1 {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: calc(12px + var(--font-size-offset));
  font-weight: 500;
}
.revision {
  color: var(--muted);
  font: calc(10px + var(--font-size-offset)) var(--mono);
}
.file-toolbar {
  display: flex;
  min-height: 40px;
  align-items: center;
  gap: 6px;
  padding: 5px 12px;
  border-bottom: 1px solid var(--border);
  flex-wrap: wrap;
}
.file-action {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  border: 1px solid var(--border);
  border-radius: 4px;
  padding: 4px 7px;
  background: var(--surface);
  color: var(--text);
  font: inherit;
  font-size: calc(10px + var(--font-size-offset));
  cursor: pointer;
  white-space: nowrap;
}
.file-action:disabled {
  opacity: 0.4;
  cursor: default;
}
.file-action:hover:not(:disabled),
.file-action[aria-pressed='true'] {
  color: var(--accent);
  border-color: var(--accent);
}
.file-action:focus-visible {
  outline: 1px solid var(--accent);
  outline-offset: 2px;
}
.file-notice {
  padding: 9px 16px;
  margin: 0;
  color: var(--muted);
  font-size: calc(10px + var(--font-size-offset));
  line-height: 1.6;
}
.file-notice.error {
  color: var(--danger);
}
.file-history-scroll {
  flex: 1;
  min-height: 0;
  overflow: auto;
}
.file-history-entry {
  display: flex;
  align-items: center;
  gap: 9px;
  padding: 12px;
  border-bottom: 1px solid var(--border);
}
.history-entry-text {
  flex: 1;
  min-width: 0;
}
.history-entry-text strong {
  display: block;
  font-weight: 500;
  font-size: calc(12px + var(--font-size-offset));
  overflow-wrap: anywhere;
}
.history-entry-text small,
.history-entry-text code {
  display: block;
  color: var(--muted);
  font-size: calc(10px + var(--font-size-offset));
  overflow-wrap: anywhere;
  margin-top: 5px;
}
.file-pagination {
  display: flex;
  justify-content: space-between;
  gap: 8px;
  align-items: center;
  padding: 10px;
  border-top: 1px solid var(--border);
  font-size: calc(10px + var(--font-size-offset));
  color: var(--muted);
}
.inspected-commit {
  position: relative;
  border-bottom: 1px solid var(--border);
  background: var(--panel);
  padding: 10px 42px 10px 16px;
  font-size: calc(11px + var(--font-size-offset));
}
.inspection-close {
  position: absolute;
  right: 8px;
  top: 7px;
}
.inspected-commit pre {
  max-height: 130px;
  overflow: auto;
  white-space: pre-wrap;
  font: inherit;
  margin: 8px 0 0;
}
</style>
