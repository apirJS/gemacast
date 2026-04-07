import { describe, it, expect, beforeEach } from 'bun:test';
import { setupBrowserGlobals, makeDeviceInfo } from './testHelpers';
import { StateHandler } from './StateHandler';
import { LatencyTracker } from './LatencyTracker';

function setup() {
  const sh = new StateHandler(makeDeviceInfo());
  const tracker = new LatencyTracker(sh);
  return { sh, tracker };
}

beforeEach(() => {
  setupBrowserGlobals();
});

describe('LatencyTracker — updateLatency', () => {
  it('sets current to the provided sample (rounded to 1 dp)', () => {
    const { sh, tracker } = setup();
    tracker.updateLatency(42.567);
    expect(sh.getState().latency.current).toBe(42.6);
  });

  it('computes avg, max, min from accumulated samples', () => {
    const { sh, tracker } = setup();
    tracker.updateLatency(10);
    tracker.updateLatency(20);
    tracker.updateLatency(30);
    const { avg, max, min } = sh.getState().latency;
    expect(avg).toBe(20);
    expect(max).toBe(30);
    expect(min).toBe(10);
  });

  it('rolling window caps at 50 samples', () => {
    const { sh, tracker } = setup();
    for (let i = 1; i <= 51; i++) tracker.updateLatency(i);
    expect(sh.getState().latency.min).toBe(2);
  });

  it('rounds values to one decimal place', () => {
    const { sh, tracker } = setup();
    tracker.updateLatency(10.123);
    tracker.updateLatency(20.456);
    const { avg } = sh.getState().latency;
    expect(avg).toBe(15.3);
  });

  it('keeps state updated after every call', () => {
    const { sh, tracker } = setup();
    tracker.updateLatency(5);
    expect(sh.getState().latency.current).toBe(5);
    tracker.updateLatency(15);
    expect(sh.getState().latency.current).toBe(15);
  });

  it('correctly handles a single sample (avg === current)', () => {
    const { sh, tracker } = setup();
    tracker.updateLatency(100);
    const { current, avg, max, min } = sh.getState().latency;
    expect(current).toBe(100);
    expect(avg).toBe(100);
    expect(max).toBe(100);
    expect(min).toBe(100);
  });
});
