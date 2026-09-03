<script setup lang="ts">
import { MenuIcon, XIcon } from "@lucide/vue";

const isMenuOpened = defineModel<boolean>();

function toggleMenu() {
  isMenuOpened.value = !isMenuOpened.value;
}
</script>

<template>
  <button class="cursor-pointer p-2 sm:hidden" @click="toggleMenu">
    <XIcon v-if="isMenuOpened" class="size-5" />
    <MenuIcon v-else class="size-4" />
  </button>
</template>
