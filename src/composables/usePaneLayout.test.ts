import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test'
import { effectScope, shallowRef } from 'vue'
import { usePaneLayout } from './usePaneLayout'

const observers: MockResizeObserver[] = []
class MockResizeObserver {
  observe = vi.fn()
  disconnect = vi.fn()
  constructor(private callback: ResizeObserverCallback) {
    observers.push(this)
  }
  measure(width: number) {
    this.callback(
      [{ contentRect: { width } } as ResizeObserverEntry],
      this as unknown as ResizeObserver,
    )
  }
}

function preferences(saved: string | null = null) {
  const values = new Map<string, string>()
  if (saved !== null) values.set('mangrove:panes', saved)
  values.set('unrelated:setting', 'preserved')
  return {
    values,
    getItem: vi.fn((key: string) => values.get(key) ?? null),
    setItem: vi.fn((key: string, value: string) => void values.set(key, value)),
  }
}

const cleanup: (() => void)[] = []
function setup(width = 1440, store = preferences()) {
  const root = shallowRef<HTMLElement>({
    getBoundingClientRect: () => ({ width }) as DOMRect,
  } as HTMLElement)
  const scope = effectScope()
  const layout = scope.run(() => usePaneLayout(root, store))!
  cleanup.push(() => scope.stop())
  return { layout, scope, observer: observers.at(-1)!, store }
}

beforeEach(() => {
  observers.length = 0
  vi.stubGlobal('ResizeObserver', MockResizeObserver)
})
afterEach(() => {
  for (const dispose of cleanup.splice(0)) dispose()
  vi.unstubAllGlobals()
})

describe('pane layout', () => {
  it('bounds both panes while preserving at least 340px for history at the minimum app width', () => {
    const { layout } = setup(880)
    expect(layout.leftWidth.value).toBe(226)
    expect(layout.rightWidth.value).toBe(302)

    layout.resize('left', 900)
    layout.resize('right', 900)
    expect(layout.leftWidth.value).toBe(248)
    expect(layout.rightWidth.value).toBe(280)
    expect(880 - layout.leftWidth.value - layout.rightWidth.value - 12).toBe(340)

    layout.resize('left', -100)
    layout.resize('right', -100)
    expect(layout.leftWidth.value).toBe(180)
    expect(layout.rightWidth.value).toBe(280)
  })

  it('adapts to a smaller window and restores preferred widths on expansion without overwriting preferences', () => {
    const store = preferences(JSON.stringify({ left: 400, right: 700 }))
    const { layout, observer } = setup(1600, store)
    expect(layout.leftWidth.value).toBe(400)
    expect(layout.rightWidth.value).toBe(700)

    observer.measure(980)
    expect(layout.leftWidth.value).toBe(348)
    expect(layout.rightWidth.value).toBe(280)
    expect(980 - layout.leftWidth.value - layout.rightWidth.value - 12).toBe(340)
    observer.measure(1600)
    expect(layout.leftWidth.value).toBe(400)
    expect(layout.rightWidth.value).toBe(700)
    expect(store.setItem).not.toHaveBeenCalled()
  })

  it('persists committed changes under the app key and restores them in another app session', () => {
    const { layout, store } = setup()
    layout.resize('left', 300)
    layout.startResize('right')
    layout.resize('right', 420)
    layout.resize('right', 510)
    expect(store.setItem).toHaveBeenCalledTimes(1)
    layout.finishResize()
    expect(store.setItem).toHaveBeenCalledTimes(2)

    const restored = setup(1440, store).layout
    expect(restored.leftWidth.value).toBe(300)
    expect(restored.rightWidth.value).toBe(510)
    expect(store.values.get('unrelated:setting')).toBe('preserved')
  })

  it('restores the original preference when pointer cancellation ends a resize', () => {
    const { layout, observer, store } = setup(
      1440,
      preferences(JSON.stringify({ left: 226, right: 600 })),
    )
    observer.measure(980)
    const originalWidth = layout.rightWidth.value
    layout.startResize('right')
    layout.resize('right', 280)
    expect(layout.resizing.value).toBe(true)
    layout.finishResize(true)
    expect(layout.resizing.value).toBe(false)
    expect(layout.rightWidth.value).toBe(originalWidth)
    observer.measure(1440)
    expect(layout.rightWidth.value).toBe(600)
    expect(store.setItem).not.toHaveBeenCalled()
  })

  it('resets each pane independently to the default width', () => {
    const { layout, observer } = setup()
    layout.resize('left', 350)
    layout.resize('right', 600)
    layout.reset('left')
    expect(layout.leftWidth.value).toBe(226)
    expect(layout.rightWidth.value).toBe(600)
    observer.measure(880)
    layout.reset('right')
    expect(layout.rightWidth.value).toBe(302)
    observer.measure(1440)
    expect(layout.rightWidth.value).toBe(390)
  })

  it('ignores malformed stored values and stays usable if preferences cannot be written', () => {
    const brokenJson = setup(1440, preferences('{broken')).layout
    expect(brokenJson.leftWidth.value).toBe(226)
    expect(brokenJson.rightWidth.value).toBe(390)

    const store = preferences(JSON.stringify({ left: '400', right: -20 }))
    store.setItem.mockImplementation(() => {
      throw new Error('Unavailable store')
    })
    const { layout } = setup(1440, store)
    expect(layout.leftWidth.value).toBe(226)
    expect(layout.rightWidth.value).toBe(280)
    expect(() => layout.resize('right', 450)).not.toThrow()
    layout.resize('left', Number.NaN)
    expect(layout.leftWidth.value).toBe(226)
    expect(layout.rightWidth.value).toBe(450)
  })

  it('disconnects measurements when the owning scope is disposed', () => {
    const { scope, observer } = setup()
    scope.stop()
    expect(observer.disconnect).toHaveBeenCalledOnce()
  })
})
