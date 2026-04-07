import { listen } from '@tauri-apps/api/event';
import { App } from '../App';
import { DiscoveredSender } from '../types';
import { GemaCastError } from '../error';

export function listenForTauriEvents(app: App) {
  listen<number>('latency-update', (event) => {
    app.latency.updateLatency(event.payload);
  });

  listen<boolean>('audio-active', (event) => {
    app.audio.updateAudioActive(event.payload);
  });

  listen<string>('playback-error', (event) => {
    app.stateHandler.displayError(GemaCastError.playbackError(event.payload));
  });

  listen<string>('discovery-error', (event) => {
    app.stateHandler.displayError(GemaCastError.discoveryError(event.payload));
  });

  listen<DiscoveredSender>('sender-discovered', (event) => {
    app.discovery.updateDiscoveredSender(event.payload);
  });

  listen<string>('sender-timeout', (event) => {
    app.connection.handleSenderTimeout(event.payload);
  });

  listen('force-disconnect', () => {
    app.connection.handleForceDisconnect();
  });
  listen<string>('service-command', async (event) => {
    const cmd = event.payload;
    if (cmd === 'DISCONNECT') {
      await app.connection.disconnect(true);
    } else if (cmd === 'STOP_STREAM') {
      await app.connection.disconnect(false);
    } else if (cmd === 'RESUME') {
      const state = app.stateHandler.getState();
      const target = state.lastConnectedSender;
      if (target) {
        await app.connection.connectToSender(target);
      }
    } else if (cmd === 'VOL_UP' || cmd === 'VOL_DOWN') {
      const state = app.stateHandler.getState();
      const sender = state.connectedSender;
      if (sender && sender.volume !== undefined && sender.volume !== null) {
        const step = 0.05;
        let newVol = sender.volume + (cmd === 'VOL_UP' ? step : -step);
        newVol = Math.max(0.0, Math.min(1.0, newVol));
        await app.audio.setRemoteVolume(newVol);
      }
    }
  });
}
