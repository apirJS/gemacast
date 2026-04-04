import { AppState, Status } from '../types';
import { fmt } from './utils';
import { App } from '../App';

export function setupLatencyStats(app: App) {
  const latencyStatsEl = document.getElementById(
    'latency-stats',
  ) as HTMLDivElement | null;
  const latencyCurrentEl = document.querySelector<HTMLSpanElement>(
    '#latency-current .latency-stats__value',
  );
  const latencyAvgEl = document.querySelector<HTMLSpanElement>(
    '#latency-avg .latency-stats__value',
  );
  const latencyMaxEl = document.querySelector<HTMLSpanElement>(
    '#latency-max .latency-stats__value',
  );
  const latencyMinEl = document.querySelector<HTMLSpanElement>(
    '#latency-min .latency-stats__value',
  );

  app.stateHandler.subscribe((state: AppState) => {
    const showLatency =
      state.status === Status.Playing || state.status === Status.Connected;

    if (latencyStatsEl) {
      latencyStatsEl.hidden = !showLatency;
    }

    if (showLatency) {
      if (latencyCurrentEl && latencyAvgEl && latencyMaxEl && latencyMinEl) {
        latencyCurrentEl.textContent = fmt(state.latency.current);
        latencyAvgEl.textContent = fmt(state.latency.avg);
        latencyMaxEl.textContent = fmt(state.latency.max);
        latencyMinEl.textContent = fmt(state.latency.min);
      }
    }
  });
}
