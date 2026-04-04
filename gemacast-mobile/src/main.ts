import { listen } from '@tauri-apps/api/event';
import { App } from './App';
import { Status, DiscoveredSender } from './types';
import { GemaCastError } from './error';

window.addEventListener('DOMContentLoaded', async () => {
  const app = await App.create();
  const errorEl = document.querySelector<HTMLDivElement>('.error');
  const errorMessageEl = document.querySelector<HTMLSpanElement>('.error__message');
  const errorExpanderEl = document.querySelector<HTMLButtonElement>('.error__expander');
  const errorPopoverDetailedMessageEl = document.querySelector<HTMLParagraphElement>('.error__popover-detailed-error');
  const infoLatencyEl = document.getElementById('info-latency');
  const infoStatusEl = document.querySelector<HTMLSpanElement>('.info__status');
  const deviceNameEl = document.querySelector<HTMLSpanElement>('.device__name');
  const deviceIpEl = document.querySelector<HTMLSpanElement>('.device__ip');
  const volumeSliderEl = document.getElementById('volume-slider') as HTMLInputElement | null;
  const volumeValueEl = document.getElementById('volume-value');
  const volumeIconEl = document.getElementById('volume-icon') as SVGElement | null;
  const muteBtnEl = document.getElementById('mute-btn') as HTMLButtonElement | null;
  const senderListEl = document.getElementById('sender-list');

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
      if (state.status === Status.Playing || state.status === Status.Connected) {
        infoLatencyEl.removeAttribute('hidden');
      } else {
        infoLatencyEl.setAttribute('hidden', '');
        infoLatencyEl.textContent = 'Latency: -- ms';
      }
    }

    if (infoStatusEl) {
      if (state.connectedSender) {
        infoStatusEl.textContent = 'Status: ' + state.status + ' | Sender: ' + state.connectedSender.addr.split(':')[0];
      } else {
        infoStatusEl.textContent = 'Status: ' + state.status;
      }
    }

    if (volumeSliderEl) {
      volumeSliderEl.value = String(Math.round(state.volume * 100));
      volumeSliderEl.disabled = false;
    }

    if (volumeValueEl) {
      volumeValueEl.textContent = Math.round(state.volume * 100) + '%';
    }

    if (volumeIconEl) {
      const pathEl = volumeIconEl.querySelector('path');
      if (pathEl) {
        let iconPath: string;
        if (state.isMuted || state.volume === 0) {
          // Muted icon
          iconPath =
            'M16.5 12c0-1.77-1.02-3.29-2.5-4.03v2.21l2.45 2.45c.03-.2.05-.41.05-.63zm2.5 0c0 .94-.2 1.82-.54 2.64l1.51 1.51C20.63 14.91 21 13.5 21 12c0-4.28-2.99-7.86-7-8.77v2.06c2.89.86 5 3.54 5 6.71zM4.27 3L3 4.27 7.73 9H3v6h4l5 5v-6.73l4.25 4.25c-.67.52-1.42.93-2.25 1.18v2.06c1.38-.31 2.63-.95 3.69-1.81L19.73 21 21 19.73l-9-9L4.27 3zM12 4L9.91 6.09 12 8.18V4z';
        } else if (state.volume <= 0.33) {
          iconPath = 'M7 9v6h4l5 5V4l-5 5H7z';
        } else if (state.volume <= 0.66) {
          iconPath =
            'M18.5 12c0-1.77-1.02-3.29-2.5-4.03v8.05c1.48-.73 2.5-2.25 2.5-4.02zM5 9v6h4l5 5V4L9 9H5z';
        } else {
          iconPath =
            'M3 9v6h4l5 5V4L7 9H3zm13.5 3c0-1.77-1.02-3.29-2.5-4.03v8.05c1.48-.73 2.5-2.25 2.5-4.02zM14 3.23v2.06c2.89.86 5 3.54 5 6.71s-2.11 5.85-5 6.71v2.06c4.01-.91 7-4.49 7-8.77s-2.99-7.86-7-8.77z';
        }
        pathEl.setAttribute('d', iconPath);
      }
      volumeIconEl.style.opacity = state.isMuted ? '0.5' : '1';
    }

    if (senderListEl) {
      senderListEl.innerHTML = '';
      state.discoveredSenders.forEach(sender => {
        const isConnected = state.connectedSender?.deviceId === sender.deviceId;
        
        const li = document.createElement('li');
        li.className = `sender-list__item ${isConnected ? 'sender-list__item--active' : ''}`;
        
        const iconInfo = document.createElement('div');
        iconInfo.className = 'sender-list__icon-info';
        
        const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
        svg.setAttribute("viewBox", "0 0 24 24");
        svg.setAttribute("fill", "currentColor");
        svg.setAttribute("width", "20");
        svg.setAttribute("height", "20");
        svg.setAttribute("class", "sender-list__icon");
        const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
        path.setAttribute("d", "M21 2H3c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h7v2H8v2h8v-2h-2v-2h7c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2zm0 14H3V4h18v12z");
        svg.appendChild(path);
        
        const nameSpan = document.createElement('span');
        nameSpan.className = 'sender-list__name';
        nameSpan.textContent = `${sender.deviceName} - ${sender.addr.split(':')[0]}`;
        
        iconInfo.appendChild(svg);
        iconInfo.appendChild(nameSpan);
        li.appendChild(iconInfo);
        
        const btn = document.createElement('button');
        btn.className = `sender-list__btn ${isConnected ? 'sender-list__btn--disconnect' : 'sender-list__btn--connect'}`;
        btn.textContent = isConnected ? 'Disconnect' : 'Connect';
        
        btn.addEventListener('click', async (e) => {
          e.stopPropagation();
          if (isConnected) {
            await app.disconnect();
          } else {
            // Disconnect old sender if any
            if (state.connectedSender) {
              await app.disconnect();
            }
            await app.connectToSender(sender);
          }
        });
        
        li.appendChild(btn);
        senderListEl.appendChild(li);
      });
    }
  });

  volumeSliderEl?.addEventListener('input', async () => {
    if (!volumeSliderEl) return;
    const level = parseFloat(volumeSliderEl.value) / 100;
    await app.setVolume(level);
  });

  muteBtnEl?.addEventListener('click', async () => {
    await app.toggleMute();
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
  
  listen<DiscoveredSender>('sender-discovered', (event) => {
    app.updateDiscoveredSender(event.payload);
  });
  
  listen('force-disconnect', () => {
    app.handleForceDisconnect();
  });

  app.startListening();
});
