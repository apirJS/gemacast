<script setup lang="ts">
import NavToggleButton from './NavToggleButton.vue'
import NavMenuList from './NavMenuList.vue'
import { watch } from 'vue'
import { useBreakpoint } from '@/composables/useBreakpoint'
import { useMenu } from '@/composables/useMenu'

const { isDesktop } = useBreakpoint('(min-width: 768px)')
const { isMenuOpen, toggleMenu } = useMenu()

watch(isDesktop, (nowDesktop) => {
  isMenuOpen.value = nowDesktop
})
</script>

<template>
  <nav class="items-center justify-center flex flex-col relative">
    <NavToggleButton v-model:isMenuOpen="isMenuOpen" v-model:toggleMenu="toggleMenu" />
    <NavMenuList v-model="isMenuOpen" />
  </nav>
</template>
