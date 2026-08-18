import { useAppStore } from '../../stores/app-store';
import { Status } from '../../core/types';

/**
 * Health bands per metric, mapped onto the existing `--color-status-*` tokens.
 *
 * Each metric gets its own thresholds because they are three distinct signals
 * (see the `Metrics` doc comment), not three views of one — a 60 ms buffer is
 * ordinary, a 60 ms jitter estimate is not. `null` returns the muted tone so an
 * unmeasured metric never reads as a fault.
 */
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
  /** Shown in place of a value when the metric does not apply on this transport. */
  placeholder?: string;
};

/**
 * One column of the readout: value on top, caption below.
 *
 * The size pairing is the point — the value carries the emphasis and the caption
 * recedes, so the block scans as data rather than as a form. The unit is a
 * separate smaller muted span tucked against the digits, so three repetitions of
 * "ms" never compete with the numbers they follow.
 */
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

export function ConnectionMetrics() {
  const metrics = useAppStore((s) => s.metrics);
  const status = useAppStore((s) => s.status);
  const linkPair = useAppStore((s) => s.networkLinkPair);

  const visible =
    status === Status.Connected || status === Status.Playing || status === Status.Paused;
  if (!visible) return null;

  // RTT is measured by the control-channel probe loop, which does not run over
  // ADB/loopback — so a null there means "not applicable", not "not yet known".
  const isLoopback = linkPair?.effective === 'adb';

  return (
    <div
      className="mt-2.5 grid w-full grid-cols-[1fr_auto_1fr_auto_1fr] items-stretch animate-[fade-in_200ms_ease-out]"
      role="group"
      aria-label="Connection metrics"
    >
      <Metric label="Buffer" ms={metrics.bufferMs} tone={bandColor(metrics.bufferMs, 30, 60)} />
      <span aria-hidden="true" className="readout-divider w-px self-stretch" />
      <Metric
        label="RTT"
        ms={metrics.networkRttMs}
        tone={bandColor(metrics.networkRttMs, 30, 80)}
        placeholder={isLoopback ? 'n/a' : undefined}
      />
      <span aria-hidden="true" className="readout-divider w-px self-stretch" />
      <Metric label="Jitter" ms={metrics.jitterMs} tone={bandColor(metrics.jitterMs, 10, 25)} />
    </div>
  );
}
