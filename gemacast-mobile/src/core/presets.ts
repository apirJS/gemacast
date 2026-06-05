import type { JitterConfig, PresetId } from './types';

export type PresetDefinition = {
  id: PresetId;
  name: string;
  description: string;
  config: JitterConfig | null;
};

export const JITTER_PRESETS: PresetDefinition[] = [
  {
    id: 'auto',
    name: 'Auto',
    description: 'Automatically discovers the lowest stable latency for your connection.',
    config: { minDepthMs: 5, comfortCapMs: 1000, peakDecayHalflifeMs: 0, resumeThresholdPct: 0.25 },
  },
  {
    id: 'wired',
    name: 'Wired',
    description: 'For USB, ADB, or flawless wired connections. Minimum latency, no safety net.',
    config: { minDepthMs: 2, comfortCapMs: 20, peakDecayHalflifeMs: 500, resumeThresholdPct: 0.20 },
  },
  {
    id: 'fast',
    name: 'Fast',
    description: 'Good 5 GHz Wi-Fi. Very low latency with light buffering for minor hiccups.',
    config: { minDepthMs: 10, comfortCapMs: 80, peakDecayHalflifeMs: 1000, resumeThresholdPct: 0.30 },
  },
  {
    id: 'balanced',
    name: 'Balanced',
    description: 'The default sweet spot. Works well on most networks with low latency.',
    config: { minDepthMs: 25, comfortCapMs: 150, peakDecayHalflifeMs: 3500, resumeThresholdPct: 0.40 },
  },
  {
    id: 'stable',
    name: 'Stable',
    description: 'Congested or 2.4 GHz Wi-Fi. More buffer headroom, handles jitter well.',
    config: { minDepthMs: 50, comfortCapMs: 500, peakDecayHalflifeMs: 34600, resumeThresholdPct: 0.50 },
  },
  {
    id: 'resilient',
    name: 'Resilient',
    description: 'Bad Wi-Fi or screen-off streaming. Maximum stability, higher latency.',
    config: { minDepthMs: 80, comfortCapMs: 1000, peakDecayHalflifeMs: 34600, resumeThresholdPct: 0.70 },
  },
  {
    id: 'custom',
    name: 'Custom',
    description: 'Define your own buffer parameters manually.',
    config: null,
  },
];

export function getPresetConfig(id: string, customConfig: JitterConfig): JitterConfig {
  if (id === 'custom' || id.startsWith('saved-')) return customConfig;
  const def = JITTER_PRESETS.find((p) => p.id === id);
  return def?.config ?? customConfig;
}
