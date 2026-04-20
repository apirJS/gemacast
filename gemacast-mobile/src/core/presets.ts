import { JitterConfig } from '../types';

export const JITTER_PRESETS: string[] = [
  'Ultra Low Latency',  // 0 — USB / perfect 5 GHz
  'Very Low Latency',   // 1 — great 5 GHz
  'Low Latency',        // 2 — good 5 GHz
  'Responsive',         // 3 — average 5 GHz / great 2.4 GHz
  'Balanced',           // 4 — default sweet spot
  'Stable',             // 5 — safe for most networks
  'Very Stable',        // 6 — congested 2.4 GHz
  'High Resilience',    // 7 — bad 2.4 GHz with interference
  'Very High Resilience', // 8 — very bad network
  'Maximum Resilience', // 9 — worst case, max safety
  'Custom',             // 10
];

export const PRESET_DESCRIPTIONS: string[] = [
  'For wired USB or flawless 5GHz connections with zero interference.',
  'Excellent 5GHz networks. Prioritizes ultra-low latency over stability.',
  'Good 5GHz networks. A fast and responsive experience.',
  'Average 5GHz or pristine 2.4GHz networks. Feels very snappy.',
  'The default sweet spot. Balances responsiveness with stability.',
  'Safe for most networks. Adds a bit more buffer to handle hiccups.',
  'Congested 2.4GHz networks. Noticeable latency, zero stuttering.',
  'Bad 2.4GHz networks with significant interference.',
  'Very bad networks with frequent stalls and packet drops.',
  'Worst-case scenario. Maximum safety, highest base latency.',
  'Define your own buffer behaviors and parameters manually.',
];


/**
 * Hand-tuned preset configurations. Each row is designed for a specific
 * network quality tier with meaningful gaps between adjacent presets.
 *
 * Key principles:
 *   - minDepthMs: the floor the buffer never drops below.
 *     Low values risk starvation on jittery networks.
 *   - comfortCapMs: the ceiling the buffer never grows beyond.
 *     Prevents runaway latency after network stalls.
 *   - bounceMultiplier: how aggressively the target grows on starvation.
 *     Higher = safer but adds more delay after stalls.
 *   - resumeThresholdPct: how full the buffer must be before unmuting.
 *     Higher = less chance of re-stalling but slower recovery.
 *   - wsolaMaxSkip: frames the shedder can skip per callback.
 *     Higher = catches up faster but may cause audible artifacts.
 */
const PRESET_CONFIGS: JitterConfig[] = [
  // 0 — Ultra Low Latency
  { minDepthMs: 2,   comfortCapMs: 50,   bounceMultiplier: 1.1,  resumeThresholdPct: 0.20, wsolaMaxSkip: 4, initialComfortMs: 2,   fastSettleMultiplier: 6.0, fastSettleFrames: 200 },
  // 1 — Very Low Latency
  { minDepthMs: 10,  comfortCapMs: 80,   bounceMultiplier: 1.15, resumeThresholdPct: 0.25, wsolaMaxSkip: 4, initialComfortMs: 10,  fastSettleMultiplier: 5.0, fastSettleFrames: 200 },
  // 2 — Low Latency
  { minDepthMs: 15,  comfortCapMs: 100,  bounceMultiplier: 1.2,  resumeThresholdPct: 0.30, wsolaMaxSkip: 3, initialComfortMs: 15,  fastSettleMultiplier: 4.0, fastSettleFrames: 200 },
  // 3 — Responsive
  { minDepthMs: 20,  comfortCapMs: 140,  bounceMultiplier: 1.3,  resumeThresholdPct: 0.35, wsolaMaxSkip: 3, initialComfortMs: 25,  fastSettleMultiplier: 3.5, fastSettleFrames: 200 },
  // 4 — Balanced
  { minDepthMs: 30,  comfortCapMs: 200,  bounceMultiplier: 1.4,  resumeThresholdPct: 0.40, wsolaMaxSkip: 3, initialComfortMs: 35,  fastSettleMultiplier: 3.0, fastSettleFrames: 200 },
  // 5 — Stable (default)
  { minDepthMs: 40,  comfortCapMs: 280,  bounceMultiplier: 1.5,  resumeThresholdPct: 0.50, wsolaMaxSkip: 2, initialComfortMs: 50,  fastSettleMultiplier: 2.5, fastSettleFrames: 200 },
  // 6 — Very Stable
  { minDepthMs: 60,  comfortCapMs: 400,  bounceMultiplier: 1.7,  resumeThresholdPct: 0.60, wsolaMaxSkip: 2, initialComfortMs: 70,  fastSettleMultiplier: 2.0, fastSettleFrames: 200 },
  // 7 — High Resilience
  { minDepthMs: 80,  comfortCapMs: 550,  bounceMultiplier: 2.0,  resumeThresholdPct: 0.70, wsolaMaxSkip: 2, initialComfortMs: 90,  fastSettleMultiplier: 1.8, fastSettleFrames: 200 },
  // 8 — Very High Resilience
  { minDepthMs: 100, comfortCapMs: 750,  bounceMultiplier: 2.2,  resumeThresholdPct: 0.80, wsolaMaxSkip: 1, initialComfortMs: 120, fastSettleMultiplier: 1.5, fastSettleFrames: 200 },
  // 9 — Maximum Resilience
  { minDepthMs: 140, comfortCapMs: 1000, bounceMultiplier: 2.5,  resumeThresholdPct: 1.00, wsolaMaxSkip: 1, initialComfortMs: 160, fastSettleMultiplier: 1.2, fastSettleFrames: 200 },
];

export function getPresetConfig(index: number, customConfig: JitterConfig): JitterConfig {
  if (index === 10) return customConfig;
  return PRESET_CONFIGS[Math.max(0, Math.min(index, 9))];
}
