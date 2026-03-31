import { listen } from '@tauri-apps/api/event';
import { App } from './App';
import { Status } from './types';
import { GemaCastError } from './error';

window.addEventListener('DOMContentLoaded', async () => {
  const app = await App.create();
  const startBtnEl = document.getElementById('start-btn');
  const playBtnEl = document.getElementById('play-btn');
  const playIconEl = document.querySelector<SVGElement>('.btn__icon--play');
  const pauseIconEl = document.querySelector<SVGElement>('.btn__icon--pause');
  const errorEl = document.querySelector<HTMLDivElement>('.error');
  const errorMessageEl =
    document.querySelector<HTMLSpanElement>('.error__message');
  const errorExpanderEl =
    document.querySelector<HTMLButtonElement>('.error__expander');
  const errorPopoverDetailedMessageEl =
    document.querySelector<HTMLParagraphElement>(
      '.error__popover-detailed-error',
    );
  const infoLatencyEl = document.getElementById('info-latency');
  const infoStatusEl = document.querySelector<HTMLSpanElement>('.info__status');
  const deviceNameEl = document.querySelector<HTMLSpanElement>('.device__name');
  const deviceIpEl = document.querySelector<HTMLSpanElement>('.device__ip');
  const volumeSliderEl = document.getElementById(
    'volume-slider',
  ) as HTMLInputElement | null;
  const volumeValueEl = document.getElementById('volume-value');
  const volumeIconEl = document.getElementById(
    'volume-icon',
  ) as SVGElement | null;

  app.subscribe((state) => {
    if (deviceNameEl) deviceNameEl.textContent = state.deviceInfo.deviceName;
    if (deviceIpEl) deviceIpEl.textContent = 'IP: ' + state.deviceInfo.ip;

    if (state.error !== null) {
      if (errorEl && errorMessageEl && errorExpanderEl) {
        errorEl.hidden = false;
        errorMessageEl.textContent = 'Error: ' + state.error.userMessage;

        if (errorPopoverDetailedMessageEl && state.error.cause) {
          let detailedMsg = 'Detailed error message is unavailable';
          if (state.error.cause instanceof Error) {
            detailedMsg = state.error.cause.message;
          } else if (typeof state.error.cause === 'string') {
            detailedMsg = state.error.cause;
          }
          errorPopoverDetailedMessageEl.textContent = detailedMsg;
        }
      }
    } else {
      if (errorEl) {
        errorEl.hidden = true;
      }
    }

    if (infoLatencyEl) {
      if (
        state.status === Status.Connected ||
        state.status === Status.Playing
      ) {
        infoLatencyEl.removeAttribute('hidden');
      } else {
        infoLatencyEl.setAttribute('hidden', '');
        infoLatencyEl.textContent = 'Latency: -- ms';
      }
    }

    if (infoStatusEl) {
      if (
        (state.status === Status.Connected ||
          state.status === Status.Playing) &&
        state.senderIp
      ) {
        infoStatusEl.textContent =
          'Status: ' + state.status + ' | Sender: ' + state.senderIp;
      } else {
        infoStatusEl.textContent = 'Status: ' + state.status;
      }
    }

    if (startBtnEl) {
      if (
        state.status === Status.Listening ||
        state.status === Status.Connected ||
        state.status === Status.Playing
      ) {
        startBtnEl.textContent = 'Disconnect';
        startBtnEl.classList.add('btn--active');
      } else {
        startBtnEl.textContent = 'Start';
        startBtnEl.classList.remove('btn--active');
      }
    }

    if (playBtnEl) {
      const canPlay =
        state.status === Status.Connected || state.status === Status.Playing;
      (playBtnEl as HTMLButtonElement).disabled = !canPlay;

      if (playIconEl && pauseIconEl) {
        if (state.status === Status.Playing) {
          playIconEl.setAttribute('hidden', '');
          pauseIconEl.removeAttribute('hidden');
        } else {
          pauseIconEl.setAttribute('hidden', '');
          playIconEl.removeAttribute('hidden');
        }
      }
    }

    if (volumeSliderEl) {
      volumeSliderEl.value = String(Math.round(state.volume * 100));
      const canAdjust =
        state.status === Status.Connected || state.status === Status.Playing;

      volumeSliderEl.disabled = !canAdjust;
    }

    if (volumeValueEl) {
      volumeValueEl.textContent = Math.round(state.volume * 100) + '%';
    }

    if (volumeIconEl) {
      const pct = state.volume * 100;
      let iconPath: string;
      if (pct === 0) {
        // Muted icon
        iconPath =
          'M16.5 12c0-1.77-1.02-3.29-2.5-4.03v2.21l2.45 2.45c.03-.2.05-.41.05-.63zm2.5 0c0 .94-.2 1.82-.54 2.64l1.51 1.51C20.63 14.91 21 13.5 21 12c0-4.28-2.99-7.86-7-8.77v2.06c2.89.86 5 3.54 5 6.71zM4.27 3L3 4.27 7.73 9H3v6h4l5 5v-6.73l4.25 4.25c-.67.52-1.42.93-2.25 1.18v2.06c1.38-.31 2.63-.95 3.69-1.81L19.73 21 21 19.73l-9-9L4.27 3zM12 4L9.91 6.09 12 8.18V4z';
      } else if (pct <= 33) {
        // Low volume
        iconPath = 'M7 9v6h4l5 5V4l-5 5H7z';
      } else if (pct <= 66) {
        // Medium volume
        iconPath =
          'M18.5 12c0-1.77-1.02-3.29-2.5-4.03v8.05c1.48-.73 2.5-2.25 2.5-4.02zM5 9v6h4l5 5V4L9 9H5z';
      } else {
        // High volume
        iconPath =
          'M3 9v6h4l5 5V4L7 9H3zm13.5 3c0-1.77-1.02-3.29-2.5-4.03v8.05c1.48-.73 2.5-2.25 2.5-4.02zM14 3.23v2.06c2.89.86 5 3.54 5 6.71s-2.11 5.85-5 6.71v2.06c4.01-.91 7-4.49 7-8.77s-2.99-7.86-7-8.77z';
      }
      const pathEl = volumeIconEl.querySelector('path');
      if (pathEl) pathEl.setAttribute('d', iconPath);
    }
  });

  startBtnEl?.addEventListener('click', async () => {
    const status = app.getState().status;
    if (
      status === Status.Listening ||
      status === Status.Connected ||
      status === Status.Playing
    ) {
      await app.stopDiscovery();
    } else {
      await app.startDiscovery();
    }
  });

  volumeSliderEl?.addEventListener('input', async () => {
    if (!volumeSliderEl) return;
    const level = parseFloat(volumeSliderEl.value) / 100;
    await app.setVolume(level);
  });

  playBtnEl?.addEventListener('click', async () => {
    let status = app.getState().status;
    if (status === Status.Connected) {
      await app.startAudioPlayback();
    } else if (status === Status.Playing) {
      await app.stopAudioPlayback();
    }
  });

  listen<string>('sender-connected', (event) => {
    app.setSenderIp(event.payload);
  });

  listen<number>('latency-update', (event) => {
    if (infoLatencyEl) {
      infoLatencyEl.textContent = `Latency: ${event.payload.toFixed(0)} ms`;
    }
  });

  listen<string>('playback-error', (event) => {
    app.displayError(GemaCastError.playbackError(event.payload));
  });

  listen<string>('discovery-error', (event) => {
    app.displayError(GemaCastError.discoveryError(event.payload));
  });
});
