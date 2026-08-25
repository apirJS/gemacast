import { useAppStore } from '../../stores/app-store';
import { hasLiveSession } from '../../core/types';
import { StatusChip } from '../device/StatusChip';
import { HelpDialog, useHelpDialog } from '../shared/HelpDialog';
import { ConnectionMetrics } from './ConnectionMetrics';
import { NetworkLinkBadge } from './NetworkLinkBadge';

export function ConnectionReadout() {
  const status = useAppStore((s) => s.status);
  const help = useHelpDialog();

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
      <ConnectionMetrics renderHelpButton={help.renderHelpButton} />
      <HelpDialog activeKey={help.activeKey} onClose={help.closeHelp} dialogRef={help.dialogRef} />
    </section>
  );
}
