<script setup lang="ts">
import GemacastIcon from './GemacastIcon.vue'
import NavMenu from './NavMenu.vue'
</script>

<template>
  <header
    class="z-50 bg-foreground min-h-[max(50px,10vh)] sticky flex justify-between pl-5 pr-7 py-1 items-center top-0"
  >
    <GemacastIcon />
    <NavMenu />
  </header>
</template>
