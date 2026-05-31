import { useState, useCallback } from 'react';
import { useAppStore } from '../stores/app-store';
import { useToastStore } from '../stores/toast-store';
import { tauriBridge } from '../core/tauri-bridge';
import { connectToSender, disconnect } from './use-connection';

/**
 * Hook that encapsulates the "connect by IP address" business logic:
 * - IP validation
 * - Reachability probe
 * - Manual sender creation
 * - Connect/disconnect orchestration
 * - Discovery list mutation
 *
 * The ManualConnect component becomes a pure form renderer.
 */
export function useManualConnect() {
  const [ip, setIp] = useState('');
  const isLoading = useAppStore((s) => s.isLoading);

  const handleConnect = useCallback(async () => {
    const trimmed = ip.trim();
    if (!trimmed) return;

    const ipRegex = /^(?:[0-9]{1,3}\.){3}[0-9]{1,3}$/;
    if (!ipRegex.test(trimmed)) {
      useToastStore.getState().show('warning', 'Invalid IP address');
      return;
    }

    useAppStore.getState().patch({ isLoading: true });

    try {
      await tauriBridge.getAudioSources({ ip: trimmed });
    } catch {
      useToastStore.getState().show('warning', 'This IP is unreachable');
      useAppStore.getState().patch({ isLoading: false });
      return;
    }

    const manualSender = {
      deviceId: `manual-${trimmed}`,
      deviceName: `Manual: ${trimmed}`,
      addr: `${trimmed}:55555`,
      isOffline: false,
    };

    const connectedSender = useAppStore.getState().connectedSender;
    if (connectedSender) {
      await disconnect();
    }

    const result = await connectToSender(manualSender);
    if (result.ok) {
      const state = useAppStore.getState();
      const existsIndex = state.discoveredSenders.findIndex(
        (s) => s.deviceId === manualSender.deviceId
      );
      const newList = [...state.discoveredSenders];
      if (existsIndex >= 0) newList.splice(existsIndex, 1);
      newList.unshift(manualSender);
      useAppStore.getState().setDiscoveredSenders(newList);
      setIp('');
    }
  }, [ip]);

  return {
    ip,
    setIp,
    isLoading,
    handleConnect,
    isDisabled: isLoading || !ip.trim(),
  };
}
