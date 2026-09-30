import { createApp } from 'vue'
import App from './App.vue'
import { getStoredTheme, setTheme } from './lib/themes'
import { initializeFontSize } from './lib/fontSize'
import './style.css'

function mount() {
  setTheme(getStoredTheme())
  initializeFontSize()
  createApp(App).mount('#app')
}

if (import.meta.env.VITE_APP_BENCHMARK === '1') {
  const benchmark = (
    window as Window & {
      __MANGROVE_BENCHMARK__?: import('./lib/appBenchmark').AppBenchmarkConfig
    }
  ).__MANGROVE_BENCHMARK__
  if (benchmark) {
    void import('./lib/appBenchmark').then(({ prepareAppBenchmark, runAppBenchmark }) => {
      const restore = prepareAppBenchmark(benchmark)
      mount()
      void runAppBenchmark(benchmark, restore)
    })
  } else {
    mount()
  }
} else {
  mount()
}
