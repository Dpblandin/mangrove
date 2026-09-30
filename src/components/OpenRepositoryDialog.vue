<script setup lang="ts">
import { nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { AlertCircle, ArrowRight, FolderOpen, LoaderCircle, ShieldCheck, X } from '@lucide/vue'
import { chooseFolder, desktop } from '../lib/api'

const props = defineProps<{
  open: boolean
  busy: boolean
  error: string
}>()

const emit = defineEmits<{
  close: []
  submit: [path: string]
}>()

const dialog = ref<HTMLDialogElement | null>(null)
const input = ref<HTMLInputElement | null>(null)
const path = ref('')
const validationError = ref('')
const choosing = ref(false)

watch(
  () => props.open,
  async (open) => {
    await nextTick()
    if (open) {
      validationError.value = ''
      if (!dialog.value?.open) dialog.value?.showModal()
      input.value?.focus()
    } else {
      dialog.value?.close()
    }
  },
  { immediate: true, flush: 'post' },
)

onBeforeUnmount(() => dialog.value?.close())

function close() {
  if (!props.busy && !choosing.value) emit('close')
}

function submit() {
  if (props.busy || choosing.value) return
  const value = path.value.trim()
  if (!value) {
    validationError.value = 'Indiquez le chemin du dossier qui contient votre dépôt Git.'
    input.value?.focus()
    return
  }
  validationError.value = ''
  emit('submit', value)
}

async function browse() {
  if (props.busy || choosing.value) return
  choosing.value = true
  validationError.value = ''
  try {
    const folder = await chooseFolder()
    if (folder) path.value = folder
  } catch (error) {
    validationError.value =
      error instanceof Error ? error.message : 'Impossible d’ouvrir le sélecteur de dossier.'
  } finally {
    choosing.value = false
    input.value?.focus()
  }
}
</script>

<template>
  <dialog
    ref="dialog"
    class="repository-dialog fixed inset-0 m-auto h-fit w-[480px] max-w-[calc(100vw_-_40px)] max-h-[calc(100vh_-_40px)] rounded-[14px] border border-border bg-panel p-0 text-text shadow-[0_24px_100px_#0009]"
    aria-labelledby="repository-dialog-title"
    aria-describedby="repository-dialog-description"
    @cancel.prevent="close"
    @click="$event.target === dialog && close()"
  >
    <div class="relative p-[30px] max-[500px]:p-6">
      <button
        class="absolute top-4 right-4 grid size-7 cursor-pointer place-items-center rounded-[5px] border-0 bg-transparent p-0 text-muted hover:bg-surface hover:text-text disabled:cursor-wait disabled:opacity-55"
        type="button"
        aria-label="Fermer"
        :disabled="busy || choosing"
        @click="close"
      >
        <X :size="17" />
      </button>
      <span
        class="mb-[21px] grid size-[50px] place-items-center rounded-xl border border-accent/25 bg-accent/9 text-accent"
        aria-hidden="true"
        ><FolderOpen :size="25" :stroke-width="1.6"
      /></span>
      <span class="block text-[9px] font-semibold tracking-[1.6px] text-accent"
        >VOTRE HISTORIQUE, EN CLAIR</span
      >
      <h2
        id="repository-dialog-title"
        class="mt-[9px] mb-2.5 text-[25px] font-medium tracking-[-.65px]"
      >
        Ouvrir un dépôt
      </h2>
      <p id="repository-dialog-description" class="mb-[26px] text-xs leading-[1.8] text-muted">
        Explorez les branches, les commits et les modifications d’un dépôt Git présent sur votre
        ordinateur.
      </p>

      <form @submit.prevent="submit">
        <label for="repository-path" class="mb-[9px] block text-[11px] font-medium"
          >Chemin du dépôt</label
        >
        <div
          class="flex h-11 items-center gap-[9px] rounded-[7px] border bg-bg px-2.5 focus-within:ring-2 focus-within:ring-accent/10"
          :class="
            validationError || error
              ? 'border-danger focus-within:border-danger'
              : 'border-border focus-within:border-accent'
          "
        >
          <FolderOpen :size="16" class="shrink-0 text-muted" aria-hidden="true" />
          <input
            class="w-full min-w-0 flex-1 border-0 bg-transparent font-mono text-[11px] text-text outline-none placeholder:text-muted disabled:cursor-wait disabled:opacity-55"
            id="repository-path"
            ref="input"
            v-model="path"
            type="text"
            placeholder="/Users/vous/Developer/mon-projet"
            autocomplete="off"
            spellcheck="false"
            :disabled="busy || choosing"
            :aria-invalid="!!(validationError || error)"
            :aria-describedby="
              validationError || error ? 'repository-error' : 'repository-path-help'
            "
            @input="validationError = ''"
          />
          <button
            v-if="desktop"
            type="button"
            class="shrink-0 cursor-pointer rounded border border-border bg-surface px-[7px] py-[5px] text-[10px] text-text disabled:cursor-wait disabled:opacity-55"
            :disabled="busy || choosing"
            @click="browse"
          >
            {{ choosing ? '…' : 'Parcourir' }}
          </button>
        </div>
        <p
          v-if="validationError || error"
          id="repository-error"
          class="mt-[9px] flex items-start gap-[7px] text-[11px] leading-relaxed text-danger [overflow-wrap:anywhere]"
          role="alert"
        >
          <AlertCircle :size="14" class="mt-0.5 shrink-0" /><span>{{
            validationError || error
          }}</span>
        </p>
        <p v-else id="repository-path-help" class="mt-[9px] text-[10px] leading-relaxed text-muted">
          Choisissez le dossier du projet. Les dépôts bare et les worktrees sont aussi pris en
          charge.
        </p>

        <div
          class="mt-[23px] flex items-start gap-2.5 rounded-[7px] border border-border bg-accent/3 p-3.5"
        >
          <ShieldCheck :size="16" class="mt-0.5 shrink-0 text-accent" />
          <p class="m-0 text-[11px] leading-relaxed">
            Votre dépôt reste intact.<br /><span class="text-[10px] text-muted"
              >Mangrove lit votre historique sans modifier vos fichiers ni vos branches.</span
            >
          </p>
        </div>
        <div class="mt-[25px] flex items-center justify-end gap-2.5 max-[500px]:flex-wrap">
          <button
            type="button"
            class="dialog-button border-border bg-transparent text-muted hover:bg-surface hover:text-text"
            :disabled="busy || choosing"
            @click="close"
          >
            Annuler
          </button>
          <button
            type="submit"
            class="dialog-button border-transparent bg-accent text-on-accent hover:brightness-110"
            :disabled="busy || choosing"
          >
            <LoaderCircle
              v-if="busy"
              class="animate-spin motion-reduce:animate-none"
              :size="15"
            /><span>{{ busy ? 'Ouverture…' : 'Explorer le dépôt' }}</span
            ><ArrowRight v-if="!busy" :size="15" />
          </button>
        </div>
      </form>
    </div>
  </dialog>
</template>

<style scoped>
@reference '../style.css';

.repository-dialog::backdrop {
  background: color-mix(in srgb, var(--bg) 85%, transparent);
  backdrop-filter: blur(5px);
}
.dialog-button {
  @apply inline-flex min-h-[37px] cursor-pointer items-center justify-center gap-[9px] rounded-md border px-3.5 text-[11px] font-medium disabled:cursor-wait disabled:opacity-55;
}
</style>
