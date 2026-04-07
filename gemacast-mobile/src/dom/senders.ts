import { App } from '../App';
import { AppState, Status } from '../types';

export function setupSenderList(app: App) {
  const senderListEl = document.getElementById('sender-list');
  const senderEmptyEl = document.getElementById('sender-empty-state');

  app.stateHandler.subscribe((state: AppState) => {
    if (!senderListEl) return;

    const isListening =
      state.status === Status.Listening ||
      state.status === Status.Connecting ||
      state.status === Status.Reconnecting ||
      state.status === Status.Connected ||
      state.status === Status.Playing;

    const isEmpty = state.discoveredSenders.length === 0 && isListening;
    if (senderEmptyEl) senderEmptyEl.hidden = !isEmpty;

    senderListEl.innerHTML = '';
    state.discoveredSenders.forEach((sender) => {
      const isConnected = state.connectedSender?.deviceId === sender.deviceId;
      const isLoading =
        state.isLoading &&
        (isConnected || state.status === Status.Connecting);

      const li = document.createElement('li');
      li.className = `sender-list__item${isConnected ? ' sender-list__item--active' : ''}`;

      const iconInfo = document.createElement('div');
      iconInfo.className = 'sender-list__icon-info';

      const svg = document.createElementNS(
        'http://www.w3.org/2000/svg',
        'svg',
      );
      svg.setAttribute('viewBox', '0 0 24 24');
      svg.setAttribute('fill', 'currentColor');
      svg.setAttribute('width', '20');
      svg.setAttribute('height', '20');
      svg.setAttribute('class', 'sender-list__icon');
      svg.setAttribute('aria-hidden', 'true');
      const path = document.createElementNS(
        'http://www.w3.org/2000/svg',
        'path',
      );
      path.setAttribute(
        'd',
        'M21 2H3c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h7v2H8v2h8v-2h-2v-2h7c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2zm0 14H3V4h18v12z',
      );
      svg.appendChild(path);

      const infoBlock = document.createElement('div');
      infoBlock.className = 'sender-list__info';

      const nameSpan = document.createElement('span');
      nameSpan.className = 'sender-list__name';
      nameSpan.textContent = sender.deviceName;

      const ipWrap = document.createElement('div');
      ipWrap.className = 'sender-list__ip-wrap';

      const ipSpan = document.createElement('span');
      ipSpan.className = 'sender-list__ip';
      ipSpan.textContent = sender.addr.split(':')[0];

      ipWrap.appendChild(ipSpan);
      infoBlock.appendChild(nameSpan);
      infoBlock.appendChild(ipWrap);

      iconInfo.appendChild(svg);
      iconInfo.appendChild(infoBlock);
      li.appendChild(iconInfo);

      const btn = document.createElement('button');
      const isDisconnectBtn = isConnected;
      btn.className = [
        'sender-list__btn',
        isDisconnectBtn
          ? 'sender-list__btn--disconnect'
          : 'sender-list__btn--connect',
        isLoading ? 'sender-list__btn--loading' : '',
      ]
        .filter(Boolean)
        .join(' ');
      btn.textContent = isLoading
        ? ''
        : isDisconnectBtn
          ? 'Disconnect'
          : 'Connect';
      btn.disabled = state.isLoading;
      btn.setAttribute(
        'aria-label',
        isDisconnectBtn
          ? `Disconnect from ${sender.deviceName}`
          : `Connect to ${sender.deviceName}`,
      );

      btn.addEventListener('click', async (e) => {
        e.stopPropagation();
        if (isDisconnectBtn) {
          await app.connection.disconnect();
        } else {
          if (state.connectedSender) {
            await app.connection.disconnect();
          }
          await app.connection.connectToSender(sender);
        }
      });

      li.appendChild(btn);
      senderListEl.appendChild(li);
    });
  });
}