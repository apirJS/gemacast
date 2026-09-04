<script setup lang="ts">
import { CodeIcon, DownloadIcon } from "@lucide/vue";
import NavButton from "./NavButton.vue";

const isMenuOpen = defineModel<boolean>();
</script>

<template>
  <ul
    class="transition-opacity flex flex-col gap-x-4 gap-y-1 absolute top-0 right-0 mt-6 border-primary/30 border
    sm:mt-0 sm:static sm:flex-row sm:border-0 sm:bg-foreground bg-transparent"
    :class="{
      'opacity-0': !isMenuOpen,
      'opacity-100': isMenuOpen,
    }"
  >
     <!-- <NavButton>
      <span>Demo</span>
    </NavButton>  -->

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
