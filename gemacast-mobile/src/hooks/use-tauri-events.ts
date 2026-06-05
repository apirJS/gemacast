import { useEffect, useRef } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useAppStore } from '../stores/app-store';
import { useToastStore } from '../stores/toast-store';
import {
  connectToSender,
  handleSenderTimeout,
  handleForceDisconnect,
  disconnect,
} from './use-connection';
import { updateAudioActive, startPlayback, stopPlayback } from './use-audio';
import { LatencyTracker } from '../core/latency-tracker';
import { GemaCastError } from '../core/error';
import type { DiscoveredSender } from '../core/types';

export function useTauriEvents() {
  const trackerRef = useRef(new LatencyTracker());

  useEffect(() => {
    const unlisteners: Promise<UnlistenFn>[] = [];

    unlisteners.push(
      listen<{ latency: number; isActive: boolean }>('audio-telemetry', (event) => {
        const stats = trackerRef.current.update(event.payload.latency);
        useAppStore.getState().updateLatency(stats);
        updateAudioActive(event.payload.isActive);
      }),
    );

    unlisteners.push(
      listen<string>('playback-error', (event) => {
        useAppStore.getState().displayError(GemaCastError.playbackError(event.payload));
      }),
    );

    unlisteners.push(
      listen<string>('discovery-error', (event) => {
        useAppStore.getState().displayError(GemaCastError.discoveryError(event.payload));
      }),
    );

    unlisteners.push(
      listen<DiscoveredSender>('sender-discovered', (event) => {
        const autoReconnectTarget = useAppStore
          .getState()
          .updateDiscoveredSender(event.payload);
        if (autoReconnectTarget) {
          connectToSender(autoReconnectTarget);
        }
      }),
    );

    unlisteners.push(
      listen<string>('sender-timeout', (event) => {
        handleSenderTimeout(event.payload);
      }),
    );

    unlisteners.push(
      listen('force-disconnect', () => {
        const isSuspended = useAppStore.getState().isSuspended;
        handleForceDisconnect(!isSuspended);
      }),
    );

    unlisteners.push(
      listen('ws-disconnect', () => {
        const isSuspended = useAppStore.getState().isSuspended;
        handleForceDisconnect(!isSuspended);
      }),
    );

    unlisteners.push(
      listen<string>('ws-error', (event) => {
        useToastStore.getState().show('warning', event.payload);
      }),
    );

    unlisteners.push(
      listen<string>('service-command', async (event) => {
        const cmd = event.payload;
        if (cmd === 'DISCONNECT') {
          await disconnect(true);
        } else if (cmd === 'STOP_STREAM') {
          await stopPlayback();
        } else if (cmd === 'RESUME') {
          await startPlayback();
        }
      }),
    );

    return () => {
      unlisteners.forEach((p) => p.then((unlisten) => unlisten()));
    };
  }, []);
}
