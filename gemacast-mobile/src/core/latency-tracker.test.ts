import { describe, it, expect } from 'bun:test';
import { LatencyTracker } from './latency-tracker';

describe('LatencyTracker', () => {
  it('returns stats for a single sample', () => {
    const tracker = new LatencyTracker();
    const stats = tracker.update(50.123);
    expect(stats.current).toBe(50.1);
    expect(stats.avg).toBe(50.1);
    expect(stats.max).toBe(50.1);
    expect(stats.min).toBe(50.1);
  });

  it('computes rolling average', () => {
    const tracker = new LatencyTracker();
    tracker.update(40);
    tracker.update(60);
    const stats = tracker.update(80);
    expect(stats.avg).toBe(60);
    expect(stats.min).toBe(40);
    expect(stats.max).toBe(80);
  });

  it('caps window at 50 samples', () => {
    const tracker = new LatencyTracker();
    for (let i = 0; i < 55; i++) {
      tracker.update(100);
    }
    const stats = tracker.update(200);
    expect(stats.current).toBe(200);
    expect(stats.max).toBe(200);
  });

  it('resets samples', () => {
    const tracker = new LatencyTracker();
    tracker.update(100);
    tracker.update(200);
    tracker.reset();
    const stats = tracker.update(50);
    expect(stats.current).toBe(50);
    expect(stats.avg).toBe(50);
    expect(stats.max).toBe(50);
    expect(stats.min).toBe(50);
  });
});
