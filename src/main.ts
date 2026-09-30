import { createApp } from 'vue'
import App from './App.vue'
import { getStoredTheme, setTheme } from './lib/themes'
import { initializeFontSize } from './lib/fontSize'
import './style.css'

setTheme(getStoredTheme())
initializeFontSize()
createApp(App).mount('#app')
