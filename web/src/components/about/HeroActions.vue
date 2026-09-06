<script setup lang="ts">
import { CodeIcon, DownloadIcon } from '@lucide/vue'
import { links } from '@/core/links'
import ActionButton from '@/components/shared/ActionButton.vue'
</script>

<template>
  <div class="flex w-full flex-col items-center gap-y-4">
    <div class="flex w-full max-w-md flex-col gap-3 px-4 sm:w-auto sm:flex-row sm:px-0">
      <ActionButton href="#download">
        <DownloadIcon class="size-4" />
        <span>Download</span>
      </ActionButton>
      <ActionButton :href="links.repo" variant="secondary" external>
        <CodeIcon class="size-4" />
        <span>View the source</span>
      </ActionButton>
    </div>

    <p class="font-mono text-center text-xs text-muted-foreground">Free and open source.</p>
  </div>
</template>
