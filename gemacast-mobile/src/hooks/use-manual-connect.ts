import { useState, useCallback } from 'react';
import { useAppStore } from '../stores/app-store';
import { useToastStore } from '../stores/toast-store';
import { tauriBridge } from '../core/tauri-bridge';
import { connectToSender } from './use-connection';
import { Ports } from '../core/constants';

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
  const [isProbing, setIsProbing] = useState(false);
  const isLoading = useAppStore((s) => s.isLoading);
  const connectingSenderId = useAppStore((s) => s.connectingSenderId);

  const isManualConnecting = isProbing || (isLoading && connectingSenderId?.startsWith('manual-'));

  const handleConnect = useCallback(async () => {
    const trimmed = ip.trim();
    if (!trimmed) return;

    const octets = trimmed.split('.');
    const validIpv4 = octets.length === 4 && octets.every((octet) => /^(0|[1-9]\d{0,2})$/.test(octet) && Number(octet) <= 255);
    const first = Number(octets[0]);
    const last = Number(octets[3]);
    const forbidden = first === 0 || first === 127 || first >= 224 || (first === 255 && last === 255);
    if (!validIpv4 || forbidden) {
      useToastStore.getState().show('warning', 'Invalid IP address');
      return;
    }

    setIsProbing(true);
    useAppStore.getState().patch({ isLoading: true });

    try {
      await tauriBridge.probeSender({
        ip: trimmed,
        deviceId: useAppStore.getState().deviceInfo.deviceId,
      });
    } catch {
      useToastStore.getState().show('warning', 'This IP is unreachable');
      useAppStore.getState().patch({ isLoading: false });
      return;
    } finally {
      setIsProbing(false);
    }

    const manualSender = {
      deviceId: `manual-${trimmed}`,
      deviceName: `Manual: ${trimmed}`,
      addr: `${trimmed}:${Ports.DISCOVERY}`,
      isOffline: false,
    };

    const previousSender = useAppStore.getState().connectedSender;
    const result = await connectToSender(manualSender);
    if (result.ok) {
      const state = useAppStore.getState();
      const existsIndex = state.discoveredSenders.findIndex(
        (s) => s.deviceId === manualSender.deviceId,
      );
      const newList = [...state.discoveredSenders];
      if (existsIndex >= 0) newList.splice(existsIndex, 1);
      newList.unshift(manualSender);
      useAppStore.getState().setDiscoveredSenders(newList);
      setIp('');
    } else if (previousSender) {
      const restored = await connectToSender(previousSender);
      if (!restored.ok) {
        useToastStore.getState().show('warning', 'Could not restore the previous stream');
      }
    }
  }, [ip]);

  return {
    ip,
    setIp,
    isLoading: isManualConnecting,
    handleConnect,
    isDisabled: isLoading || !ip.trim() || isProbing,
  };
}
