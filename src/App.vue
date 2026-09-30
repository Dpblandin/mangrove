<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import {
  ArrowRight,
  ChevronDown,
  ChevronRight,
  CircleDot,
  Command,
  FolderGit2,
  FolderOpen,
  GitBranch,
  RefreshCw,
  Search,
  Settings,
  Download,
  X,
} from '@lucide/vue'
import CommitList from './components/CommitList.vue'
import CommitDetailsPanel from './components/CommitDetailsPanel.vue'
import DiffWorkspace from './components/DiffWorkspace.vue'
import FileWorkspace from './components/FileWorkspace.vue'
import OpenRepositoryDialog from './components/OpenRepositoryDialog.vue'
import PaneDivider from './components/PaneDivider.vue'
import ReferenceTree from './components/ReferenceTree.vue'
import WorkingTreeRow from './components/WorkingTreeRow.vue'
import WorkingTreePanel from './components/WorkingTreePanel.vue'
import RepositoryTabs from './components/RepositoryTabs.vue'
import SettingsView from './components/SettingsView.vue'
import RepositoryContext from './components/RepositoryContext.vue'
import StashPanel from './components/StashPanel.vue'
import { useRepositoryContext } from './composables/useRepositoryContext'
import { useRepository } from './composables/useRepository'
import { useRepositoryTabs } from './composables/useRepositoryTabs'
import { useRecentRepositories } from './composables/useRecentRepositories'
import { useWorkingTree } from './composables/useWorkingTree'
import { usePaneLayout } from './composables/usePaneLayout'
import { useDelayedBusy } from './composables/useDelayedBusy'
import { useAutoFetch } from './composables/useAutoFetch'
import { chooseFolder, desktop } from './lib/api'
import { count } from './lib/format'
import { themes, getStoredTheme, setTheme, type ThemeId } from './lib/themes'
import { handleFontSizeShortcut } from './lib/fontSize'
import { getStoredAutoFetchInterval, storeAutoFetchInterval } from './lib/autoFetch'
import type {
  ChangedFile,
  CommitRow,
  Reference,
  StashArea,
  WorkingTreeArea,
  WorkingTreeFile,
} from './lib/types'

const state = useRepository()
const repositoryTabs = useRepositoryTabs()
const { tabs, activeId: activeTabId } = repositoryTabs
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
  highlightedDiff,
} = state
const openingVisible = useDelayedBusy(() => opening.value)
const showSettings = ref(false)
const settingsButton = ref<HTMLButtonElement>()
const autoFetchInterval = ref(getStoredAutoFetchInterval())
watch(autoFetchInterval, storeAutoFetchInterval)
const autoFetch = useAutoFetch(repository, autoFetchInterval, state.refresh)
const { fetching, error: fetchError } = autoFetch
function dismissError() {
  error.value = ''
  fetchError.value = ''
}
const selection = ref<'commit' | 'working' | 'stash'>('commit')
const context = useRepositoryContext(repository, () => selection.value === 'stash')
const {
  worktrees,
  stashes,
  worktreesLoading,
  stashesLoading,
  worktreesError,
  stashesError,
  details: stashDetails,
  selectedSha: stashSha,
  detailsLoading: stashLoading,
  detailsError: stashError,
  selectedFile: stashFile,
  selectedArea: stashArea,
  diff: stashDiff,
  highlightedDiff: stashHighlightedDiff,
  fileLoading: stashDiffLoading,
  diffError: stashDiffError,
} = context
const worktreesExpanded = ref(false)
const stashesExpanded = ref(false)
const soloRefs = computed(() => new Set(repository.value?.soloRefs ?? []))
const graphReferences = computed(() => {
  const references = repository.value?.references ?? []
  return soloRefs.value.size
    ? references.filter((reference) => soloRefs.value.has(reference.name))
    : references
})
const soloLabels = computed(
  () =>
    repository.value?.references
      .filter((reference) => soloRefs.value.has(reference.name))
      .map((reference) => reference.label) ?? [],
)
watch(
  () => repository.value?.id,
  (id) => {
    if (!id) return
    if (worktreesExpanded.value) void context.loadWorktrees()
    if (stashesExpanded.value) void context.loadStashes()
  },
)
function toggleWorktrees() {
  worktreesExpanded.value = !worktreesExpanded.value
  if (worktreesExpanded.value) void context.loadWorktrees()
}
function toggleStashes() {
  stashesExpanded.value = !stashesExpanded.value
  if (stashesExpanded.value) void context.loadStashes()
}
function selectStash(sha: string) {
  interactionRevision++
  selection.value = 'stash'
  state.pauseDiffPreload()
  diffOpen.value = false
  void context.selectStash(sha)
}
function openStashFile(area: StashArea, file: ChangedFile) {
  interactionRevision++
  diffOpen.value = true
  void context.selectFile(area, file)
}
function changeStashArea() {
  interactionRevision++
  diffOpen.value = false
  context.clearFile()
}
async function toggleSolo(reference: Reference) {
  const path = repository.value?.path
  if (!path || opening.value) return
  interactionRevision++
  const next = new Set(soloRefs.value)
  if (next.has(reference.name)) next.delete(reference.name)
  else next.add(reference.name)
  await openPath(path, true, [...next])
}
async function stopSolo() {
  if (!repository.value || opening.value) return
  interactionRevision++
  await openPath(repository.value.path, true, [])
}
const working = useWorkingTree(repository, undefined, {
  active: () => selection.value === 'working',
})
const {
  status: workingStatus,
  selectedFile: workingFile,
  selectedArea: workingArea,
  diff: workingDiff,
  highlightedDiff: workingHighlightedDiff,
  refreshing: workingRefreshing,
  diffLoading: workingDiffLoading,
  error: workingError,
} = working
let interactionRevision = 0
const showWorkingRow = computed(
  () =>
    selection.value === 'working' ||
    Boolean(workingError.value) ||
    (workingStatus.value?.stagedCount ?? 0) + (workingStatus.value?.unstagedCount ?? 0) > 0,
)
watch(
  () => repository.value?.path,
  () => {
    selection.value = 'commit'
    diffOpen.value = false
  },
)
const list = ref<InstanceType<typeof CommitList>>()
const workingRow = ref<InstanceType<typeof WorkingTreeRow>>()
const layoutRoot = ref<HTMLElement>()
const layout = usePaneLayout(layoutRoot)
const { leftWidth, rightWidth, leftBounds, rightBounds, resizing } = layout
function openSettings() {
  if (!showDialog.value && !resizing.value) showSettings.value = true
}
async function closeSettings() {
  showSettings.value = false
  await nextTick()
  settingsButton.value?.focus()
}
const theme = ref<ThemeId>(getStoredTheme())
watch(theme, setTheme)
const diffMode = ref<'central' | 'compact'>('central')
const diffOpen = ref(false)
const fileSelectionRevision = ref(0)
const showCentralDiff = computed(() => diffMode.value === 'central' && diffOpen.value)
watch(selectedSha, (sha) => {
  if (!sha && selection.value === 'commit') diffOpen.value = false
})
watch(stashSha, (sha) => {
  if (!sha && selection.value === 'stash') diffOpen.value = false
})
const displayedFile = computed(() =>
  selection.value === 'stash'
    ? stashFile.value
    : selection.value === 'working'
      ? workingFile.value
      : selectedFile.value,
)
const displayedDiff = computed(() =>
  selection.value === 'stash'
    ? stashDiff.value
    : selection.value === 'working'
      ? workingDiff.value
      : diff.value,
)
const displayedDiffLoading = computed(() =>
  selection.value === 'stash'
    ? stashLoading.value || stashDiffLoading.value
    : selection.value === 'working'
      ? workingDiffLoading.value
      : detailLoading.value || diffLoading.value,
)
const displayedDiffError = computed(() =>
  selection.value === 'stash'
    ? stashError.value || stashDiffError.value
    : selection.value === 'working'
      ? workingError.value
      : detailError.value || diffError.value,
)

function selectWorkingTree() {
  interactionRevision++
  state.pauseDiffPreload()
  selection.value = 'working'
  diffOpen.value = false
  workingRow.value?.focus()
  void working.refresh()
}
function openWorkingFile(area: WorkingTreeArea, file: WorkingTreeFile) {
  interactionRevision++
  diffOpen.value = true
  void working.selectFile(area, file)
}
function selectCommit(row: CommitRow, index: number) {
  interactionRevision++
  selection.value = 'commit'
  state.resumeDiffPreload()
  diffOpen.value = false
  void state.select(row, index)
}
function refreshOnFocus() {
  if (opening.value) return
  if (repository.value) void refreshRepository()
  else void recentRepositories.refreshNames()
}
async function refreshRepository() {
  const previousRepository = repository.value
  if (!previousRepository || opening.value) return
  const path = previousRepository.path
  const tabId = activeTabId.value
  const previousSelection = selection.value
  const previousStash = stashSha.value
  const previousStashFile = stashFile.value
  const previousStashArea = stashArea.value
  const revision = ++interactionRevision
  const previousFile = workingFile.value
  const previousArea = workingArea.value
  const previousDiffOpen = diffOpen.value
  if (!(await state.refresh())) return
  const info = repository.value
  if (!info || info.path !== path || activeTabId.value !== tabId) return
  if (tabId) repositoryTabs.update(tabId, info)
  recentRepositories.updateName(info)
  const sessionId = info.id
  const current = () =>
    repository.value?.path === path &&
    repository.value?.id === sessionId &&
    activeTabId.value === tabId &&
    revision === interactionRevision
  if (!current()) return
  if (sessionId === previousRepository.id) {
    await Promise.all([
      worktreesExpanded.value ? context.loadWorktrees() : Promise.resolve(),
      stashesExpanded.value || previousSelection === 'stash'
        ? context.loadStashes()
        : Promise.resolve(),
      working.refresh(),
    ])
    return
  }
  if (previousSelection === 'stash') {
    if (!stashes.value) await context.loadStashes()
    if (!current()) return
    if (!stashes.value?.stashes.some((stash) => stash.sha === previousStash)) return
    selection.value = 'stash'
    state.pauseDiffPreload()
    await context.selectStash(previousStash)
    if (!current() || selection.value !== 'stash') return
    if (!previousStashFile || !previousStashArea) return
    const file = stashDetails.value?.groups
      .find((group) => group.area === previousStashArea)
      ?.files.find((file) => file.fileKey === previousStashFile.fileKey)
    if (!file) return
    await context.selectFile(previousStashArea, file)
    if (current() && selection.value === 'stash') diffOpen.value = previousDiffOpen
    return
  }
  selection.value = previousSelection
  if (previousSelection === 'working') state.pauseDiffPreload()
  if (previousSelection === 'working' && previousFile && previousArea) {
    await working.refresh()
    if (!current() || selection.value !== 'working') return
    const file = workingStatus.value?.[previousArea].find((file) => file.key === previousFile.key)
    if (file) {
      await working.selectFile(previousArea, file)
      if (current() && selection.value === 'working') diffOpen.value = previousDiffOpen
    }
  }
}

function openFile(file: ChangedFile) {
  interactionRevision++
  fileSelectionRevision.value++
  diffOpen.value = true
  void state.selectFile(file)
}
function changeDiffMode(mode: 'central' | 'compact') {
  interactionRevision++
  diffMode.value = mode
  if (mode === 'central' && displayedFile.value) diffOpen.value = true
}
async function closeDiff() {
  interactionRevision++
  diffOpen.value = false
  await nextTick()
  list.value?.focus()
}
const search = ref<HTMLInputElement>()
const showDialog = ref(false)
const referenceFilter = ref('')
const normalizedReferenceFilter = computed(() => referenceFilter.value.trim().toLocaleLowerCase())
const collapsed = ref(new Set<string>())
const searchCollapsedGroups = ref(new Set<string>())
watch(referenceFilter, () => {
  searchCollapsedGroups.value = new Set()
})
const closedGroups = computed(() =>
  normalizedReferenceFilter.value ? searchCollapsedGroups.value : collapsed.value,
)
const collapsedFolders = ref<Record<Reference['kind'], Set<string>>>({
  local: new Set(),
  remote: new Set(),
  tag: new Set(),
  other: new Set(),
})
const isMac = /Mac/.test(navigator.platform)
const shortcut = isMac ? '⌘' : 'Ctrl'
const recentRepositories = useRecentRepositories()
const recent = recentRepositories.entries
const recentFilter = ref('')
const recentSearch = ref<HTMLInputElement>()
const welcomeOpen = ref<HTMLButtonElement>()
const filteredRecent = computed(() => {
  const text = recentFilter.value.trim().toLocaleLowerCase()
  return recent.value.filter((entry) =>
    `${entry.name} ${entry.path}`.toLocaleLowerCase().includes(text),
  )
})
watch(
  activeTabId,
  () => {
    recentFilter.value = ''
    if (!tabs.value.find((tab) => tab.id === activeTabId.value)?.path)
      void recentRepositories.refreshNames()
  },
  { immediate: true },
)

async function removeRecent(path: string) {
  recentRepositories.remove(path)
  await nextTick()
  const target = recentSearch.value ?? welcomeOpen.value
  target?.focus()
}
const groups = computed(() =>
  [
    { kind: 'local' as const, title: 'Branches locales' },
    { kind: 'remote' as const, title: 'Branches distantes' },
    { kind: 'tag' as const, title: 'Tags' },
    { kind: 'other' as const, title: 'Autres références' },
  ].map((group) => ({
    ...group,
    references: (repository.value?.references ?? []).filter(
      (reference) =>
        reference.kind === group.kind &&
        reference.label.toLocaleLowerCase().includes(normalizedReferenceFilter.value),
    ),
  })),
)
const latestReference = computed(() =>
  repository.value?.references.find(
    (reference) =>
      reference.kind === 'local' && reference.name === 'refs/heads/' + repository.value?.headName,
  ),
)
const hasReferences = computed(() => groups.value.some((group) => group.references.length))

let openingRevision = 0
async function openPath(path: string, refreshing = false, solo?: string[]) {
  if (!refreshing) showSettings.value = false
  const previousQuery = query.value
  const requestedSolo = solo ?? (refreshing ? (repository.value?.soloRefs ?? []) : [])
  const attempt = ++openingRevision
  const previousTabs = tabs.value
  if (!refreshing) interactionRevision++
  const tab = refreshing
    ? tabs.value.find((item) => item.id === activeTabId.value)
    : repositoryTabs.open(path)
  const previousPath = repository.value?.path
  const previousTab = previousTabs.find((item) => item.id === tab?.id)
  if (await state.open(path, requestedSolo, refreshing ? previousQuery : '')) {
    showDialog.value = false
    const info = repository.value
    if (!info || !tab || activeTabId.value !== tab.id || attempt !== openingRevision) return false
    repositoryTabs.update(tab.id, info)
    recentRepositories.remember(info)
    if (!refreshing) referenceFilter.value = ''
    if (info.path !== previousPath) {
      collapsed.value = new Set()
      collapsedFolders.value = {
        local: new Set(),
        remote: new Set(),
        tag: new Set(),
        other: new Set(),
      }
    }
    return true
  } else if (!refreshing && tab && attempt === openingRevision) {
    repositoryTabs.rejectOpen(tab.id, previousTab)
  }
  return false
}
async function activateTab(id: string) {
  if (id === activeTabId.value && (repository.value || opening.value)) return
  const tab = repositoryTabs.activate(id)
  if (tab?.path) await openPath(tab.path)
  else if (tab) state.close()
}
async function newTab() {
  interactionRevision++
  const tab = repositoryTabs.createNew()
  state.close()
  diffOpen.value = false
  await nextTick()
  if (activeTabId.value === tab.id) recentSearch.value?.focus()
}
function closeTab(id: string) {
  const wasActive = id === activeTabId.value
  repositoryTabs.close(id)
  if (!wasActive) return
  interactionRevision++
  const next = tabs.value.find((tab) => tab.id === activeTabId.value)
  if (next?.path) void openPath(next.path)
  else {
    state.close()
    referenceFilter.value = ''
  }
}
async function openDialog() {
  interactionRevision++
  if (opening.value) return
  showSettings.value = false
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
async function jumpToCommit(sha: string) {
  interactionRevision++
  diffOpen.value = false
  const index = await state.jump(sha)
  if (index !== null) {
    await nextTick()
    if (selectedSha.value === sha) list.value?.scrollToIndex(index)
  }
}
async function jump(reference: Reference) {
  const revision = ++interactionRevision
  const sessionId = repository.value?.id
  selection.value = 'commit'
  state.resumeDiffPreload()
  diffOpen.value = false
  let index = await state.jump(reference.sha, { reportMissing: !soloRefs.value.size })
  if (revision !== interactionRevision || sessionId !== repository.value?.id) return
  if (index === null && soloRefs.value.size && repository.value && !error.value) {
    const path = repository.value.path
    if (!(await openPath(path, true, [])) || revision !== interactionRevision) return
    const nextSession = repository.value?.id
    index = await state.jump(reference.sha, { waitForIndex: true })
    if (revision !== interactionRevision || nextSession !== repository.value?.id) return
  }
  if (index !== null) {
    await nextTick()
    if (
      revision === interactionRevision &&
      selectedIndex.value === index &&
      selectedSha.value === reference.sha
    )
      list.value?.scrollToIndex(index)
  }
}
async function navigate(direction: -1 | 1) {
  interactionRevision++
  if (selection.value === 'working' && direction === -1) return
  if (
    direction === -1 &&
    selection.value === 'commit' &&
    selectedIndex.value === 0 &&
    showWorkingRow.value
  ) {
    selectWorkingTree()
    await nextTick()
    workingRow.value?.focus()
    return
  }
  const fromWorkingTree = selection.value !== 'commit'
  const revision = interactionRevision
  const index = fromWorkingTree ? await state.selectIndex(0) : await state.moveSelection(direction)
  if (index !== null && selectedIndex.value === index && revision === interactionRevision) {
    selection.value = 'commit'
    state.resumeDiffPreload()
    list.value?.scrollToIndex(index)
    if (fromWorkingTree && selection.value === 'commit') {
      await nextTick()
      list.value?.focus()
    }
  }
}
function toggleGroup(kind: string) {
  const next = new Set(closedGroups.value)
  if (next.has(kind)) next.delete(kind)
  else next.add(kind)
  if (normalizedReferenceFilter.value) searchCollapsedGroups.value = next
  else collapsed.value = next
}
function toggleFolder(kind: Reference['kind'], path: string) {
  const next = new Set(collapsedFolders.value[kind])
  if (next.has(path)) next.delete(path)
  else next.add(path)
  collapsedFolders.value = { ...collapsedFolders.value, [kind]: next }
}
watch(filter, async () => {
  await nextTick()
  list.value?.scrollToIndex(0)
})
function keyboard(event: KeyboardEvent) {
  if (event.defaultPrevented) return
  if (handleFontSizeShortcut(event)) return
  if (event.key === 'Escape' && !showDialog.value) {
    if (showSettings.value) {
      event.preventDefault()
      void closeSettings()
    } else if (showCentralDiff.value && !resizing.value) {
      event.preventDefault()
      void closeDiff()
    }
    return
  }
  if (!event.metaKey && !event.ctrlKey) return
  if (
    event.key === ',' &&
    !event.altKey &&
    !event.shiftKey &&
    (isMac ? event.metaKey : event.ctrlKey)
  ) {
    event.preventDefault()
    openSettings()
    return
  }
  if (event.key.toLowerCase() === 'o') {
    event.preventDefault()
    void openDialog()
  }
  if (event.key.toLowerCase() === 'k' && repository.value) {
    interactionRevision++
    event.preventDefault()
    showSettings.value = false
    diffOpen.value = false
    void nextTick(() => search.value?.focus())
  }
}
onMounted(() => {
  window.addEventListener('keydown', keyboard)
  window.addEventListener('focus', refreshOnFocus)
})
onBeforeUnmount(() => {
  window.removeEventListener('keydown', keyboard)
  window.removeEventListener('focus', refreshOnFocus)
})
</script>

<template>
  <div class="flex h-dvh min-h-[540px] min-w-[880px] flex-col overflow-hidden">
    <RepositoryTabs
      v-show="!showSettings"
      :tabs="tabs"
      :active-id="activeTabId"
      @select="activateTab"
      @close="closeTab"
      @open="newTab"
    />
    <div
      v-show="!showSettings"
      ref="layoutRoot"
      :style="{ '--details-width': rightWidth + 'px' }"
      :id="activeTabId ? 'repository-tab-panel' : undefined"
      :role="activeTabId ? 'tabpanel' : undefined"
      :aria-labelledby="activeTabId ?? undefined"
      class="app-shell flex min-h-0 flex-1 overflow-hidden"
      :class="{ 'panes-resizing': resizing }"
    >
      <aside
        id="branches-pane"
        class="sidebar flex shrink-0 flex-col bg-panel border-r border-border"
        :class="{ compact: leftWidth < 210 }"
        :style="{ width: `${leftWidth}px` }"
      >
        <div class="workspace-heading">
          <span>ESPACE DE TRAVAIL</span>
        </div>
        <template v-if="repository">
          <label class="reference-search"
            ><Search :size="13" /><input
              v-model="referenceFilter"
              placeholder="Filtrer les références"
              aria-label="Filtrer les références"
          /></label>
          <nav class="references" aria-label="Références Git">
            <div
              v-for="group in groups"
              v-show="group.references.length"
              :key="group.kind"
              class="reference-group"
            >
              <button
                class="group-title"
                :aria-expanded="!closedGroups.has(group.kind)"
                @click="toggleGroup(group.kind)"
              >
                <ChevronRight v-if="closedGroups.has(group.kind)" :size="12" /><ChevronDown
                  v-else
                  :size="12"
                />
                <span>{{ group.title }}</span
                ><small>{{ count(group.references.length) }}</small>
              </button>
              <ReferenceTree
                v-if="!closedGroups.has(group.kind)"
                :references="group.references"
                :collapsed="collapsedFolders[group.kind]"
                :filter="referenceFilter"
                :current-name="latestReference?.name"
                :selected-sha="selectedSha"
                :solo-refs="soloRefs"
                @select="jump"
                @solo="toggleSolo"
                @toggle-folder="toggleFolder(group.kind, $event)"
              />
            </div>
            <p v-if="!hasReferences" class="reference-none">Aucune référence trouvée.</p>
            <RepositoryContext
              :worktrees="worktrees"
              :stashes="stashes"
              :worktrees-expanded="worktreesExpanded"
              :stashes-expanded="stashesExpanded"
              :worktrees-loading="worktreesLoading"
              :stashes-loading="stashesLoading"
              :worktrees-error="worktreesError"
              :stashes-error="stashesError"
              :selected-stash="selection === 'stash' ? stashSha : ''"
              @toggle-worktrees="toggleWorktrees"
              @toggle-stashes="toggleStashes"
              @open-worktree="openPath"
              @select-stash="selectStash"
            />
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

      <main
        class="workspace relative flex min-w-0 flex-1 flex-col"
        :class="{ welcome: !repository && !opening }"
        :aria-busy="opening"
      >
        <header class="topbar">
          <div class="breadcrumbs">
            <span>Espace local</span><ChevronRight :size="12" />
            <strong>{{
              repository?.name ??
              (opening ? tabs.find((tab) => tab.id === activeTabId)?.name : undefined) ??
              'Bienvenue'
            }}</strong>
          </div>
          <div class="topbar-actions">
            <span
              v-if="openingVisible"
              role="status"
              aria-label="Ouverture du dépôt en cours"
              class="spinner"
            ></span>
            <select v-model="theme" class="theme-select" aria-label="Thème de l’interface">
              <option v-for="item in themes" :key="item.id" :value="item.id">
                {{ item.label }}
              </option>
            </select>
            <button
              v-if="repository?.remotes.length"
              class="icon-button"
              :disabled="fetching || opening"
              :aria-busy="fetching"
              aria-label="Récupérer les références distantes"
              :title="fetching ? 'Fetch en cours…' : 'Récupérer les références de tous les remotes'"
              @click="autoFetch.fetchNow"
            >
              <Download :size="17" />
            </button>
            <button
              ref="settingsButton"
              class="icon-button"
              aria-label="Paramètres"
              :aria-keyshortcuts="isMac ? 'Meta+,' : 'Control+,'"
              :title="'Paramètres · ' + shortcut + ' ,'"
              @click="openSettings"
            >
              <Settings :size="17" />
            </button>
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
            <div class="relative flex min-h-0 min-w-0 overflow-hidden">
              <section
                class="history-panel flex min-h-0 min-w-0 flex-1 flex-col bg-reading"
                :class="{ invisible: showCentralDiff }"
                :inert="showCentralDiff"
                :aria-hidden="showCentralDiff"
                aria-label="Historique du dépôt"
              >
                <div class="history-toolbar">
                  <div class="history-title">
                    <GitBranch :size="17" />
                    <h1>Historique</h1>
                    <span class="branch-chip" title="Branche réellement active dans le worktree"
                      >HEAD · {{ repository.headName }}</span
                    >
                  </div>
                  <button
                    class="icon-button"
                    aria-label="Actualiser le dépôt"
                    title="Relire les références et les modifications locales"
                    :disabled="opening"
                    @click="refreshRepository"
                  >
                    <RefreshCw :size="14" />
                  </button>
                </div>
                <div
                  v-if="soloRefs.size"
                  class="solo-banner flex shrink-0 items-center gap-2 border-b border-warning/30 bg-warning/10 px-4 py-2 text-xs text-warning"
                  role="status"
                >
                  <span class="min-w-0 flex-1 truncate" :title="soloLabels.join(', ')"
                    >Solo : {{ soloLabels.join(', ') }}</span
                  >
                  <button
                    class="shrink-0 rounded border border-warning/30 px-2 py-1 hover:bg-warning/10"
                    @click="stopSolo"
                  >
                    Quitter le mode Solo
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
                  :selected-sha="selection === 'commit' ? selectedSha : ''"
                  :selected-index="selectedIndex"
                  :references="graphReferences"
                  :context-references="repository.references"
                  :solo-refs="soloRefs"
                  :filtered="Boolean(filter)"
                  :loading="loadingPages > 0 || opening || query.trim().slice(0, 256) !== filter"
                  :indexing="!progress.complete"
                  @range="state.loadRange"
                  @select="selectCommit"
                  @navigate="navigate"
                  @solo="toggleSolo"
                >
                  <template #working-tree>
                    <WorkingTreeRow
                      v-if="showWorkingRow"
                      ref="workingRow"
                      :status="workingStatus"
                      :selected="selection === 'working'"
                      @select="selectWorkingTree"
                      @navigate="navigate(1)"
                    />
                  </template>
                </CommitList>
                <footer class="history-footer">
                  <span v-if="!progress.complete"
                    ><CircleDot :size="11" />Indexation ·
                    {{ count(progress.indexed) }} commits</span
                  >
                  <span v-else-if="progress.error"
                    ><CircleDot :size="11" />Indexation interrompue</span
                  >
                  <span v-else><span class="ready-dot"></span>Index local prêt</span>
                  <span class="footer-hint"
                    ><Command :size="11" />{{ shortcut }} K pour rechercher</span
                  >
                </footer>
              </section>
              <FileWorkspace
                v-if="showCentralDiff && selection === 'commit'"
                :key="fileSelectionRevision"
                class="absolute inset-0"
                :session-id="repository.id"
                :sha="details?.sha ?? ''"
                :intent-sha="selectedSha"
                :file="selectedFile"
                :diff="diff"
                :loading="detailLoading || diffLoading"
                :error="detailError || diffError"
                :highlighted="highlightedDiff"
                @close="closeDiff"
                @jump-commit="jumpToCommit"
              />
              <DiffWorkspace
                v-else-if="showCentralDiff"
                class="absolute inset-0"
                :file="displayedFile"
                :diff="displayedDiff"
                :loading="displayedDiffLoading"
                :error="displayedDiffError"
                :highlighted="
                  selection === 'stash'
                    ? stashHighlightedDiff
                    : selection === 'commit'
                      ? highlightedDiff
                      : workingHighlightedDiff
                "
                @close="closeDiff"
              />
            </div>
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
            <WorkingTreePanel
              v-if="selection === 'working'"
              id="commit-details-pane"
              :key="repository.id"
              :status="workingStatus"
              :selected-file="workingFile"
              :selected-area="workingArea"
              :diff="workingDiff"
              :highlighted="workingHighlightedDiff"
              :loading="workingRefreshing"
              :diff-loading="workingDiffLoading"
              :error="workingError"
              :diff-mode="diffMode"
              @select-file="openWorkingFile"
              @diff-mode-changed="changeDiffMode"
            />
            <StashPanel
              v-else-if="selection === 'stash'"
              id="commit-details-pane"
              :details="stashDetails"
              :loading="stashLoading"
              :error="stashError"
              :selected-file="stashFile"
              :selected-area="stashArea"
              :diff="stashDiff"
              :highlighted="stashHighlightedDiff"
              :diff-loading="stashDiffLoading"
              :diff-error="stashDiffError"
              :diff-mode="diffMode"
              @select-file="openStashFile"
              @area-changed="changeStashArea"
              @diff-mode-changed="changeDiffMode"
            />
            <CommitDetailsPanel
              v-else
              id="commit-details-pane"
              :details="details"
              :loading="detailLoading"
              :error="detailError"
              :selected-file="selectedFile"
              :diff="diff"
              :diff-loading="diffLoading"
              :diff-error="diffError"
              :highlighted="highlightedDiff"
              :diff-mode="diffMode"
              @select-file="openFile"
              @diff-mode-changed="changeDiffMode"
            />
          </div>
          <div v-if="(error || fetchError) && !showDialog" class="error-toast" role="alert">
            <span class="whitespace-pre-line">{{ error || fetchError }}</span>
            <button class="icon-button" aria-label="Fermer le message" @click="dismissError">
              <X :size="14" />
            </button>
          </div>
        </template>

        <section
          v-else-if="opening"
          class="min-h-0 flex-1 bg-reading"
          aria-label="Ouverture du dépôt"
        ></section>
        <section v-else class="welcome-content">
          <div class="welcome-symbol"><img src="/favicon.svg" width="56" height="56" alt="" /></div>
          <h1>Mangrove</h1>
          <button
            ref="welcomeOpen"
            class="primary-button welcome-open"
            :disabled="opening"
            @click="openDialog"
          >
            <FolderOpen :size="17" />
            Ouvrir un dépôt local<ArrowRight :size="15" />
          </button>
          <span class="welcome-shortcut"
            >ou <kbd>{{ shortcut }} O</kbd></span
          >
          <p v-if="error && !showDialog" class="welcome-error" role="alert">{{ error }}</p>
          <div v-if="recent.length" class="recent-repositories">
            <h2>Récemment ouverts</h2>
            <label
              class="recent-search mb-2 flex min-h-9 items-center gap-2 rounded border border-border bg-bg px-2.5 text-muted"
            >
              <Search :size="14" aria-hidden="true" />
              <input
                ref="recentSearch"
                v-model="recentFilter"
                type="search"
                class="min-w-0 flex-1 border-0 bg-transparent py-2 text-xs text-text outline-none placeholder:text-muted"
                placeholder="Rechercher un dépôt récent…"
                aria-label="Rechercher un dépôt récent"
                spellcheck="false"
              />
            </label>
            <div v-for="entry in filteredRecent" :key="entry.path" class="recent-entry">
              <button
                class="recent-open"
                :disabled="opening"
                :title="`${entry.name}\n${entry.path}`"
                @click="openPath(entry.path)"
              >
                <FolderGit2 :size="16" /><span
                  >{{ entry.name }}<small>{{ entry.path }}</small></span
                ><ChevronRight :size="14" />
              </button>
              <button
                class="recent-remove"
                :aria-label="`Retirer ${entry.name} des dépôts récents`"
                title="Retirer des dépôts récents"
                :disabled="opening"
                @click="removeRecent(entry.path)"
              >
                <X :size="14" aria-hidden="true" />
              </button>
            </div>
            <p v-if="!filteredRecent.length" class="py-2 text-xs text-muted" role="status">
              Aucun dépôt récent trouvé.
            </p>
          </div>
        </section>
      </main>
    </div>
    <OpenRepositoryDialog
      :open="showDialog"
      :busy="opening"
      :error="error"
      @close="showDialog = false"
      @submit="openPath"
    />
    <SettingsView v-if="showSettings" v-model:interval="autoFetchInterval" @back="closeSettings" />
  </div>
</template>

<style scoped>
.recent-search:focus-within {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}
.recent-search input,
.recent-search input:focus-visible {
  outline: none;
  font-size: calc(12px + var(--font-size-offset));
}
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
  font-size: calc(11px + var(--font-size-offset));
  cursor: pointer;
}
.theme-select:focus-visible {
  outline: 1px solid var(--accent);
  outline-offset: 2px;
}
.sidebar.compact .sidebar-bottom {
  padding-inline: 14px;
}
.sidebar.compact .workspace-heading {
  padding-inline: 14px;
}
</style>
