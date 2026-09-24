import { useState } from 'react';
import { useAppStore } from '../stores/app-store';
import { useToastStore } from '../stores/toast-store';
import { tauriBridge } from '../core/tauri-bridge';
import { connectionController } from './connection-controller';
import { Ports } from '../core/constants';

/**
 * Hook that encapsulates the "connect by IP address" business logic:
 * - IP validation
 * - Reachability probe
 * - Manual streamer creation
 * - Connect/disconnect orchestration
 * - Discovery list mutation
 *
 * The ManualConnect component becomes a pure form renderer.
 */
export class ManualConnectionController {
  isValidAddress(address: string): boolean {
    const octets = address.split('.');
    const validIpv4 =
      octets.length === 4 &&
      octets.every((octet) => /^(0|[1-9]\d{0,2})$/.test(octet) && Number(octet) <= 255);
    const first = Number(octets[0]);
    const last = Number(octets[3]);
    const forbidden =
      first === 0 || first === 127 || first >= 224 || (first === 255 && last === 255);
    return validIpv4 && !forbidden;
  }

  async connect(address: string): Promise<boolean> {
    if (!this.isValidAddress(address)) {
      useToastStore.getState().show('warning', 'Invalid IP address');
      return false;
    }

    useAppStore.getState().patch({ isLoading: true });
    try {
      await tauriBridge.probeStreamer({
        ip: address,
        deviceId: useAppStore.getState().deviceInfo.deviceId,
      });
    } catch {
      useToastStore.getState().show('warning', 'This IP is unreachable');
      useAppStore.getState().patch({ isLoading: false });
      return false;
    }

    const manualStreamer = {
      deviceId: `manual-${address}`,
      deviceName: `Manual: ${address}`,
      addr: `${address}:${Ports.DISCOVERY}`,
      isOffline: false,
    };
    const previousStreamer = useAppStore.getState().connectedStreamer;
    const result = await connectionController.connect(manualStreamer);
    if (result.ok) {
      const state = useAppStore.getState();
      const withoutDuplicate = state.discoveredStreamers.filter(
        (streamer) => streamer.deviceId !== manualStreamer.deviceId,
      );
      state.setDiscoveredStreamers([manualStreamer, ...withoutDuplicate]);
      return true;
    }

    if (previousStreamer) {
      const restored = await connectionController.connect(previousStreamer);
      if (!restored.ok) {
        useToastStore.getState().show('warning', 'Could not restore the previous stream');
      }
    }
    return false;
  }
}

export const manualConnectionController = new ManualConnectionController();

export function useManualConnectionController() {
  const [ip, setIp] = useState('');
  const [isProbing, setIsProbing] = useState(false);
  const isLoading = useAppStore((s) => s.isLoading);
  const connectingStreamerId = useAppStore((s) => s.connectingStreamerId);

  const isManualConnecting =
    isProbing || Boolean(isLoading && connectingStreamerId?.startsWith('manual-'));

  const handleConnect = async () => {
    const trimmed = ip.trim();
    if (!trimmed) return;

    setIsProbing(true);
    try {
      const connected = await manualConnectionController.connect(trimmed);
      if (connected) setIp('');
    } finally {
      setIsProbing(false);
    }
  };

  return {
    ip,
    setIp,
    isLoading: isManualConnecting,
    handleConnect,
    isDisabled: isLoading || !ip.trim() || isProbing,
  };
}
