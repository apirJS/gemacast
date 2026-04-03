import { StateHandler } from './StateHandler';

const LATENCY_WINDOW = 50;

export class LatencyTracker {
  private latencySamples: number[] = [];

  constructor(private stateHandler: StateHandler) {}

  public updateLatency(currentMs: number) {
    this.latencySamples.push(currentMs);
    if (this.latencySamples.length > LATENCY_WINDOW) {
      this.latencySamples.shift();
    }

    const sum = this.latencySamples.reduce((a, b) => a + b, 0);
    const avg = sum / this.latencySamples.length;
    const max = Math.max(...this.latencySamples);
    const min = Math.min(...this.latencySamples);

    this.stateHandler.updateLatencyInfo(
      Math.round(currentMs * 10) / 10,
      Math.round(avg * 10) / 10,
      Math.round(max * 10) / 10,
      Math.round(min * 10) / 10,
    );
  }
}
