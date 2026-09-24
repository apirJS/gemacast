import type { Metrics } from '../../core/types';
import { Status } from '../../core/types';
import { useAppStore } from '../../stores/app-store';

function bandColor(ms: number | null, warn: number, lost: number): string {
  if (ms === null) return 'text-muted-foreground/50';
  if (ms <= warn) return 'text-status-ok';
  if (ms <= lost) return 'text-status-warn';
  return 'text-status-lost';
}

type MetricProps = {
  label: string;
  ms: number | null;
  tone: string;
  placeholder?: string;
};

function Metric({ label, ms, tone, placeholder }: MetricProps) {
  const unavailable = ms === null && placeholder !== undefined;
  return (
    <div className="flex min-w-0 flex-col items-center gap-1 py-0.5">
      <span
        className={`flex items-baseline whitespace-nowrap text-[19px] font-semibold leading-none tabular-nums transition-colors duration-300 ${tone}`}
      >
        {unavailable ? (
          <span className="text-[13px] font-medium">{placeholder}</span>
        ) : (
          <>
            {ms === null ? '--' : ms}
            <span className="ml-px text-[10px] font-medium text-muted-foreground">ms</span>
          </>
        )}
      </span>
      <span className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
        {label}
      </span>
    </div>
  );
}

type ConnectionMetricsViewProps = {
  metrics: Metrics;
  visible: boolean;
  loopback: boolean;
  renderHelpButton?: (key: string) => React.ReactNode;
};

export function ConnectionMetricsView({
  metrics,
  visible,
  loopback,
  renderHelpButton,
}: ConnectionMetricsViewProps) {
  if (!visible) return null;
  return (
    <div
      className="mt-2.5 grid w-full grid-cols-[auto_1fr_auto_1fr_auto_1fr_auto] items-stretch animate-[fade-in_200ms_ease-out]"
      role="group"
      aria-label="Connection metrics"
    >
      <span aria-hidden="true" className={renderHelpButton ? 'w-6' : ''} />
      <Metric label="Buffer" ms={metrics.bufferMs} tone={bandColor(metrics.bufferMs, 30, 60)} />
      <span aria-hidden="true" className="readout-divider w-px self-stretch" />
      <Metric
        label="RTT"
        ms={metrics.networkRttMs}
        tone={bandColor(metrics.networkRttMs, 30, 80)}
        placeholder={loopback ? 'n/a' : undefined}
      />
      <span aria-hidden="true" className="readout-divider w-px self-stretch" />
      <Metric label="Jitter" ms={metrics.jitterMs} tone={bandColor(metrics.jitterMs, 10, 25)} />
      <span className="self-center">{renderHelpButton?.('connection-metrics')}</span>
    </div>
  );
}

type ConnectionMetricsProps = {
  renderHelpButton?: (key: string) => React.ReactNode;
};

export function ConnectionMetrics({ renderHelpButton }: ConnectionMetricsProps = {}) {
  const metrics = useAppStore((state) => state.metrics);
  const status = useAppStore((state) => state.status);
  const loopback = useAppStore((state) => state.networkLinkPair?.effective === 'adb');
  return (
    <ConnectionMetricsView
      metrics={metrics}
      visible={[Status.Connected, Status.Playing, Status.Paused].includes(status)}
      loopback={loopback}
      renderHelpButton={renderHelpButton}
    />
  );
}
