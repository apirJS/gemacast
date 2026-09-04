<script setup lang="ts">
const isMenuOpen = defineModel<boolean>("isMenuOpen");
const toggleMenu = defineModel<() => void>("toggleMenu");
</script>

<template>
  <button
    class="cursor-pointer sm:hidden border-none w-5 h-5 relative"
    @click="toggleMenu"
    :class="{ 'is-open': isMenuOpen }"
  >
    <span class="bar bar-top top-1"></span>
    <span class="bar bar-middle top-2.5"></span>
    <span class="bar bar-bottom top-4"></span>
  </button>
</template>

<style lang="css" scoped>
.bar {
  position: absolute;
  left: 0;
  width: 100%;
  height: 2px;
  background: var(--color-secondary);
  border-radius: 1px;
  transition:
    transform 0.15s ease,
    opacity 0.15s ease,
    top 0.15s ease;
}

.is-open .bar-top {
  top: 8px;
  transform: rotate(45deg);
}

.is-open .bar-middle {
  opacity: 0;
}

.is-open .bar-bottom {
  top: 8px;
  transform: rotate(-45deg);
}
</style>
