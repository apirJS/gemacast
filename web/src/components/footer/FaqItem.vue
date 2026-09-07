<script setup lang="ts">
import { PlusIcon } from '@lucide/vue'

const props = defineProps<{
  question: string
  answer: string
  isOpen: boolean
}>()

const emit = defineEmits<{ toggle: [] }>()

const panelId = `faq-panel-${props.question.toLowerCase().replace(/[^a-z0-9]+/g, '-')}`
</script>

<template>
  <li class="border border-primary bg-card">
    <h3>
      <button
        type="button"
        class="font-mono flex w-full cursor-pointer items-center justify-between gap-x-3 px-4 py-3.5 text-left text-sm font-bold text-primary transition-colors sm:px-5 sm:text-base sm:hover:bg-secondary"
        :class="{ 'bg-secondary': isOpen }"
        :aria-expanded="isOpen"
        :aria-controls="panelId"
        @click="emit('toggle')"
      >
        <span>{{ question }}</span>
        <PlusIcon
          class="size-4 shrink-0 transition-transform duration-200"
          :class="isOpen ? 'rotate-45' : 'rotate-0'"
        />
      </button>
    </h3>

    <p
      v-show="isOpen"
      :id="panelId"
      class="font-mono border-t border-border px-4 py-3.5 text-xs leading-relaxed text-secondary-foreground sm:px-5 sm:text-sm"
    >
      {{ answer }}
    </p>
  </li>
</template>
