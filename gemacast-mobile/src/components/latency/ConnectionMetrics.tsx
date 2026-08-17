import { useAppStore } from '../../stores/app-store';
import { Status } from '../../core/types';

function getBufferColor(ms: number | null): string {
  if (ms === null) return 'bg-muted-foreground/40';
  if (ms <= 30) return 'bg-status-ok';
  if (ms <= 60) return 'bg-status-warn';
  return 'bg-status-lost';
}

function StatItem({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex flex-col items-center">
      <span className="text-[10px] uppercase tracking-wider text-muted-foreground mb-0.5">
        {label}
      </span>
      <span className="text-xs font-medium tabular-nums whitespace-nowrap text-muted-foreground">
        {value}
      </span>
    </div>
  );
}

export function ConnectionMetrics() {
  const metrics = useAppStore((s) => s.metrics);
  const status = useAppStore((s) => s.status);

  const visible =
    status === Status.Connected || status === Status.Playing || status === Status.Paused;
  if (!visible) return null;

  const format = (v: number | null) => (v !== null ? `${v} ms` : '-- ms');

  return (
    <div className="flex items-center justify-center gap-5 text-xs animate-[fade-in_200ms_ease-out] w-full px-4">
      <div className="flex flex-col items-center">
        <span className="text-[10px] uppercase tracking-wider text-muted-foreground mb-0.5">
          Buffer
        </span>
        <span className="inline-flex items-center gap-1.5 text-xs font-medium tabular-nums text-foreground whitespace-nowrap">
          <span
            className={`inline-flex h-1.5 w-1.5 rounded-full ${getBufferColor(metrics.bufferMs)} transition-colors duration-300`}
          />
          {format(metrics.bufferMs)}
        </span>
      </div>
      <StatItem label="Network (RTT)" value={format(metrics.networkRttMs)} />
      <StatItem label="Jitter" value={format(metrics.jitterMs)} />
    </div>
  );
}
