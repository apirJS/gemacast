<script setup lang="ts">
import { CodeIcon, DownloadIcon } from '@lucide/vue'
import { useBreakpoint } from '@/composables/useBreakpoint'
import { links } from '@/core/links'
import NavButton from './NavButton.vue'

const isMenuOpen = defineModel<boolean>()

const { isDesktop } = useBreakpoint('(min-width: 768px)')

const closeOnMobile = () => {
  if (!isDesktop.value) {
    isMenuOpen.value = false
  }
}
</script>

<template>
  <ul
    class="transition-opacity flex flex-col gap-x-4 gap-y-1 absolute top-0 right-0 mt-6 min-w-44 p-2 bg-foreground border-primary/30 border md:mt-0 md:static md:min-w-0 md:flex-row md:border-0 md:p-0"
    :class="{
      'opacity-0 invisible': !isMenuOpen,
      'opacity-100 visible': isMenuOpen,
    }"
  >
    <li>
      <NavButton href="#features" class="w-full" @click="closeOnMobile">
        <span>Features</span>
      </NavButton>
    </li>

    <li>
      <NavButton href="#demo" class="w-full" @click="closeOnMobile">
        <span>Demo</span>
      </NavButton>
    </li>

    <li>
      <NavButton href="#download" class="w-full" @click="closeOnMobile">
        <span>Download</span>
        <DownloadIcon class="size-4" />
      </NavButton>
    </li>

    <li>
      <NavButton :href="links.repo" external class="w-full bg-secondary" @click="closeOnMobile">
        <span>Source</span>
        <CodeIcon class="size-4 mt-0.75" />
      </NavButton>
    </li>
  </ul>
</template>
