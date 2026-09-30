import { onScopeDispose, ref, watch, type Ref } from 'vue'
import { request, type GitClient } from '../lib/api'
import { validAutoFetchInterval } from '../lib/autoFetch'
import type { FetchResult, RepositoryInfo } from '../lib/types'

export function useAutoFetch(
  repository: Readonly<Ref<RepositoryInfo | null>>,
  interval: Ref<number>,
  refresh: () => Promise<void>,
  client: GitClient = request,
) {
  const fetching = ref(false)
  const error = ref('')
  let disposed = false
  let revision = 0
  let timer: ReturnType<typeof setTimeout> | undefined

  function schedule() {
    clearTimeout(timer)
    timer = undefined
    if (
      disposed ||
      fetching.value ||
      !repository.value?.remotes.length ||
      !validAutoFetchInterval(interval.value) ||
      interval.value === 0
    )
      return
    timer = setTimeout(() => void fetchNow(), interval.value * 60_000)
  }

  async function fetchNow() {
    const repo = repository.value
    if (disposed || fetching.value || !repo?.remotes.length) return
    clearTimeout(timer)
    fetching.value = true
    error.value = ''
    const generation = revision
    try {
      const result = await client<FetchResult>('fetch_repository', { sessionId: repo.id })
      if (disposed || generation !== revision) return
      // A remote can succeed even when another fails; publish those updates too.
      error.value = result.error ?? ''
      await refresh()
    } catch (cause) {
      if (!disposed && generation === revision)
        error.value = `Impossible de récupérer les références distantes : ${String(cause)}`
    } finally {
      fetching.value = false
      schedule()
    }
  }

  watch(
    () => repository.value,
    (repo, previous) => {
      if (repo?.id === previous?.id) return
      revision++
      if (repo?.path !== previous?.path) error.value = ''
      schedule()
    },
    { flush: 'sync' },
  )
  watch(interval, schedule, { immediate: true, flush: 'sync' })
  onScopeDispose(() => {
    disposed = true
    revision++
    clearTimeout(timer)
  })

  return { fetching, error, fetchNow }
}
