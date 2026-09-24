import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useAppStore } from '../stores/app-store';
import { useToastStore } from '../stores/toast-store';
import { connectionController } from './connection-controller';
import { playbackController } from './playback-controller';
import { GemaCastError } from '../core/error';
import type { DiscoveredStreamer } from '../core/types';

export class AppEventsController {
  start(): () => void {
    const unlisteners: Promise<UnlistenFn>[] = [];

    unlisteners.push(
      listen<{ latency: number; isActive: boolean; jitter: number }>('audio-telemetry', (event) => {
        useAppStore.getState().updateMetrics({
          bufferMs: Math.round(event.payload.latency),
          jitterMs: Math.round(event.payload.jitter),
        });
        playbackController.reportActivity(event.payload.isActive);
      }),
    );

    unlisteners.push(
      listen<number>('network-rtt', (event) => {
        useAppStore.getState().updateMetrics({ networkRttMs: Math.round(event.payload) });
      }),
    );

    unlisteners.push(
      listen<number>('pc-volume-changed', (event) => {
        useAppStore.getState().setPcOutputVolume(event.payload);
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
      listen<DiscoveredStreamer>('streamer-discovered', (event) => {
        const autoReconnectTarget = useAppStore.getState().updateDiscoveredStreamer(event.payload);
        if (autoReconnectTarget) {
          void connectionController.connect(autoReconnectTarget);
        }
      }),
    );

    unlisteners.push(
      listen<string>('streamer-timeout', (event) => {
        connectionController.streamerTimedOut(event.payload);
      }),
    );

    unlisteners.push(
      listen('force-disconnect', () => {
        const isSuspended = useAppStore.getState().isSuspended;
        connectionController.forceDisconnect(!isSuspended);
      }),
    );

    unlisteners.push(
      listen('link-lost', () => {
        void connectionController.linkLost();
      }),
    );

    unlisteners.push(
      listen<{ deviceRegistered: boolean | null }>('link-recovered', (event) => {
        void connectionController.linkRecovered(event.payload.deviceRegistered);
      }),
    );

    unlisteners.push(
      listen('link-recovery-gave-up', () => {
        connectionController.linkRecoveryGaveUp();
      }),
    );

    unlisteners.push(
      listen('ws-disconnect', () => {
        const isSuspended = useAppStore.getState().isSuspended;
        connectionController.forceDisconnect(!isSuspended);
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
          await connectionController.disconnect(true);
        } else if (cmd === 'STOP_STREAM') {
          await playbackController.stop();
        } else if (cmd === 'RESUME') {
          await playbackController.start();
        }
      }),
    );

    return () => {
      unlisteners.forEach((p) => p.then((unlisten) => unlisten()));
    };
  }
}

export const appEventsController = new AppEventsController();
