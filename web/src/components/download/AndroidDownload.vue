<script setup lang="ts">
import { CodeIcon } from '@lucide/vue'
import { links } from '@/core/links'
import ActionButton from '@/components/shared/ActionButton.vue'
import AndroidLogo from '@/components/shared/AndroidLogo.vue'

const details: string[] = ['Android 8.0 or newer']
</script>

<template>
  <article class="offset-shadow flex flex-col border border-primary bg-card">
    <header
      class="flex items-center gap-x-3 border-b border-primary bg-secondary px-4 py-3 sm:px-5 sm:py-4"
    >
      <span class="border border-primary bg-card p-2 text-primary">
        <AndroidLogo class="size-5" />
      </span>
      <div class="flex flex-col">
        <h3 class="font-[Black_Ops_One] text-lg text-primary sm:text-xl">The player</h3>
        <p class="font-mono text-xs text-muted-foreground">Plays the audio on your phone</p>
      </div>
    </header>

    <div class="flex flex-col gap-4 p-4 sm:p-5">
      <ul class="flex flex-col gap-y-2">
        <li
          v-for="detail in details"
          :key="detail"
          class="font-mono flex items-start gap-x-2 text-xs text-secondary-foreground sm:text-sm"
        >
          <span class="mt-1.5 size-1.5 shrink-0 border border-primary bg-accent-green"></span>
          <span>{{ detail }}</span>
        </li>
      </ul>

      <div class="flex flex-col gap-2">
        <ActionButton :href="links.androidApk" external>
          <AndroidLogo class="size-4" />
          <span>Download APK</span>
        </ActionButton>
        <ActionButton :href="links.repo" variant="secondary" external>
          <CodeIcon class="size-4" />
          <span>Read the source</span>
        </ActionButton>
      </div>
    </div>
  </article>
</template>
