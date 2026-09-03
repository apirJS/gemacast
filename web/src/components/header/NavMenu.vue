<script setup lang="ts">
import NavToggleButton from "./NavToggleButton.vue";
import NavMenuList from "./NavMenuList.vue";
import { ref, watch } from "vue";
import { useBreakpoint } from "@/composables/useBreakpoint";

const isMenuOpen = ref(false);
const { isDesktop } = useBreakpoint();

watch(isDesktop, (nowDesktop) => {
  isMenuOpen.value = nowDesktop;
});
</script>

<template>
  <nav class="items-center flex flex-col relative">
    <NavToggleButton v-model="isMenuOpen" />
    <NavMenuList :hidden="!isMenuOpen" />
  </nav>
</template>
