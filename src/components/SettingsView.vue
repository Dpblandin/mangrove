<script setup lang="ts">
import { computed, ref } from 'vue'
import { AlertTriangle, ArrowLeft, Check, Settings, SlidersHorizontal } from '@lucide/vue'
import { MAX_AUTO_FETCH_INTERVAL, parseAutoFetchInterval } from '../lib/autoFetch'

const props = defineProps<{ interval: number }>()
const emit = defineEmits<{ back: []; 'update:interval': [value: number] }>()
const input = ref<HTMLInputElement>()
const draft = ref<string | number>(String(props.interval))
const error = ref('')
const saved = ref(false)
const changed = computed(() => String(draft.value) !== String(props.interval))

function clearFeedback() {
  error.value = ''
  saved.value = false
}

function reset() {
  draft.value = String(props.interval)
  clearFeedback()
}

function save() {
  const value = parseAutoFetchInterval(draft.value)
  if (value === null) {
    error.value = `Indiquez un nombre entier entre 0 et ${MAX_AUTO_FETCH_INTERVAL}.`
    input.value?.focus()
    return
  }
  emit('update:interval', value)
  error.value = ''
  saved.value = true
}
</script>

<template>
  <div class="settings-view flex min-h-0 flex-1 flex-col bg-bg">
    <header class="flex min-h-[64px] shrink-0 items-center gap-6 border-b border-border px-6">
      <button
        type="button"
        class="settings-back inline-flex items-center gap-2 rounded px-2 py-2 text-xs text-muted hover:bg-hover hover:text-text"
        aria-label="Retour à l’espace de travail"
        @click="emit('back')"
      >
        <ArrowLeft :size="16" aria-hidden="true" />Retour à l’espace de travail
        <kbd class="ml-2 text-[10px] opacity-70" aria-hidden="true">Esc</kbd>
      </button>
      <span class="h-5 border-l border-border" aria-hidden="true"></span>
      <h1 class="flex items-center gap-2.5 text-base font-medium">
        <Settings :size="18" class="text-accent" aria-hidden="true" />Paramètres
      </h1>
    </header>

    <div class="flex min-h-0 flex-1">
      <aside class="w-[226px] shrink-0 border-r border-border bg-panel px-3 py-6">
        <nav aria-label="Rubriques des paramètres">
          <button
            type="button"
            class="flex w-full cursor-pointer items-center gap-3 rounded-md bg-accent/10 px-4 py-3 text-left text-xs font-medium text-accent"
            aria-current="page"
            @click="input?.focus()"
          >
            <SlidersHorizontal :size="16" aria-hidden="true" />Général
          </button>
        </nav>
      </aside>

      <main class="min-h-0 min-w-0 flex-1 overflow-y-auto px-10 py-9">
        <section class="max-w-[760px]" aria-labelledby="settings-general-title">
          <h2 id="settings-general-title" class="text-2xl font-semibold">Général</h2>
          <p class="mt-2 text-xs leading-relaxed text-muted">
            Réglez la fréquence de récupération des références distantes.
          </p>

          <form class="mt-8" @submit.prevent="save" novalidate>
            <div class="rounded-xl border border-border bg-panel p-6">
              <h3 class="mb-6 text-sm font-medium">Auto-fetch</h3>
              <div class="grid grid-cols-[200px_minmax(0,1fr)] items-start gap-5">
                <label for="auto-fetch-interval" class="pt-2 text-xs">
                  Intervalle d’auto-fetch
                </label>
                <div>
                  <div class="flex items-center gap-3">
                    <input
                      id="auto-fetch-interval"
                      ref="input"
                      v-model="draft"
                      type="number"
                      min="0"
                      :max="MAX_AUTO_FETCH_INTERVAL"
                      step="1"
                      required
                      class="h-9 w-[120px] rounded border border-border bg-bg px-3 text-xs text-text focus:border-accent focus:outline-none"
                      :aria-describedby="
                        'auto-fetch-help auto-fetch-warning' + (error ? ' auto-fetch-error' : '')
                      "
                      :aria-invalid="Boolean(error)"
                      @input="clearFeedback"
                    />
                    <span class="text-xs text-muted">minutes</span>
                  </div>
                  <p id="auto-fetch-help" class="mt-3 text-xs leading-relaxed text-muted">
                    Entre 0 et {{ MAX_AUTO_FETCH_INTERVAL }} minutes. 0 désactive l’auto-fetch.
                    Récupère les références de tous les remotes du dépôt actif.
                  </p>
                  <p
                    v-if="error"
                    id="auto-fetch-error"
                    class="mt-2 text-xs text-danger"
                    role="alert"
                  >
                    {{ error }}
                  </p>
                </div>
              </div>
              <p
                id="auto-fetch-warning"
                class="mt-6 flex items-start gap-2 border-t border-border pt-4 text-xs leading-relaxed text-muted"
              >
                <AlertTriangle :size="14" class="mt-0.5 shrink-0" aria-hidden="true" />
                <span>
                  De nombreux remotes peuvent affecter les performances. Les objets LFS ne sont pas
                  récupérés automatiquement.
                </span>
              </p>
            </div>

            <div class="mt-5 flex min-h-9 items-center justify-between gap-4">
              <span class="inline-flex items-center gap-2 text-xs text-accent" role="status">
                <template v-if="saved">
                  <Check :size="14" aria-hidden="true" />Paramètres enregistrés.
                </template>
              </span>
              <div class="flex gap-3">
                <button
                  type="button"
                  class="settings-button border-border text-muted"
                  :disabled="!changed"
                  @click="reset"
                >
                  Annuler les modifications
                </button>
                <button
                  type="submit"
                  class="settings-button border-transparent bg-accent text-on-accent"
                  :disabled="!changed"
                >
                  Enregistrer
                </button>
              </div>
            </div>
          </form>
        </section>
      </main>
    </div>
  </div>
</template>

<style scoped>
@reference '../style.css';

.settings-back,
.settings-button {
  cursor: pointer;
}
.settings-button {
  @apply inline-flex min-h-[37px] items-center justify-center rounded-md border px-3.5 text-xs font-medium hover:brightness-110;
}
.settings-button:disabled {
  cursor: default;
}
</style>
