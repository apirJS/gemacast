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
    config: { minDepthMs: 25, comfortCapMs: 1000, peakDecayHalflifeMs: 0, resumeThresholdPct: 0.25 },
  },
  {
    id: 'wired',
    name: 'Wired',
    description: 'USB, ADB, or near-perfect connections. Fixed 10ms buffer.',
    config: {
      minDepthMs: 0,
      comfortCapMs: 10,
      peakDecayHalflifeMs: 1000,
      resumeThresholdPct: 0.5,
      staticTargetMs: 10,
    },
  },
  {
    id: 'fast',
    name: 'Fast',
    description: 'Good 5 GHz Wi-Fi. Fixed 30ms buffer — very low latency.',
    config: {
      minDepthMs: 10,
      comfortCapMs: 30,
      peakDecayHalflifeMs: 1000,
      resumeThresholdPct: 0.5,
      staticTargetMs: 30,
    },
  },
  {
    id: 'balanced',
    name: 'Balanced',
    description: 'Most Wi-Fi networks. Fixed 60ms buffer — reliable with low latency.',
    config: {
      minDepthMs: 20,
      comfortCapMs: 60,
      peakDecayHalflifeMs: 1000,
      resumeThresholdPct: 0.5,
      staticTargetMs: 60,
    },
  },
  {
    id: 'stable',
    name: 'Stable',
    description: 'Congested or 2.4 GHz Wi-Fi. Fixed 120ms buffer — extra headroom.',
    config: {
      minDepthMs: 40,
      comfortCapMs: 120,
      peakDecayHalflifeMs: 1000,
      resumeThresholdPct: 0.5,
      staticTargetMs: 120,
    },
  },
  {
    id: 'resilient',
    name: 'Resilient',
    description: 'Bad Wi-Fi or screen-off streaming. Fixed 200ms buffer — maximum stability.',
    config: {
      minDepthMs: 60,
      comfortCapMs: 200,
      peakDecayHalflifeMs: 1000,
      resumeThresholdPct: 0.5,
      staticTargetMs: 200,
    },
  },
  {
    id: 'custom',
    name: 'Custom',
    description: 'Define your own buffer parameters manually.',
    config: null,
  },
  {
    id: 'nobuffer',
    name: 'No Buffer',
    description: 'Play audio instantly as it arrives. Zero buffering, zero safety net.',
    config: {
      minDepthMs: 0,
      comfortCapMs: 0,
      peakDecayHalflifeMs: 1000,
      resumeThresholdPct: 0,
      staticTargetMs: 0,
    },
  },
];

export function getPresetConfig(id: string, customConfig: JitterConfig): JitterConfig {
  if (id === 'custom' || id.startsWith('saved-')) return customConfig;
  const def = JITTER_PRESETS.find((p) => p.id === id);
  return def?.config ?? customConfig;
}
