<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import {
  ArrowRight,
  Check,
  ChevronDown,
  ChevronRight,
  CircleDot,
  Command,
  FolderGit2,
  FolderOpen,
  GitBranch,
  GitCommitHorizontal,
  Globe,
  Layers,
  RefreshCw,
  Search,
  ShieldCheck,
  Tag,
  X,
} from '@lucide/vue'
import CommitList from './components/CommitList.vue'
import CommitDetailsPanel from './components/CommitDetailsPanel.vue'
import OpenRepositoryDialog from './components/OpenRepositoryDialog.vue'
import PaneDivider from './components/PaneDivider.vue'
import { useRepository } from './composables/useRepository'
import { usePaneLayout } from './composables/usePaneLayout'
import { chooseFolder, desktop } from './lib/api'
import { count, provider } from './lib/format'
import { themes, getStoredTheme, setTheme, type ThemeId } from './lib/themes'
import type { Reference } from './lib/types'

const state = useRepository()
const {
  repository,
  progress,
  pages,
  total,
  query,
  filter,
  opening,
  loadingPages,
  error,
  selectedSha,
  selectedIndex,
  details,
  detailLoading,
  detailError,
  selectedFile,
  diff,
  diffLoading,
  diffError,
} = state
const list = ref<InstanceType<typeof CommitList>>()
const layoutRoot = ref<HTMLElement>()
const layout = usePaneLayout(layoutRoot)
const { leftWidth, rightWidth, leftBounds, rightBounds, resizing } = layout
const theme = ref<ThemeId>(getStoredTheme())
watch(theme, setTheme)
const search = ref<HTMLInputElement>()
const showDialog = ref(false)
const referenceFilter = ref('')
const collapsed = ref(new Set<string>())
const shortcut = /Mac/.test(navigator.platform) ? '⌘' : 'Ctrl'
const recent = ref<{ path: string; name: string }[]>([])
try {
  const saved: unknown = JSON.parse(localStorage.getItem('mangrove:recent') ?? '[]')
  if (Array.isArray(saved))
    recent.value = saved
      .filter(
        (entry): entry is { path: string; name: string } =>
          typeof entry === 'object' &&
          entry !== null &&
          typeof entry.path === 'string' &&
          typeof entry.name === 'string',
      )
      .slice(0, 6)
} catch {
  /* A missing or unavailable preferences store does not prevent opening a repo. */
}
const groups = computed(() =>
  [
    { kind: 'local', title: 'Branches locales', icon: GitBranch },
    { kind: 'remote', title: 'Branches distantes', icon: Globe },
    { kind: 'tag', title: 'Tags', icon: Tag },
    { kind: 'other', title: 'Autres références', icon: Layers },
  ].map((group) => ({
    ...group,
    references: (repository.value?.references ?? []).filter(
      (reference) =>
        reference.kind === group.kind &&
        reference.label.toLocaleLowerCase().includes(referenceFilter.value.toLocaleLowerCase()),
    ),
  })),
)
const sourceLabel = computed(() => provider(repository.value?.remotes ?? []))
const latestReference = computed(() =>
  repository.value?.references.find(
    (reference) =>
      reference.kind === 'local' && reference.name === 'refs/heads/' + repository.value?.headName,
  ),
)
const hasReferences = computed(() => groups.value.some((group) => group.references.length))

async function openPath(path: string) {
  if (await state.open(path)) {
    showDialog.value = false
    const info = repository.value
    if (!info) return
    recent.value = [
      { path: info.path, name: info.name },
      ...recent.value.filter((entry) => entry.path !== info.path),
    ].slice(0, 6)
    try {
      localStorage.setItem('mangrove:recent', JSON.stringify(recent.value))
    } catch {
      /* Preferences are optional. */
    }
    referenceFilter.value = ''
    collapsed.value = new Set()
  }
}
async function openDialog() {
  if (opening.value) return
  if (desktop) {
    try {
      const path = await chooseFolder()
      if (path) await openPath(path)
    } catch (cause) {
      error.value = String(cause)
      showDialog.value = true
    }
  } else showDialog.value = true
}
async function jump(reference: Reference) {
  const index = await state.jump(reference.sha)
  if (index !== null) {
    await nextTick()
    if (selectedIndex.value === index && selectedSha.value === reference.sha)
      list.value?.scrollToIndex(index)
  }
}
async function navigate(direction: -1 | 1) {
  const index = await state.moveSelection(direction)
  if (index !== null && selectedIndex.value === index) list.value?.scrollToIndex(index)
}
function toggleGroup(kind: string) {
  const next = new Set(collapsed.value)
  if (next.has(kind)) next.delete(kind)
  else next.add(kind)
  collapsed.value = next
}
watch(filter, async () => {
  await nextTick()
  list.value?.scrollToIndex(0)
})
function keyboard(event: KeyboardEvent) {
  if (!event.metaKey && !event.ctrlKey) return
  if (event.key.toLowerCase() === 'o') {
    event.preventDefault()
    void openDialog()
  }
  if (event.key.toLowerCase() === 'k' && repository.value) {
    event.preventDefault()
    search.value?.focus()
  }
}
onMounted(() => window.addEventListener('keydown', keyboard))
onBeforeUnmount(() => window.removeEventListener('keydown', keyboard))
</script>

<template>
  <div
    ref="layoutRoot"
    class="app-shell flex h-dvh min-h-[540px] min-w-[880px] overflow-hidden"
    :class="{ 'panes-resizing': resizing }"
  >
    <aside
      id="branches-pane"
      class="sidebar flex shrink-0 flex-col bg-panel border-r border-border"
      :class="{ compact: leftWidth < 210 }"
      :style="{ width: `${leftWidth}px` }"
    >
      <div class="brand">
        <img src="/favicon.svg" width="31" height="31" alt="" />
        <span>Mangrove<small>Git, en lecture seule.</small></span>
      </div>
      <div class="workspace-heading">
        <span>ESPACE DE TRAVAIL</span><span class="local-dot"></span>
      </div>
      <button class="repository-switch" :disabled="opening" @click="openDialog">
        <FolderGit2 :size="19" />
        <span
          >{{ repository?.name ?? 'Ouvrir un dépôt'
          }}<small>{{ repository ? sourceLabel : 'Depuis votre ordinateur' }}</small></span
        >
        <ChevronDown :size="14" />
      </button>
      <template v-if="repository">
        <label class="reference-search"
          ><Search :size="13" /><input
            v-model="referenceFilter"
            placeholder="Filtrer les références"
            aria-label="Filtrer les références"
        /></label>
        <nav class="references" aria-label="Références Git">
          <div
            v-for="group in groups.filter((group) => group.references.length)"
            :key="group.kind"
            class="reference-group"
          >
            <button
              class="group-title"
              :aria-expanded="!collapsed.has(group.kind)"
              @click="toggleGroup(group.kind)"
            >
              <ChevronRight v-if="collapsed.has(group.kind)" :size="12" /><ChevronDown
                v-else
                :size="12"
              />
              <span>{{ group.title }}</span
              ><small>{{ count(group.references.length) }}</small>
            </button>
            <div v-if="!collapsed.has(group.kind)" class="reference-items">
              <button
                v-for="reference in group.references"
                :key="reference.name"
                class="reference-item"
                :class="{
                  current: reference.name === latestReference?.name,
                  highlighted: reference.sha === selectedSha,
                }"
                :title="reference.label"
                @click="jump(reference)"
              >
                <component :is="group.icon" :size="13" />
                <span>{{ reference.label }}</span>
                <Check v-if="reference.name === latestReference?.name" :size="12" />
              </button>
            </div>
          </div>
          <p v-if="!hasReferences" class="reference-none">Aucune référence trouvée.</p>
        </nav>
      </template>
      <div v-else class="sidebar-empty">
        <div class="sidebar-empty-line"></div>
        <GitBranch :size="22" />
        <p>Les branches et les tags<br />de votre dépôt apparaîtront ici.</p>
      </div>
      <div class="sidebar-bottom">
        <span class="workspace-avatar"><FolderGit2 :size="17" /></span>
        <span>Espace local<small>Prototype · 0.1.0</small></span>
        <span class="offline-indicator" title="Consultation locale, sans connexion requise"></span>
      </div>
    </aside>

    <PaneDivider
      :value="leftWidth"
      :min="leftBounds.min"
      :max="leftBounds.max"
      :direction="1"
      label="Largeur du panneau des branches"
      controls="branches-pane"
      @resize="layout.resize('left', $event)"
      @start="layout.startResize('left')"
      @finish="layout.finishResize"
      @reset="layout.reset('left')"
    />

    <main class="workspace relative flex min-w-0 flex-1 flex-col" :class="{ welcome: !repository }">
      <header class="topbar">
        <div class="breadcrumbs">
          <span>Espace local</span><ChevronRight :size="12" />
          <strong>{{ repository?.name ?? 'Bienvenue' }}</strong>
        </div>
        <div class="topbar-actions">
          <select v-model="theme" class="theme-select" aria-label="Thème de l’interface">
            <option v-for="item in themes" :key="item.id" :value="item.id">{{ item.label }}</option>
          </select>
          <span class="readonly-badge" title="Aucune commande de modification du dépôt"
            ><ShieldCheck :size="12" />Lecture seule</span
          >
          <button
            class="icon-button"
            :disabled="opening"
            aria-label="Ouvrir un dépôt"
            :title="'Ouvrir un dépôt · ' + shortcut + ' O'"
            @click="openDialog"
          >
            <FolderOpen :size="17" />
          </button>
        </div>
      </header>

      <template v-if="repository">
        <div
          class="workspace-content grid min-h-0 flex-1"
          :style="{ gridTemplateColumns: `minmax(0, 1fr) 6px ${rightWidth}px` }"
        >
          <section
            class="history-panel flex min-h-0 min-w-0 flex-col bg-bg"
            aria-label="Historique du dépôt"
          >
            <div class="history-toolbar">
              <div class="history-title">
                <GitBranch :size="17" />
                <h1>Historique</h1>
                <span class="branch-chip">{{ repository.headName }}</span>
              </div>
              <button
                class="icon-button"
                aria-label="Actualiser le dépôt"
                title="Relire les références locales"
                :disabled="opening"
                @click="openPath(repository.path)"
              >
                <RefreshCw :size="14" />
              </button>
            </div>
            <div class="search-toolbar">
              <label class="commit-search"
                ><Search :size="15" />
                <input
                  ref="search"
                  v-model="query"
                  maxlength="256"
                  aria-label="Rechercher des commits"
                  placeholder="Message, auteur, SHA, référence…"
                  spellcheck="false"
                />
                <button
                  v-if="query"
                  class="search-clear"
                  aria-label="Effacer la recherche"
                  @click="query = ''"
                >
                  <X :size="12" />
                </button>
                <kbd v-else>{{ shortcut }} K</kbd>
              </label>
              <span class="result-count"
                >{{ count(total) }} <span>{{ filter ? 'résultats' : 'commits' }}</span></span
              >
            </div>
            <CommitList
              ref="list"
              :session-id="repository.id"
              :pages="pages"
              :total="total"
              :selected-sha="selectedSha"
              :selected-index="selectedIndex"
              :references="repository.references"
              :filtered="Boolean(filter)"
              :loading="loadingPages > 0"
              :indexing="!progress.complete"
              @range="state.loadRange"
              @select="state.select"
              @navigate="navigate"
            />
            <footer class="history-footer">
              <span v-if="!progress.complete"
                ><span class="spinner small"></span>Indexation ·
                {{ count(progress.indexed) }} commits</span
              >
              <span v-else-if="progress.error"><CircleDot :size="11" />Indexation interrompue</span>
              <span v-else><span class="ready-dot"></span>Index local prêt</span>
              <span class="footer-hint"
                ><Command :size="11" />{{ shortcut }} K pour rechercher</span
              >
            </footer>
          </section>
          <PaneDivider
            :value="rightWidth"
            :min="rightBounds.min"
            :max="rightBounds.max"
            :direction="-1"
            label="Largeur du panneau des détails"
            controls="commit-details-pane"
            @resize="layout.resize('right', $event)"
            @start="layout.startResize('right')"
            @finish="layout.finishResize"
            @reset="layout.reset('right')"
          />
          <CommitDetailsPanel
            id="commit-details-pane"
            :details="details"
            :loading="detailLoading"
            :error="detailError"
            :selected-file="selectedFile"
            :diff="diff"
            :diff-loading="diffLoading"
            :diff-error="diffError"
            @select-file="state.selectFile"
          />
        </div>
        <div v-if="error && !showDialog" class="error-toast" role="alert">
          <span>{{ error }}</span>
          <button class="icon-button" aria-label="Fermer le message" @click="error = ''">
            <X :size="14" />
          </button>
        </div>
      </template>

      <section v-else class="welcome-content">
        <div class="welcome-symbol"><img src="/favicon.svg" width="56" height="56" alt="" /></div>
        <span class="welcome-eyebrow">UN AUTRE REGARD SUR VOTRE CODE</span>
        <h1>Votre historique,<br /><span>en clair.</span></h1>
        <p>
          Explorez les branches, retrouvez un commit et comprenez<br class="desktop-break" />
          ce qui a changé. Votre dépôt reste intact.
        </p>
        <button class="primary-button welcome-open" :disabled="opening" @click="openDialog">
          <span v-if="opening" class="spinner"></span><FolderOpen v-else :size="17" />
          {{ opening ? 'Ouverture du dépôt…' : 'Ouvrir un dépôt local' }}<ArrowRight :size="15" />
        </button>
        <span class="welcome-shortcut"
          >ou <kbd>{{ shortcut }} O</kbd></span
        >
        <p v-if="error && !showDialog" class="welcome-error" role="alert">{{ error }}</p>
        <div v-if="recent.length" class="recent-repositories">
          <h2>Récemment ouverts</h2>
          <button
            v-for="entry in recent"
            :key="entry.path"
            :disabled="opening"
            :title="entry.path"
            @click="openPath(entry.path)"
          >
            <FolderGit2 :size="16" /><span
              >{{ entry.name }}<small>{{ entry.path }}</small></span
            ><ChevronRight :size="14" />
          </button>
        </div>
        <div class="welcome-features">
          <div>
            <GitBranch :size="18" /><strong>Voyez les connexions</strong
            ><span>Un graphe de branches<br />à parcourir librement.</span>
          </div>
          <div>
            <Search :size="18" /><strong>Retrouvez l’essentiel</strong
            ><span>Une recherche locale<br />dans votre historique.</span>
          </div>
          <div>
            <GitCommitHorizontal :size="18" /><strong>Comprenez les changements</strong
            ><span>Les fichiers et leurs diffs,<br />au moment où vous en avez besoin.</span>
          </div>
        </div>
        <span class="welcome-footnote"
          ><ShieldCheck :size="12" />Vos dépôts restent sur votre machine.</span
        >
      </section>
    </main>
    <OpenRepositoryDialog
      :open="showDialog"
      :busy="opening"
      :error="error"
      @close="showDialog = false"
      @submit="openPath"
    />
  </div>
</template>

<style scoped>
.panes-resizing,
.panes-resizing :deep(*) {
  cursor: col-resize !important;
  user-select: none !important;
}
.theme-select {
  max-width: 190px;
  height: 29px;
  padding: 4px 8px;
  border: 1px solid var(--border);
  border-radius: 5px;
  background: var(--surface);
  color: var(--text);
  font: inherit;
  font-size: 11px;
  cursor: pointer;
}
.theme-select:focus-visible {
  outline: 1px solid var(--accent);
  outline-offset: 2px;
}
.sidebar.compact .brand,
.sidebar.compact .sidebar-bottom {
  padding-inline: 14px;
}
.sidebar.compact .brand {
  gap: 9px;
}
.sidebar.compact .workspace-heading {
  padding-inline: 14px;
}
</style>
