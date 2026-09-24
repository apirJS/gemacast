import { Trash2 } from 'lucide-react';
import { usePairedPcController, type PairedPc } from '../../controllers';
import { ConfirmDialog } from '../shared/ConfirmDialog';

type PairedPcListViewProps = {
  pairedPcs: PairedPc[];
  selectedPc: PairedPc | null;
  connectedPcId?: string;
  onSelect: (pc: PairedPc) => void;
  onCancel: () => void;
  onForget: () => void;
};

export function PairedPcListView({
  pairedPcs,
  selectedPc,
  connectedPcId,
  onSelect,
  onCancel,
  onForget,
}: PairedPcListViewProps) {
  return (
    <div className="space-y-2">
      {pairedPcs.length === 0 ? (
        <p className="text-xs text-muted-foreground/70">No paired PCs</p>
      ) : (
        <div
          className="max-h-40 space-y-1 overflow-y-auto overscroll-contain pr-1"
          role="list"
          aria-label="Paired PCs"
        >
          {pairedPcs.map((pc) => (
            <div
              key={pc.deviceId}
              className="flex items-start justify-between gap-3 text-sm"
              role="listitem"
            >
              <span className="min-w-0 flex-1 wrap-anywhere leading-snug">{pc.deviceName}</span>
              <button
                type="button"
                className="shrink-0 rounded-default p-2 text-muted-foreground hover:bg-muted hover:text-status-lost"
                title={`Forget ${pc.deviceName}`}
                aria-label={`Forget ${pc.deviceName}`}
                onClick={() => onSelect(pc)}
              >
                <Trash2 className="h-4 w-4" />
              </button>
            </div>
          ))}
        </div>
      )}
      <ConfirmDialog
        open={selectedPc !== null}
        message={
          selectedPc
            ? selectedPc.deviceId === connectedPcId
              ? `Disconnect from and forget ${selectedPc.deviceName}?`
              : `Forget the saved identity for ${selectedPc.deviceName}?`
            : ''
        }
        confirmLabel="Forget"
        onConfirm={onForget}
        onCancel={onCancel}
      />
    </div>
  );
}

export function ForgetPcIdentity() {
  const controller = usePairedPcController();
  return (
    <PairedPcListView
      pairedPcs={controller.pairedPcs}
      selectedPc={controller.selectedPc}
      connectedPcId={controller.connectedPcId}
      onSelect={controller.select}
      onCancel={controller.cancel}
      onForget={() => void controller.forgetSelected()}
    />
  );
}
