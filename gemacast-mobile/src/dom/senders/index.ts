import { App } from '../../App';
import { AppState, AudioSource, DiscoveredSender, Status } from '../../types';
import { h } from '../utils';
import { createIcon, createInfoBlock } from './icons';
import {
  createProcessSelect,
  dropdownOpen,
  dropdownScrollTop,
  searchInputFocused,
  searchSelectionStart,
  searchSelectionEnd,
  teardownDropdownListeners,
  setDropdownOpen,
} from './process-select';

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

    if (searchInputFocused && dropdownOpen) {
      return;
    }

    teardownDropdownListeners();

    // Reset source when not connected
    if (!state.connectedSender) {
      currentSource = { type: 'desktop' };
    }

    senderListEl.innerHTML = '';

    state.discoveredSenders.forEach((sender) => {
      const isConnected = state.connectedSender?.deviceId === sender.deviceId;
      const isLoading =
        state.isLoading && (isConnected || state.status === Status.Connecting);
      const hasSource =
        isConnected &&
        (state.audioSources.length > 0 || state.processList.length > 0);
      const isAdb = sender.addr.startsWith('127.0.0.1');

      // Reset dropdown state when no longer connected
      if (!isConnected && dropdownOpen) {
        setDropdownOpen(false);
      }

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
        const select = createProcessSelect(
          app,
          state,
          currentSource,
          (src) => (currentSource = src),
        );
        li.appendChild(select.el);

        // Append to live DOM first — scrollTop only works with layout
        senderListEl.appendChild(li);

        // Now restore scroll position
        if (dropdownOpen) {
          if (dropdownScrollTop > 0) {
            select.optionsList.scrollTop = dropdownScrollTop;
          }
          if (searchInputFocused && select.searchInput) {
            select.searchInput.focus();
            select.searchInput.setSelectionRange(searchSelectionStart, searchSelectionEnd);
          }
        }
      } else {
        senderListEl.appendChild(li);
      }
    });
  });
}
