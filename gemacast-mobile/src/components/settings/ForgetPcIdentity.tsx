import { useEffect, useRef, useState } from 'react';
import { Trash2 } from 'lucide-react';
import { tauriBridge } from '../../core/tauri-bridge';
import { forgetPcName, loadPcNames } from '../../core/persistence';
import { disconnect } from '../../hooks/use-connection';
import { useAppStore } from '../../stores/app-store';
import { useToastStore } from '../../stores/toast-store';
import { ConfirmDialog } from '../shared/ConfirmDialog';

export function ForgetPcIdentity() {
  const connectedStreamer = useAppStore((state) => state.connectedStreamer);
  const connectedStreamerId = connectedStreamer?.deviceId;
  const lastConnectedStreamer = useAppStore((state) => state.lastConnectedStreamer);
  const discoveredStreamers = useAppStore((state) => state.discoveredStreamers);
  const [pairedPcIds, setPairedPcIds] = useState<string[]>([]);
  const [rememberedNames, setRememberedNames] = useState<Record<string, string>>({});
  const [selectedPc, setSelectedPc] = useState<{ deviceId: string; deviceName: string } | null>(
    null,
  );
  const hasLoadedPairedPcs = useRef(false);

  useEffect(() => {
    if (hasLoadedPairedPcs.current && !connectedStreamerId) return;
    hasLoadedPairedPcs.current = true;
    let active = true;
    void tauriBridge
      .getPairedPcIds()
      .then((ids) => {
        if (active) {
          setPairedPcIds([...new Set(ids)]);
          // Read the cache alongside the trust store rather than during render,
          // so the two always describe the same moment.
          setRememberedNames(loadPcNames());
        }
      })
      .catch((error) => {
        if (active) {
          useToastStore.getState().show('error', 'Could not load paired PCs', String(error));
        }
      });
    return () => {
      active = false;
    };
  }, [connectedStreamerId]);

  const streamers = (() => {
    // Cached names first, then live state on top: a PC that is currently
    // discovered or connected has the freshest name, but the cache is the only
    // source that survives Wi-Fi dropping or a switch through ADB.
    const byId = new Map<string, string>(Object.entries(rememberedNames));
    for (const streamer of [...discoveredStreamers, lastConnectedStreamer, connectedStreamer]) {
      if (streamer?.deviceName) byId.set(streamer.deviceId, streamer.deviceName);
    }
    return pairedPcIds.map((deviceId) => ({
      deviceId,
      deviceName: byId.get(deviceId) ?? deviceId,
    }));
  })();

  const forget = async () => {
    if (!selectedPc) return;
    const pc = selectedPc;
    setSelectedPc(null);
    try {
      if (useAppStore.getState().connectedStreamer?.deviceId === pc.deviceId) {
        const result = await disconnect(true);
        if (!result.ok) throw result.error;
      }
      await tauriBridge.forgetPcIdentity(pc.deviceId);
      forgetPcName(pc.deviceId);
      setPairedPcIds((ids) => ids.filter((id) => id !== pc.deviceId));
      useToastStore.getState().show('success', `Forgot ${pc.deviceName}`);
    } catch (error) {
      useToastStore.getState().show('error', `Could not forget ${pc.deviceName}`, String(error));
    }
  };

  return (
    <div className="space-y-2">
      {streamers.length === 0 ? (
        <p className="text-xs text-muted-foreground/70">No paired PCs</p>
      ) : (
        <div
          className="max-h-40 space-y-1 overflow-y-auto overscroll-contain pr-1"
          role="list"
          aria-label="Paired PCs"
        >
          {streamers.map((streamer) => (
            <div
              key={streamer.deviceId}
              className="flex items-start justify-between gap-3 text-sm"
              role="listitem"
            >
              <span className="min-w-0 flex-1 wrap-anywhere leading-snug">{streamer.deviceName}</span>
              <button
                type="button"
                className="shrink-0 rounded-default p-2 text-muted-foreground hover:bg-muted hover:text-status-lost"
                title={`Forget ${streamer.deviceName}`}
                aria-label={`Forget ${streamer.deviceName}`}
                onClick={() => setSelectedPc(streamer)}
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
            ? selectedPc.deviceId === connectedStreamerId
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
