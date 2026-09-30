import { createApp } from 'vue'
import App from './App.vue'
import { getStoredTheme, setTheme } from './lib/themes'
import './style.css'

setTheme(getStoredTheme())
createApp(App).mount('#app')
