<script setup lang="ts">
import { CodeIcon, DownloadIcon } from "@lucide/vue";
import NavButton from "./NavButton.vue";
</script>

<template>
  <ul
    class="flex flex-col border border-black gap-4 bg-white absolute top-0 right-0 mt-8 sm:mt-0 sm:static sm:flex-row sm:border-0"
  >
    <NavButton>
      <span>Demo</span>
    </NavButton>

    <NavButton>
      <span>Download</span>
      <DownloadIcon class="size-4" />
    </NavButton>

    <NavButton>
      <span>Source</span>
      <CodeIcon class="size-4 mt-0.75" />
    </NavButton>
  </ul>
</template>
