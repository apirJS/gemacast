import { invoke } from '@tauri-apps/api/core';
import { App } from '../App';
import { DiscoveredSender, Status } from '../types';
import { toastManager } from './toast';

export function setupManualConnect(app: App) {
  const container = document.getElementById('manual-connect-container');
  const input = document.getElementById('manual-connect-ip') as HTMLInputElement;
  const btn = document.getElementById('manual-connect-btn') as HTMLButtonElement;

  if (!container || !input || !btn) return;

  btn.addEventListener('click', async () => {
    const ip = input.value.trim();
    if (!ip) return;

    const ipRegex = /^(?:[0-9]{1,3}\.){3}[0-9]{1,3}$/;
    if (!ipRegex.test(ip)) {
      toastManager.showWarning('Invalid IP address');
      return;
    }

    app.stateHandler.setState({
      isLoading: true,
      status: Status.Connecting,
      connectingSenderId: `manual-${ip}`,
    });

    try {
      await invoke('get_audio_sources', { ip });
    } catch (e) {
      toastManager.showWarning('This IP is unreachable');
      app.stateHandler.setState({
        isLoading: false,
        status: Status.Listening,
        connectingSenderId: null,
      });
      return;
    }

    const manualSender: DiscoveredSender = {
      deviceId: `manual-${ip}`,
      deviceName: `Manual: ${ip}`,
      addr: `${ip}:55555`,
      isOffline: false,
    };

    const state = app.stateHandler.getState();
    if (state.connectedSender) {
      await app.connection.disconnect();
    }

    const result = await app.connection.connectToSender(manualSender);
    if (result.ok) {
      const newState = app.stateHandler.getState();
      const existsIndex = newState.discoveredSenders.findIndex(s => s.deviceId === manualSender.deviceId);

      const newList = [...newState.discoveredSenders];
      if (existsIndex >= 0) {
        newList.splice(existsIndex, 1);
      }

      newList.unshift(manualSender);

      app.stateHandler.setState({
        discoveredSenders: newList
      });
      input.value = '';
    }
  });

  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') {
      btn.click();
    }
  });

  app.stateHandler.subscribe((state) => {
    const isConnectingState = state.isLoading || state.status === Status.Connecting;
    const isConnectingToThis = state.status === Status.Connecting && state.connectingSenderId?.startsWith('manual-');

    btn.disabled = isConnectingState;
    input.disabled = isConnectingState;

    if (isConnectingToThis) {
      btn.classList.add('manual-connect__btn--loading');
      btn.textContent = '';
    } else {
      btn.classList.remove('manual-connect__btn--loading');
      btn.textContent = 'connect';
    }
  });
}
