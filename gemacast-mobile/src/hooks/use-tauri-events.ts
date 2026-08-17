import { useEffect } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useAppStore } from '../stores/app-store';
import { useToastStore } from '../stores/toast-store';
import {
  connectToSender,
  handleSenderTimeout,
  handleForceDisconnect,
  handleLinkLost,
  handleLinkRecovered,
  handleLinkRecoveryGaveUp,
  disconnect,
} from './use-connection';
import { updateAudioActive, startPlayback, stopPlayback } from './use-audio';
import { GemaCastError } from '../core/error';
import type { DiscoveredSender } from '../core/types';

export function useTauriEvents() {
  useEffect(() => {
    const unlisteners: Promise<UnlistenFn>[] = [];

    unlisteners.push(
      listen<{ latency: number; isActive: boolean; jitter: number }>('audio-telemetry', (event) => {
        useAppStore.getState().updateMetrics({
          bufferMs: Math.round(event.payload.latency),
          jitterMs: Math.round(event.payload.jitter),
        });
        updateAudioActive(event.payload.isActive);
      }),
    );

    unlisteners.push(
      listen<number>('network-rtt', (event) => {
        useAppStore.getState().updateMetrics({ networkRttMs: Math.round(event.payload) });
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
        const autoReconnectTarget = useAppStore.getState().updateDiscoveredSender(event.payload);
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

    // The receiver watchdog gave up on its own. Unlike `force-disconnect`,
    // nobody asked for this, so it keeps the sender and probes for its return.
    unlisteners.push(
      listen('link-lost', () => {
        handleLinkLost();
      }),
    );

    unlisteners.push(
      listen<{ deviceRegistered: boolean | null }>('link-recovered', (event) => {
        handleLinkRecovered(event.payload.deviceRegistered);
      }),
    );

    unlisteners.push(
      listen('link-recovery-gave-up', () => {
        handleLinkRecoveryGaveUp();
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
