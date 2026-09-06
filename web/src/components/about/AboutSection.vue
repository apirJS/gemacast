<script setup lang="ts">
import HeroActions from './HeroActions.vue'
import HeroGraphic from './HeroGraphic.vue'
import HeroHeadline from './HeroHeadline.vue'
</script>

<template>
  <section id="about" class="flex flex-col pb-16 items-center justify-center gap-y-10 select-none">
    <HeroHeadline />
    <HeroGraphic />
    <HeroActions />
  </section>
</template>
