import { useEffect, useMemo, useRef, useState } from 'react';
import {
  forgetPcName,
  loadLastStreamer,
  loadPcNames,
  saveLastMode,
  saveLastStreamer,
} from '../core/persistence';
import { tauriBridge } from '../core/tauri-bridge';
import { useAppStore } from '../stores/app-store';
import { useToastStore } from '../stores/toast-store';
import { connectionController } from './connection-controller';

export type PairedPc = {
  deviceId: string;
  deviceName: string;
};

export function usePairedPcController() {
  const connectedStreamer = useAppStore((state) => state.connectedStreamer);
  const connectedStreamerId = connectedStreamer?.deviceId;
  const lastConnectedStreamer = useAppStore((state) => state.lastConnectedStreamer);
  const discoveredStreamers = useAppStore((state) => state.discoveredStreamers);
  const [pairedPcIds, setPairedPcIds] = useState<string[]>([]);
  const [rememberedNames, setRememberedNames] = useState<Record<string, string>>({});
  const [selectedPc, setSelectedPc] = useState<PairedPc | null>(null);
  const loaded = useRef(false);

  useEffect(() => {
    if (loaded.current && !connectedStreamerId) return;
    loaded.current = true;
    let active = true;
    void tauriBridge
      .getPairedPcIds()
      .then((ids) => {
        if (!active) return;
        setPairedPcIds([...new Set(ids)]);
        setRememberedNames(loadPcNames());
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

  const pairedPcs = useMemo(() => {
    const names = new Map<string, string>(Object.entries(rememberedNames));
    for (const streamer of [...discoveredStreamers, lastConnectedStreamer, connectedStreamer]) {
      if (streamer?.deviceName) names.set(streamer.deviceId, streamer.deviceName);
    }
    return pairedPcIds.map((deviceId) => ({
      deviceId,
      deviceName: names.get(deviceId) ?? deviceId,
    }));
  }, [connectedStreamer, discoveredStreamers, lastConnectedStreamer, pairedPcIds, rememberedNames]);

  const forgetSelected = async (): Promise<void> => {
    if (!selectedPc) return;
    const pc = selectedPc;
    setSelectedPc(null);
    try {
      if (useAppStore.getState().connectedStreamer?.deviceId === pc.deviceId) {
        const result = await connectionController.disconnect(true);
        if (!result.ok) throw result.error;
      }
      await tauriBridge.forgetPcIdentity(pc.deviceId);
      forgetPcName(pc.deviceId);
      if (loadLastStreamer()?.deviceId === pc.deviceId) {
        saveLastStreamer(null);
        saveLastMode(null);
        useAppStore.getState().patch({ lastConnectedStreamer: null, lastConnectedMode: null });
      }
      setPairedPcIds((ids) => ids.filter((id) => id !== pc.deviceId));
      useToastStore.getState().show('success', `Forgot ${pc.deviceName}`);
    } catch (error) {
      useToastStore.getState().show('error', `Could not forget ${pc.deviceName}`, String(error));
    }
  };

  return {
    pairedPcs,
    selectedPc,
    connectedPcId: connectedStreamer?.deviceId,
    select: setSelectedPc,
    cancel: () => setSelectedPc(null),
    forgetSelected,
  };
}
