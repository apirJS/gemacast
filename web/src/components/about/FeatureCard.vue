<script setup lang="ts">
import type { Component } from 'vue'

defineProps<{
  icon: Component
  title: string
  body: string
  tag: string
}>()
</script>

<template>
  <li
    class="offset-shadow hover:offset-shadow-sm flex flex-col gap-y-3 border border-primary bg-card p-5 sm:p-6 transition-all hover:translate-x-0.5 hover:translate-y-0.5"
  >
    <div class="flex items-center gap-x-3">
      <span class="border border-primary bg-secondary p-2 text-primary">
        <component :is="icon" class="size-5 sm:size-6" />
      </span>
      <h3 class="font-[Black_Ops_One] text-primary text-base sm:text-lg leading-tight">
        {{ title }}
      </h3>
    </div>

    <p class="font-mono text-sm text-secondary-foreground leading-relaxed grow">
      {{ body }}
    </p>

    <span
      class="font-mono text-[0.65rem] sm:text-xs font-bold uppercase tracking-wider text-muted-foreground border-t border-border pt-3"
    >
      {{ tag }}
    </span>
  </li>
</template>
