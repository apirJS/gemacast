import { useAppStore } from '../../stores/app-store';
import { hasLiveSession } from '../../core/types';
import { StatusChip } from '../device/StatusChip';
import { ConnectionMetrics } from './ConnectionMetrics';
import { NetworkLinkBadge } from './NetworkLinkBadge';

/**
 * The live connection readout card.
 *
 * It renders only when there is a session to report on. Idle, scanning and
 * connecting are deliberately absent: with no metrics to show, the card
 * collapsed to a single line of placeholder text that spent a whole card's worth
 * of space saying nothing. That progress is already reported where it belongs —
 * `EmptyState` covers discovery inside the sender list, `SenderCard` shows its
 * own connecting state, and failures arrive as toasts.
 *
 * `Reconnecting` counts as live (see `hasLiveSession`) because it reports on an
 * *existing* session and answers "why did the audio stop?". The metrics row
 * still hides itself while the link is down, so the card is one line there.
 */
export function ConnectionReadout() {
  const status = useAppStore((s) => s.status);

  if (!hasLiveSession(status)) return null;

  return (
    <section
      aria-label="Connection status"
      className="surface-card rounded-lg px-4 py-3 animate-[fade-in_200ms_ease-out]"
    >
      <div className="flex items-center gap-1.5">
        <StatusChip />
        <NetworkLinkBadge withLeadingSeparator />
      </div>
      <ConnectionMetrics />
    </section>
  );
}
