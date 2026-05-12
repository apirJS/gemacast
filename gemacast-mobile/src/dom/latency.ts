import { AppState, Status } from '../types';
import { fmt, h } from './utils';
import { App } from '../App';

export function setupLatencyStats(app: App) {
  const latencyStatsEl = document.getElementById('latency-stats');
  if (!latencyStatsEl) return;

  latencyStatsEl.innerHTML = '';

  const currentVal = h('span', { className: 'latency-stats__value', textContent: '-- ms' });
  const avgVal = h('span', { className: 'latency-stats__value', textContent: '-- ms' });
  const maxVal = h('span', { className: 'latency-stats__value', textContent: '-- ms' });
  const minVal = h('span', { className: 'latency-stats__value', textContent: '-- ms' });

  const createItem = (id: string, title: string, label: string, valueEl: HTMLElement) => {
    return h('span', { className: 'latency-stats__item', id, title },
      h('span', { className: 'latency-stats__label', textContent: label }),
      valueEl
    );
  };

  const sep = () => h('span', { className: 'latency-stats__sep', ariaHidden: 'true', textContent: '·' });

  latencyStatsEl.appendChild(createItem('latency-current', 'Current buffer time', 'Now', currentVal));
  latencyStatsEl.appendChild(sep());
  latencyStatsEl.appendChild(createItem('latency-avg', 'Rolling average latency', 'Avg', avgVal));
  latencyStatsEl.appendChild(sep());
  latencyStatsEl.appendChild(createItem('latency-max', 'Peak latency', 'Max', maxVal));
  latencyStatsEl.appendChild(sep());
  latencyStatsEl.appendChild(createItem('latency-min', 'Minimum latency', 'Min', minVal));

  app.stateHandler.subscribe((state: AppState) => {
    const showLatency = state.status === Status.Playing || state.status === Status.Connected;
    latencyStatsEl.hidden = !showLatency;

    if (showLatency) {
      currentVal.textContent = fmt(state.latency.current);
      avgVal.textContent = fmt(state.latency.avg);
      maxVal.textContent = fmt(state.latency.max);
      minVal.textContent = fmt(state.latency.min);
    }
  });
}
