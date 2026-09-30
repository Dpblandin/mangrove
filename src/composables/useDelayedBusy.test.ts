import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test'
import { effectScope, isReadonly, ref } from 'vue'
import { useDelayedBusy } from './useDelayedBusy'

const cleanup: (() => void)[] = []
function setup(initialBusy = false, delayMs = 500) {
  const busy = ref(initialBusy)
  const scope = effectScope()
  const visible = scope.run(() => useDelayedBusy(() => busy.value, delayMs))!
  cleanup.push(() => scope.stop())
  return { busy, visible, scope }
}

beforeEach(() => vi.useFakeTimers())
afterEach(() => {
  for (const dispose of cleanup.splice(0)) dispose()
  vi.useRealTimers()
})

describe('delayed busy indicator', () => {
  it('starts immediately for an already busy operation and appears only at 500 ms', () => {
    const { busy, visible } = setup(true)
    expect(visible.value).toBe(false)
    vi.advanceTimersByTime(499)
    expect(visible.value).toBe(false)
    vi.advanceTimersByTime(1)
    expect(visible.value).toBe(true)
    expect(isReadonly(visible)).toBe(true)
    busy.value = false
    expect(visible.value).toBe(false)
    expect(vi.getTimerCount()).toBe(0)
  })

  it('never flashes when the operation completes before the delay', () => {
    const { busy, visible } = setup()
    expect(vi.getTimerCount()).toBe(0)
    busy.value = true
    vi.advanceTimersByTime(499)
    busy.value = false
    expect(visible.value).toBe(false)
    expect(vi.getTimerCount()).toBe(0)
    vi.advanceTimersByTime(1_000)
    expect(visible.value).toBe(false)
  })

  it('restarts the delay synchronously without applying a timer from the previous operation', () => {
    const { busy, visible } = setup(true)
    vi.advanceTimersByTime(400)
    busy.value = false
    busy.value = true
    vi.advanceTimersByTime(100)
    expect(visible.value).toBe(false)
    vi.advanceTimersByTime(399)
    expect(visible.value).toBe(false)
    vi.advanceTimersByTime(1)
    expect(visible.value).toBe(true)
    busy.value = false
    busy.value = true
    expect(visible.value).toBe(false)
    vi.advanceTimersByTime(499)
    expect(visible.value).toBe(false)
  })

  it('uses the caller’s delay', () => {
    const { visible } = setup(true, 200)
    vi.advanceTimersByTime(199)
    expect(visible.value).toBe(false)
    vi.advanceTimersByTime(1)
    expect(visible.value).toBe(true)
  })

  it.each([false, true])(
    'releases an active or completed timer on disposal (visible=%s)',
    (shown) => {
      const { busy, visible, scope } = setup(true)
      vi.advanceTimersByTime(shown ? 500 : 400)
      expect(visible.value).toBe(shown)
      scope.stop()
      expect(visible.value).toBe(false)
      expect(vi.getTimerCount()).toBe(0)
      busy.value = false
      busy.value = true
      vi.advanceTimersByTime(1_000)
      expect(visible.value).toBe(false)
      expect(vi.getTimerCount()).toBe(0)
    },
  )
})
