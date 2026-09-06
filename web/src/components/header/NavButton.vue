<script setup lang="ts">
withDefaults(
  defineProps<{
    href?: string
    external?: boolean
  }>(),
  { href: undefined, external: false },
)
</script>

<template>
  <component
    :is="href ? 'a' : 'button'"
    :href="href"
    :target="href && external ? '_blank' : undefined"
    :rel="href && external ? 'noreferrer' : undefined"
    class="active:text-primary/60 sm:hover:text-primary/60 sm:active:scale-95 font-semibold text-primary justify-start md:justify-center bg-primary-foreground flex items-center gap-x-1 py-1 px-2 rounded-sm cursor-pointer whitespace-nowrap"
  >
    <slot></slot>
  </component>
</template>
