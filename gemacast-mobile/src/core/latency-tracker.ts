import type { LatencyStats } from './types';

const LATENCY_WINDOW = 50;

export class LatencyTracker {
  private samples: number[] = [];

  update(currentMs: number): LatencyStats {
    this.samples.push(currentMs);
    if (this.samples.length > LATENCY_WINDOW) {
      this.samples.shift();
    }

    let sum = 0;
    for (const s of this.samples) {
      sum += s;
    }
    const avg = sum / this.samples.length;
    const max = Math.max(...this.samples);
    const min = Math.min(...this.samples);

    return {
      current: Math.round(currentMs * 10) / 10,
      avg: Math.round(avg * 10) / 10,
      max: Math.round(max * 10) / 10,
      min: Math.round(min * 10) / 10,
    };
  }

  reset() {
    this.samples = [];
  }
}
