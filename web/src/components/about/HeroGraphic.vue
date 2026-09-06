<script setup lang="ts">
import { MonitorIcon, SmartphoneIcon, UsbIcon, WifiIcon } from '@lucide/vue'
</script>

<template>
  <div class="flex flex-row gap-x-3 items-center sm:gap-x-8">
    <div class="relative flex flex-col items-center">
      <MonitorIcon class="text-primary size-20 bg-background z-2 sm:size-24" />

      <div
        class="font-mono absolute top-[12.5%] bottom-[29.2%] left-[8.3%] right-[8.3%] flex flex-col items-center justify-center text-foreground text-[0.5rem] leading-tight font-semibold pointer-events-none z-3"
      >
        <span>Capture: 12ms</span>
        <span>Buffer: 20ms</span>
      </div>

      <span class="absolute animate-bip-pc">*bip*</span>
    </div>
    <div class="flex flex-col">
      <div class="flex flex-row justify-center gap-x-4">
        <WifiIcon class="text-primary size-5 sm:size-6" />
        <UsbIcon class="text-primary size-5 sm:size-6" />
      </div>
      <svg
        class="text-primary w-20 sm:w-30 sm:scale-125"
        xmlns="http://www.w3.org/2000/svg"
        width="120"
        height="24"
        viewBox="-60 -12 120 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
      >
        <line x1="-40" y1="0" x2="40" y2="0" />
        <polyline points="33 -7 40 0 33 7" />
      </svg>
      <div class="flex justify-center">
        <span class="font-mono font-bold text-primary text-sm sm:text-base">ADB</span>
      </div>
    </div>
    <div class="relative flex flex-col items-center pt-6">
      <SmartphoneIcon class="text-primary size-14 bg-background z-2 sm:size-18" />
      <span class="absolute animate-bip-phone">*bip*</span>
    </div>
  </div>
</template>

<style lang="css" scoped>
.animate-bip-pc {
  animation: bip 1000ms ease-out 0s infinite normal forwards;
}
.animate-bip-phone {
  animation: bip 1000ms ease-out 32ms infinite normal forwards;
}

@keyframes bip {
  from {
    top: 20%;
    opacity: 0;
  }

  to {
    top: 100%;
    opacity: 1;
  }
}
</style>
