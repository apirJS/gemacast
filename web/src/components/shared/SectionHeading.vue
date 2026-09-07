<script setup lang="ts">
defineProps<{
  eyebrow: string
  title: string
  subtitle?: string
}>()
</script>

<template>
  <div class="flex flex-col items-center gap-y-3 text-center">
    <span
      class="font-mono text-xs sm:text-sm font-bold uppercase tracking-[0.2em] text-muted-foreground border border-border bg-secondary px-3 py-1"
    >
      {{ eyebrow }}
    </span>

    <h2 class="font-[Black_Ops_One] text-primary text-2xl sm:text-3xl lg:text-4xl max-w-3xl">
      {{ title }}
    </h2>

    <p
      v-if="subtitle"
      class="font-mono text-sm sm:text-base text-secondary-foreground max-w-2xl leading-relaxed"
    >
      {{ subtitle }}
    </p>
  </div>
</template>
