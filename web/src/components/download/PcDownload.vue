<script setup lang="ts">
import type { Component } from 'vue'
import { MonitorIcon } from '@lucide/vue'
import { links } from '@/core/links'
import ActionButton from '@/components/shared/ActionButton.vue'
import AppleLogo from '@/components/shared/AppleLogo.vue'
import LinuxLogo from '@/components/shared/LinuxLogo.vue'
import WindowsLogo from '@/components/shared/WindowsLogo.vue'

interface Platform {
  icon: Component
  name: string
  requirement: string
  primaryLabel: string
  primaryHref: string
  secondaryLabel: string
  secondaryHref: string
}

const platforms: Platform[] = [
  {
    icon: WindowsLogo,
    name: 'Windows',
    requirement: '10 or newer, captures with WASAPI',
    primaryLabel: 'Installer (.msi)',
    primaryHref: links.windowsInstaller,
    secondaryLabel: 'Portable (.zip)',
    secondaryHref: links.windowsPortable,
  },
  {
    icon: LinuxLogo,
    name: 'Linux',
    requirement: 'Needs a running PipeWire session',
    primaryLabel: 'Archive (.tar.xz)',
    primaryHref: links.linuxArchive,
    secondaryLabel: 'deb / rpm / AppImage',
    secondaryHref: links.releases,
  },
  {
    icon: AppleLogo,
    name: 'macOS',
    requirement: '13 or newer for ScreenCaptureKit',
    primaryLabel: 'Universal (.dmg)',
    primaryHref: links.macosDmg,
    secondaryLabel: 'Other builds',
    secondaryHref: links.releases,
  },
]
</script>

<template>
  <article class="offset-shadow flex flex-col border border-primary bg-card">
    <header
      class="flex items-center gap-x-3 border-b border-primary bg-secondary px-4 py-3 sm:px-5 sm:py-4"
    >
      <span class="border border-primary bg-card p-2 text-primary">
        <MonitorIcon class="size-5" />
      </span>
      <div class="flex flex-col">
        <h3 class="font-[Black_Ops_One] text-lg text-primary sm:text-xl">The streamer</h3>
        <p class="font-mono text-xs text-muted-foreground">Runs in your desktop's tray</p>
      </div>
    </header>

    <ul class="flex grow flex-col divide-y divide-border">
      <li
        v-for="platform in platforms"
        :key="platform.name"
        class="flex grow flex-col gap-3 p-4 sm:p-5"
      >
        <div class="flex items-center gap-x-3">
          <component :is="platform.icon" class="size-5 shrink-0 text-primary" />
          <div class="flex min-w-0 flex-col">
            <h4 class="font-mono text-sm font-bold text-primary">{{ platform.name }}</h4>
            <p class="font-mono text-xs text-muted-foreground">{{ platform.requirement }}</p>
          </div>
        </div>

        <div class="grid grid-cols-1 gap-2 sm:grid-cols-2">
          <ActionButton :href="platform.primaryHref" external>
            <span>{{ platform.primaryLabel }}</span>
          </ActionButton>
          <ActionButton :href="platform.secondaryHref" variant="secondary" external>
            <span>{{ platform.secondaryLabel }}</span>
          </ActionButton>
        </div>
      </li>
    </ul>
  </article>
</template>
