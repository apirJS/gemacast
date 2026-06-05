import { useAppStore } from '../../stores/app-store';
import { Status } from '../../core/types';

const STATUS_CONFIG: Record<Status, { class: string; label: string } | ((attempts: number) => { class: string; label: string })> = {
  [Status.Idle]: { class: 'bg-muted text-muted-foreground', label: 'Idle' },
  [Status.Listening]: { class: 'bg-status-connecting-bg text-status-connecting border border-status-connecting-border', label: 'Scanning…' },
  [Status.Connecting]: { class: 'bg-status-connecting-bg text-status-connecting border border-status-connecting-border', label: 'Connecting…' },
  [Status.Connected]: { class: 'bg-status-ok-bg text-status-ok border border-status-ok-border', label: 'Connected' },
  [Status.Playing]: { class: 'bg-status-ok-bg text-status-ok border border-status-ok-border', label: 'Playing' },
  [Status.Paused]: { class: 'bg-status-warn-bg text-status-warn border border-status-warn-border', label: 'Paused' },
  [Status.Reconnecting]: (attempts) => ({
    class: 'bg-status-warn-bg text-status-warn border border-status-warn-border',
    label: attempts > 0 ? `Reconnecting (${attempts}/5)…` : 'Reconnecting…',
  }),
};

export function StatusChip() {
  const status = useAppStore((s) => s.status);
  const attempts = useAppStore((s) => s.reconnectAttempts);

  const configEntry = STATUS_CONFIG[status];
  const config = typeof configEntry === 'function' ? configEntry(attempts) : configEntry;

  const showPulse = status === Status.Listening || status === Status.Connecting || status === Status.Reconnecting;

  return (
    <div
      role="status"
      aria-live="polite"
      className={`
        inline-flex items-center gap-2 rounded-full px-3 py-1.5 text-xs font-medium
        transition-colors duration-200 ${config.class}
      `}
    >
      {showPulse && (
        <span className="relative flex h-2 w-2">
          <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-current opacity-75" />
          <span className="relative inline-flex h-2 w-2 rounded-full bg-current" />
        </span>
      )}
      {!showPulse && status === Status.Playing && (
        <span className="relative flex h-2 w-2">
          <span className="relative inline-flex h-2 w-2 rounded-full bg-current" />
        </span>
      )}
      <span>{config.label}</span>
    </div>
  );
}
