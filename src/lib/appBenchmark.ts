import { invoke } from '@tauri-apps/api/core'
import { nextTick } from 'vue'

export type AppBenchmarkConfig = { repository: string; samples: number }
type Frames = { interval_ms: number[]; draw_ms: number[]; submit_ms: number[] }
type Report = {
  schema_version: number
  implementation: string
  readiness_proxy: string
  frame_proxy: string
  status: string
  timings_ms: Record<string, number[]>
  frames: Record<string, Frames>
  workload: Record<string, number>
  elapsed_ms: number
  time_origin_unix_ms: number
  failure: string | null
  failure_snapshot?: Record<string, number>
}

const syntaxPaths = [
  'apps/frontend/src/composables/useDiff.ts',
  'apps/frontend/src/stores/repositories.ts',
]
const heavyPath = 'src/generated/catalog.ts'
const historySelector = '.commit-scroll'
const detailsSelector = '[aria-label="Détails du commit"]'
const diffSelector = '[aria-label="Diff du fichier, défilement horizontal et vertical"]'
let observedFrames = 0

/** Called before Vue mounts, only when native benchmark initialization opted in. */
export function prepareAppBenchmark(config: AppBenchmarkConfig) {
  const saved = new Map<string, string>()
  for (const key of Object.keys(localStorage)) {
    if (!key.startsWith('mangrove:')) continue
    const value = localStorage.getItem(key)
    if (value !== null) saved.set(key, value)
    localStorage.removeItem(key)
  }
  localStorage.setItem('mangrove:auto-fetch-interval', '0')
  localStorage.setItem('mangrove:theme', 'classic')
  // The desktop welcome button opens an OS picker. A synthetic recent entry
  // reaches the same real openPath handler without any OS UI automation.
  localStorage.setItem(
    'mangrove:recent',
    JSON.stringify([{ path: config.repository, name: 'Benchmark fixture' }]),
  )
  return () => {
    for (const key of Object.keys(localStorage))
      if (key.startsWith('mangrove:')) localStorage.removeItem(key)
    for (const [key, value] of saved) localStorage.setItem(key, value)
  }
}

function frame(timeout = 30_000) {
  return new Promise<number>((resolve, reject) => {
    const request = requestAnimationFrame((timestamp) => {
      clearTimeout(watchdog)
      observedFrames++
      resolve(timestamp)
    })
    const watchdog = setTimeout(() => {
      cancelAnimationFrame(request)
      reject(new Error('Timed out waiting for an application frame'))
    }, timeout)
  })
}
async function renderingOpportunity() {
  await nextTick()
  await frame()
  await frame()
}

function element(selector: string): HTMLElement {
  const value = document.querySelector<HTMLElement>(selector)
  if (!value) throw new Error('A required benchmark control is absent')
  return value
}

async function until(ready: () => boolean, timeout = 30_000) {
  const deadline = performance.now() + timeout
  while (!ready()) {
    if (performance.now() > deadline) throw new Error('Timed out waiting for application readiness')
    await frame(Math.max(1, deadline - performance.now()))
  }
}

function visibleRows() {
  const viewport = document.querySelector<HTMLElement>(historySelector)
  if (!viewport) return []
  const bounds = viewport.getBoundingClientRect()
  return [...viewport.querySelectorAll<HTMLElement>('.commit-row')].filter((row) => {
    const rect = row.getBoundingClientRect()
    return rect.bottom > bounds.top && rect.top < bounds.bottom
  })
}

function detailsReady(sha?: string) {
  const details = document.querySelector<HTMLElement>(detailsSelector)
  const current = details?.querySelector('.commit-sha')?.textContent?.trim()
  return (
    details?.getAttribute('aria-busy') === 'false' && Boolean(current) && (!sha || current === sha)
  )
}

function diffReady(colored: boolean) {
  const viewer = document.querySelector('.diff-viewer')
  return (
    viewer?.getAttribute('aria-busy') === 'false' &&
    Boolean(viewer.querySelector('.diff-line')) &&
    (!colored || Boolean(viewer.querySelector('[class^="syntax-"]:not(.syntax-plain)')))
  )
}

async function scrollFileListTo(path: string) {
  const list = element(`${detailsSelector} [role="listbox"]`)
  for (
    let position = 0;
    position <= list.scrollHeight;
    position += Math.max(100, list.clientHeight / 2)
  ) {
    const match = [...list.querySelectorAll<HTMLButtonElement>('.file-row')].find((row) =>
      row.getAttribute('aria-label')?.startsWith(`${path},`),
    )
    if (match) return match
    list.scrollTop = position
    await renderingOpportunity()
  }
  throw new Error('A synthetic benchmark target file is absent')
}

async function scrollWorkload(selector: string): Promise<Frames> {
  const viewport = element(selector)
  viewport.scrollTop = 0
  const intervals: number[] = []
  let previous = await frame()
  const started = previous
  while (previous - started < 3_000) {
    viewport.scrollTop = Math.min(
      viewport.scrollHeight - viewport.clientHeight,
      (previous - started) * 2.4,
    )
    // The browser emits its normal scroll event; the real virtualization and
    // graph handlers run once, as they do during user scrolling.
    const current = await frame()
    intervals.push(current - previous)
    previous = current
    if (intervals.length >= 2_000) throw new Error('Benchmark frame sample bound exceeded')
  }
  return { interval_ms: intervals, draw_ms: [], submit_ms: [] }
}

/** Executes actual UI handlers inside the packaged Tauri WebView. */
export async function runAppBenchmark(config: AppBenchmarkConfig, restore: () => void) {
  const report: Report = {
    schema_version: 1,
    implementation: 'tauri',
    readiness_proxy:
      'DOM-ready observed at frame cadence then two requestAnimationFrame opportunities',
    frame_proxy: 'WebView requestAnimationFrame intervals (not display scanout)',
    status: 'ok',
    timings_ms: {},
    frames: {},
    workload: {
      history_total: 0,
      changed_files: 0,
      heavy_diff_rows: 0,
      heavy_diff_colored: 0,
      selection_samples: 0,
      syntax_samples: 0,
      scroll_duration_ms: 3_000,
      scroll_speed_px_s: 2_400,
      viewport_width: innerWidth,
      viewport_height: innerHeight,
    },
    elapsed_ms: 0,
    time_origin_unix_ms: performance.timeOrigin,
    failure: null,
  }
  const samples = Math.floor(Math.max(2, Math.min(100, config.samples)) / 2) * 2
  const record = (name: string, milliseconds: number) => {
    ;(report.timings_ms[name] ??= []).push(milliseconds)
  }
  const phase = (name: string) => invoke('app_benchmark_phase', { phase: name })
  const measure = async (name: string, action: () => void, ready: () => boolean) => {
    const started = performance.now()
    action()
    await until(ready)
    record(`${name}_cpu_ready`, performance.now() - started)
    await renderingOpportunity()
    record(name, performance.now() - started)
  }
  try {
    await phase('startup')
    element('.recent-open').click()
    await until(
      () =>
        visibleRows().length >= 10 && detailsReady() && Boolean(document.querySelector('canvas')),
    )
    record('startup_history_cpu_ready', performance.now())
    await renderingOpportunity()
    record('startup_history', performance.now())
    await phase('index')
    await until(() => Boolean(document.querySelector('.ready-dot')))
    report.workload.history_total = Number(element('.commit-row').getAttribute('aria-setsize'))
    await phase('selection')
    for (let sample = 0; sample < samples; sample++) {
      const target = element(`#commit-option-${sample % 2 === 0 ? 1 : 0}`)
      const sha = target.querySelector('.sha-column')?.textContent?.trim()
      const before = element(`${detailsSelector} .commit-sha`).textContent?.trim()
      await measure(
        'select_details',
        () => target.click(),
        () => {
          const current = document
            .querySelector(`${detailsSelector} .commit-sha`)
            ?.textContent?.trim()
          return detailsReady() && current !== before && Boolean(sha && current?.startsWith(sha))
        },
      )
    }
    report.workload.selection_samples = samples
    report.workload.changed_files = Number(
      element(`${detailsSelector} .file-row`).getAttribute('aria-setsize'),
    )
    await phase('syntax')
    for (let sample = 0; sample < samples; sample++) {
      const target = await scrollFileListTo(syntaxPaths[sample % 2]!)
      await measure(
        'select_colored_diff',
        () => target.click(),
        () => target.getAttribute('aria-selected') === 'true' && diffReady(true),
      )
    }
    report.workload.syntax_samples = samples
    await phase('heavy_diff')
    const target = await scrollFileListTo(heavyPath)
    await measure(
      'open_heavy_diff',
      () => target.click(),
      () =>
        target.getAttribute('aria-selected') === 'true' &&
        diffReady(false) &&
        document.querySelectorAll('.diff-viewer .diff-line').length === 2_000,
    )
    report.workload.heavy_diff_rows = document.querySelectorAll('.diff-viewer .diff-line').length
    report.workload.heavy_diff_colored = Number(
      Boolean(document.querySelector('.diff-viewer [class^="syntax-"]:not(.syntax-plain)')),
    )
    await phase('diff_scroll')
    report.frames.diff_scroll = await scrollWorkload(diffSelector)
    await phase('close_diff')
    element('[aria-label="Fermer le fichier"]').click()
    await renderingOpportunity()
    await phase('history_scroll')
    report.frames.history_scroll = await scrollWorkload(historySelector)
    await phase('idle')
    await new Promise((resolve) => setTimeout(resolve, 2_000))
  } catch {
    report.status = 'failed'
    report.failure_snapshot = {
      observed_frames: observedFrames,
      document_visible: Number(document.visibilityState === 'visible'),
      document_focused: Number(document.hasFocus()),
      history_rows: document.querySelectorAll('.commit-row').length,
      visible_history_rows: visibleRows().length,
      details_ready: Number(detailsReady()),
      graph_canvases: document.querySelectorAll('canvas').length,
    }
    // Source paths, SHA, content and native error messages never reach the report.
    report.failure = 'The application did not complete the required visual workload'
  } finally {
    report.elapsed_ms = performance.now()
    restore()
    await invoke('app_benchmark_report', { report: JSON.stringify(report) })
  }
}
