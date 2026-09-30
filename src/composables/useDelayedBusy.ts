import { onScopeDispose, readonly, ref, watch, type Ref } from 'vue'

export function useDelayedBusy(busy: () => boolean, delayMs = 500): Readonly<Ref<boolean>> {
  const visible = ref(false)
  let timer: ReturnType<typeof setTimeout> | undefined
  let generation = 0

  function reset() {
    generation += 1
    clearTimeout(timer)
    timer = undefined
    visible.value = false
  }

  watch(
    busy,
    (currentBusy) => {
      reset()
      if (!currentBusy) return
      const current = generation
      timer = setTimeout(() => {
        if (current !== generation) return
        timer = undefined
        if (busy()) visible.value = true
      }, delayMs)
    },
    { immediate: true, flush: 'sync' },
  )

  onScopeDispose(reset)
  return readonly(visible)
}
