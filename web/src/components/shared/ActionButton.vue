<script setup lang="ts">
withDefaults(
  defineProps<{
    href: string
    variant?: 'primary' | 'secondary'
    external?: boolean
  }>(),
  { variant: 'primary', external: false },
)
</script>

<template>
  <a
    :href="href"
    :target="external ? '_blank' : undefined"
    :rel="external ? 'noreferrer' : undefined"
    class="offset-shadow-sm hover:offset-shadow-none font-mono flex items-center justify-center gap-x-2 border border-primary px-4 py-2.5 text-sm font-bold transition-all hover:translate-x-0.5 hover:translate-y-0.5 active:translate-x-0.5 active:translate-y-0.5"
    :class="
      variant === 'primary'
        ? 'bg-primary text-primary-foreground'
        : 'bg-secondary text-secondary-foreground'
    "
  >
    <slot></slot>
  </a>
</template>
