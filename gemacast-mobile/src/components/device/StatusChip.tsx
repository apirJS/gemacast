import { useAppStore } from '../../stores/app-store';
import { Status } from '../../core/types';

const STATUS_CONFIG: Record<
  Status,
  | { dot: string; label: string; glow?: string }
  | ((attempts: number) => { dot: string; label: string; glow?: string })
> = {
  [Status.Idle]: { dot: 'bg-muted-foreground/40', label: 'Idle' },
  [Status.Listening]: {
    dot: 'bg-status-connecting',
    glow: 'shadow-[0_0_6px_2px_var(--color-status-connecting)]',
    label: 'Scanning…',
  },
  [Status.Connecting]: {
    dot: 'bg-status-connecting',
    glow: 'shadow-[0_0_6px_2px_var(--color-status-connecting)]',
    label: 'Connecting…',
  },
  [Status.Connected]: {
    dot: 'bg-status-ok',
    glow: 'shadow-[0_0_6px_2px_var(--color-status-ok)]',
    label: 'Connected',
  },
  [Status.Playing]: {
    dot: 'bg-status-ok',
    glow: 'shadow-[0_0_6px_2px_var(--color-status-ok)]',
    label: 'Playing',
  },
  [Status.Paused]: {
    dot: 'bg-status-warn',
    label: 'Paused',
  },
  [Status.Reconnecting]: (attempts) => ({
    dot: 'bg-status-warn',
    glow: 'shadow-[0_0_6px_2px_var(--color-status-warn)]',
    label: attempts > 0 ? `Reconnecting (${attempts}/5)…` : 'Reconnecting…',
  }),
};

export function StatusChip() {
  const status = useAppStore((s) => s.status);
  const attempts = useAppStore((s) => s.reconnectAttempts);

  const configEntry = STATUS_CONFIG[status];
  const config = typeof configEntry === 'function' ? configEntry(attempts) : configEntry;

  const isActive =
    status === Status.Listening || status === Status.Connecting || status === Status.Reconnecting;

  const isLive = status === Status.Playing || status === Status.Connected;

  const showBreathing = isActive || isLive;

  return (
    <div
      role="status"
      aria-live="polite"
      className={`
        status-chip
        inline-flex items-center gap-2 rounded-full px-4 py-1.5
        text-xs font-medium tracking-wide
        text-muted-foreground
        transition-all duration-300
      `}
    >
      <span
        className={`
          relative flex h-2 w-2 shrink-0
        `}
      >
        {/* Breathing ring for active/live states */}
        {showBreathing && (
          <span
            className={`
              absolute inset-0 rounded-full opacity-75
              ${config.dot}
              animate-[status-breathe_2s_ease-in-out_infinite]
            `}
          />
        )}
        {/* Core dot */}
        <span
          className={`
            relative inline-flex h-2 w-2 rounded-full
            ${config.dot}
            ${config.glow ?? ''}
            transition-all duration-300
          `}
        />
      </span>
      <span>{config.label}</span>
    </div>
  );
}
