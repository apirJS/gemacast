<script setup lang="ts">
import SectionHeading from '@/components/shared/SectionHeading.vue'
import AndroidDownload from './AndroidDownload.vue'
import PcDownload from './PcDownload.vue'
</script>

<template>
  <section
    id="download"
    class="flex flex-col items-center justify-center gap-y-10 px-4 py-20 sm:px-6 sm:py-24 lg:px-10"
  >
    <SectionHeading
      eyebrow="Download"
      title="Two apps, no virtual cable needed"
      subtitle="The streamer on your desktop, the player on your phone."
    />

    <div class="grid w-full max-w-5xl grid-cols-1 items-start gap-4 sm:gap-5 md:grid-cols-2">
      <PcDownload />
      <AndroidDownload />
    </div>
  </section>
</template>
