import { App } from '../App';
import { AppState, AudioSource, DiscoveredSender, Status } from '../types';
import { h } from './utils';

function sourceLabel(source: AudioSource): string {
  if (source.type === 'desktop') return '🖥️ Desktop Audio';
  return `🎵 ${source.name} (PID ${source.pid})`;
}

function sourcesEqual(a: AudioSource, b: AudioSource): boolean {
  if (a.type !== b.type) return false;
  if (a.type === 'desktop') return true;
  return a.type === 'process' && b.type === 'process' && a.pid === b.pid;
}

function createIcon(isAdb: boolean) {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('fill', 'currentColor');
  svg.setAttribute('width', '20');
  svg.setAttribute('height', '20');
  svg.setAttribute('class', 'sender-list__icon');
  svg.setAttribute('aria-hidden', 'true');

  if (isAdb) {
    svg.innerHTML =
      '<path d="M15 7v4h1v2h-3V5h2l-3-4-3 4h2v8H8v-2.07c.7-.37 1.2-1.08 1.2-1.93 0-1.21-.99-2.2-2.2-2.2-1.21 0-2.2.99-2.2 2.2 0 .85.5 1.56 1.2 1.93V13c0 1.11.89 2 2 2h3v3.05c-.71.37-1.2 1.08-1.2 1.95 0 1.21.99 2.2 2.2 2.2 1.21 0 2.2-.99 2.2-2.2 0-.87-.49-1.58-1.2-1.95V15h3c1.11 0 2-.89 2-2v-2h1V7h-4z"/>';
  } else {
    const path = document.createElementNS('http://www.w3.org/2000/svg', 'path');
    path.setAttribute(
      'd',
      'M21 2H3c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h7v2H8v2h8v-2h-2v-2h7c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2zm0 14H3V4h18v12z',
    );
    svg.appendChild(path);
  }
  return svg;
}

function createInfoBlock(sender: DiscoveredSender, isAdb: boolean) {
  return h(
    'div',
    { className: 'sender-list__info' },
    h('span', {
      className: 'sender-list__name',
      textContent: sender.deviceName,
    }),
    h(
      'div',
      { className: 'sender-list__ip-wrap' },
      h('span', {
        className: 'sender-list__ip',
        textContent: isAdb ? 'Direct USB Cable' : sender.addr.split(':')[0],
      }),
    ),
  );
}

function createConnectButton(
  app: App,
  sender: DiscoveredSender,
  isConnected: boolean,
  isLoading: boolean,
  connectedSender: DiscoveredSender | null,
) {
  const action = isConnected ? 'disconnect' : 'connect';

  return h('button', {
    className:
      `sender-list__btn sender-list__btn--${action} ${isLoading ? 'sender-list__btn--loading' : ''}`.trim(),
    disabled: isLoading,
    ariaLabel: `${isConnected ? 'Disconnect from' : 'Connect to'} ${sender.deviceName}`,
    textContent: isLoading ? '' : isConnected ? 'Disconnect' : 'Connect',
    onClick: async (e) => {
      e.stopPropagation();
      if (isConnected) {
        await app.connection.disconnect();
      } else {
        if (connectedSender) {
          await app.connection.disconnect();
        }
        await app.connection.connectToSender(sender);
      }
    },
  });
}

function createSourceSelect(
  app: App,
  state: AppState,
  currentSource: AudioSource,
  onSourceChange: (source: AudioSource) => void,
) {
  const caps = state.senderCapabilities;
  const onlyDesktop =
    !caps?.supportsProcessCapture && state.audioSources.length <= 1;

  const select = h(
    'select',
    {
      className: 'source-select__dropdown',
      id: 'source-select-dropdown',
      ariaLabel: 'Audio source',
      disabled: onlyDesktop,
      onChange: async (e) => {
        const target = e.target as HTMLSelectElement;
        const selectedIdx = parseInt(target.value, 10);
        const selected = state.audioSources[selectedIdx];
        if (selected) {
          onSourceChange(selected);
          await app.connection.changeAudioSource(selected);
        }
      },
    },
    ...state.audioSources.map((source, idx) =>
      h('option', {
        value: String(idx),
        textContent: sourceLabel(source),
        selected: sourcesEqual(source, currentSource),
      }),
    ),
  );

  return h(
    'div',
    { className: 'source-select' },
    h('span', { className: 'source-select__label', textContent: 'Source:' }),
    select,
  );
}

export function setupSenderList(app: App) {
  const senderListEl = document.getElementById('sender-list');
  const senderEmptyEl = document.getElementById('sender-empty-state');

  let currentSource: AudioSource = { type: 'desktop' };

  app.stateHandler.subscribe((state: AppState) => {
    if (!senderListEl) return;

    const isListening = [
      Status.Listening,
      Status.Connecting,
      Status.Reconnecting,
      Status.Connected,
      Status.Playing,
    ].includes(state.status);

    const isEmpty = state.discoveredSenders.length === 0 && isListening;
    if (senderEmptyEl) senderEmptyEl.hidden = !isEmpty;

    senderListEl.innerHTML = '';

    state.discoveredSenders.forEach((sender) => {
      const isConnected = state.connectedSender?.deviceId === sender.deviceId;
      const isLoading =
        state.isLoading && (isConnected || state.status === Status.Connecting);
      const hasSource = isConnected && state.audioSources.length > 0;
      const isAdb = sender.addr.startsWith('127.0.0.1');

      const li = h(
        'li',
        {
          className:
            `sender-list__item ${isConnected ? 'sender-list__item--active' : ''} ${hasSource ? 'sender-list__item--has-source' : ''}`.trim(),
        },
        h(
          'div',
          { className: 'sender-list__icon-info' },
          createIcon(isAdb),
          createInfoBlock(sender, isAdb),
        ),
        createConnectButton(
          app,
          sender,
          isConnected,
          isLoading,
          state.connectedSender,
        ),
      );

      if (hasSource) {
        li.appendChild(
          createSourceSelect(
            app,
            state,
            currentSource,
            (src) => (currentSource = src),
          ),
        );
      }

      senderListEl.appendChild(li);
    });
  });
}
