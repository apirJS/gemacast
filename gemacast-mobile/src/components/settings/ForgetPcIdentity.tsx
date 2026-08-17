import { useEffect, useMemo, useRef, useState } from 'react';
import { Trash2 } from 'lucide-react';
import { tauriBridge } from '../../core/tauri-bridge';
import { disconnect } from '../../hooks/use-connection';
import { useAppStore } from '../../stores/app-store';
import { useToastStore } from '../../stores/toast-store';
import { ConfirmDialog } from '../shared/ConfirmDialog';

export function ForgetPcIdentity() {
  const connectedSender = useAppStore((state) => state.connectedSender);
  const connectedSenderId = connectedSender?.deviceId;
  const lastConnectedSender = useAppStore((state) => state.lastConnectedSender);
  const discoveredSenders = useAppStore((state) => state.discoveredSenders);
  const [pairedPcIds, setPairedPcIds] = useState<string[]>([]);
  const [selectedPc, setSelectedPc] = useState<{ deviceId: string; deviceName: string } | null>(null);
  const hasLoadedPairedPcs = useRef(false);

  useEffect(() => {
    if (hasLoadedPairedPcs.current && !connectedSenderId) return;
    hasLoadedPairedPcs.current = true;
    let active = true;
    void tauriBridge
      .getPairedPcIds()
      .then((ids) => {
        if (active) setPairedPcIds([...new Set(ids)]);
      })
      .catch((error) => {
        if (active) {
          useToastStore.getState().show('error', 'Could not load paired PCs', String(error));
        }
      });
    return () => {
      active = false;
    };
  }, [connectedSenderId]);

  const senders = useMemo(() => {
    const byId = new Map<string, { deviceId: string; deviceName: string }>();
    for (const sender of [...discoveredSenders, lastConnectedSender, connectedSender]) {
      if (sender) byId.set(sender.deviceId, sender);
    }
    return pairedPcIds.map((deviceId) => ({
      deviceId,
      deviceName: byId.get(deviceId)?.deviceName ?? deviceId,
    }));
  }, [connectedSender, discoveredSenders, lastConnectedSender, pairedPcIds]);

  const forget = async () => {
    if (!selectedPc) return;
    const pc = selectedPc;
    setSelectedPc(null);
    try {
      if (useAppStore.getState().connectedSender?.deviceId === pc.deviceId) {
        const result = await disconnect(true);
        if (!result.ok) throw result.error;
      }
      await tauriBridge.forgetPcIdentity(pc.deviceId);
      setPairedPcIds((ids) => ids.filter((id) => id !== pc.deviceId));
      useToastStore.getState().show('success', `Forgot ${pc.deviceName}`);
    } catch (error) {
      useToastStore.getState().show('error', `Could not forget ${pc.deviceName}`, String(error));
    }
  };

  return (
    <div className="space-y-2">
      {senders.length === 0 ? (
        <p className="text-xs text-muted-foreground/70">No paired PCs</p>
      ) : (
        <div
          className="max-h-40 space-y-1 overflow-y-auto overscroll-contain pr-1"
          role="list"
          aria-label="Paired PCs"
        >
          {senders.map((sender) => (
            <div
              key={sender.deviceId}
              className="flex items-start justify-between gap-3 text-sm"
              role="listitem"
            >
              <span className="min-w-0 flex-1 wrap-anywhere leading-snug">
                {sender.deviceName}
              </span>
              <button
                type="button"
                className="shrink-0 rounded-default p-2 text-muted-foreground hover:bg-muted hover:text-status-lost"
                title={`Forget ${sender.deviceName}`}
                aria-label={`Forget ${sender.deviceName}`}
                onClick={() => setSelectedPc(sender)}
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
            ? selectedPc.deviceId === connectedSenderId
              ? `Disconnect from and forget ${selectedPc.deviceName}?`
              : `Forget the saved identity for ${selectedPc.deviceName}?`
            : ''
        }
        confirmLabel="Forget"
        onConfirm={forget}
        onCancel={() => setSelectedPc(null)}
      />
    </div>
  );
}
