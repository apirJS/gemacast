<script setup lang="ts">
defineProps<{
  src: string
  alt: string
  label: string
  caption: string
  width: number
  height: number
  tall?: boolean
}>()
</script>

<template>
  <figure class="offset-shadow flex flex-col border border-primary bg-card">
    <figcaption
      class="font-mono flex items-center gap-x-2 border-b border-primary bg-secondary px-3 py-2"
    >
      <span class="size-2 shrink-0 border border-primary bg-accent-green"></span>
      <span class="text-xs font-bold uppercase tracking-wider text-secondary-foreground">
        {{ label }}
      </span>
    </figcaption>

    <div class="flex grow items-center justify-center bg-muted p-3 sm:p-4">
      <img
        :src="src"
        :alt="alt"
        :width="width"
        :height="height"
        loading="lazy"
        decoding="async"
        class="w-auto max-w-full border border-border object-contain"
        :class="tall ? 'max-h-[78vh] sm:max-h-[60vh]' : 'max-h-56 sm:max-h-64'"
      />
    </div>

    <p
      class="font-mono border-t border-border px-3 py-3 text-xs leading-relaxed text-secondary-foreground sm:text-sm"
    >
      {{ caption }}
    </p>
  </figure>
</template>
