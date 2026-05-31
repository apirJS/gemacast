import { useAppStore } from '../../stores/app-store';
import { Status } from '../../core/types';

function StatItem({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex flex-col items-center">
      <span className="text-[10px] uppercase tracking-wider text-muted-foreground mb-0.5">{label}</span>
      <span className="text-xs font-medium tabular-nums text-foreground whitespace-nowrap">{value}</span>
    </div>
  );
}

export function LatencyStats() {
  const latency = useAppStore((s) => s.latency);
  const status = useAppStore((s) => s.status);

  const visible = status === Status.Connected || status === Status.Playing;
  if (!visible) return null;

  const format = (v: number | null) => (v !== null ? `${v} ms` : '-- ms');

  return (
    <div className="flex items-center justify-center gap-5 text-xs animate-[fade-in_200ms_ease-out] w-full px-4">
      <StatItem label="Now" value={format(latency.current)} />
      <StatItem label="Avg" value={format(latency.avg)} />
      <StatItem label="Max" value={format(latency.max)} />
      <StatItem label="Min" value={format(latency.min)} />
    </div>
  );
}
