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
import { getRenderHash, setRenderHash } from './render-state';

function createConnectButton(
  app: App,
  sender: DiscoveredSender,
  isConnected: boolean,
  isLoading: boolean,
  isDisabled: boolean,
  connectedSender: DiscoveredSender | null,
) {
  const action = isConnected ? 'disconnect' : 'connect';
  const isManual = sender.deviceId.startsWith('manual-');

  return h('button', {
    className:
      `sender-list__btn sender-list__btn--${action} ${isLoading ? 'sender-list__btn--loading' : ''}`.trim(),
    disabled: isDisabled,
    ariaLabel: `${isConnected ? 'Disconnect from' : 'Connect to'} ${sender.deviceName}`,
    textContent: isLoading ? '' : isConnected ? 'Disconnect' : 'Connect',
    onClick: async (e) => {
      e.stopPropagation();
      if (isConnected) {
        await app.connection.disconnect();
        // Remove manual senders from the list on disconnect
        if (isManual) {
          const currentState = app.stateHandler.getState();
          const newList = currentState.discoveredSenders.filter(s => s.deviceId !== sender.deviceId);
          app.stateHandler.setState({ discoveredSenders: newList });
        }
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

    if (dropdownOpen) {
      return;
    }

    teardownDropdownListeners();

    if (!state.connectedSender) {
      currentSource = { type: 'desktop' };
      if (dropdownOpen) {
        setDropdownOpen(false);
      }
    }

    const renderHash = JSON.stringify({
      senders: state.discoveredSenders,
      connected: state.connectedSender?.deviceId,
      isConnecting: state.status === Status.Connecting,
      loading: state.isLoading,
      sources: state.audioSources,
      procs: state.processList,
      connectingId: state.connectingSenderId,
    });

    if (renderHash === getRenderHash()) {
      return;
    }
    setRenderHash(renderHash);

    senderListEl.innerHTML = '';

    state.discoveredSenders.forEach((sender) => {
      const isConnected = state.connectedSender?.deviceId === sender.deviceId;
      const isConnectingToThis = state.status === Status.Connecting && state.connectingSenderId === sender.deviceId;
      const isLoading = state.isLoading && (isConnected || isConnectingToThis);
      const isDisabled = state.isLoading || state.status === Status.Connecting;
      const hasSource =
        isConnected &&
        (state.audioSources.length > 0 || state.processList.length > 0);
      const isAdb = sender.addr.startsWith('127.0.0.1');
      const isManual = sender.deviceId.startsWith('manual-');

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
          isDisabled,
          state.connectedSender,
        ),
      );

      if (isManual) {
        let startX = 0;
        let currentX = 0;
        let hasMoved = false;
        let isSwiping = false;

        li.addEventListener('touchstart', (e) => {
          // Don't allow swiping if the touch originates inside the dropdown
          const target = e.target as Element;
          if (target.closest('.process-select')) {
            isSwiping = false;
            return;
          }
          startX = e.touches[0].clientX;
          currentX = startX;
          hasMoved = false;
          isSwiping = true;
          li.style.transition = 'none';
        }, { passive: true });

        li.addEventListener('touchmove', (e) => {
          if (!isSwiping) return;
          currentX = e.touches[0].clientX;
          const deltaX = currentX - startX;

          if (Math.abs(deltaX) > 10) {
            hasMoved = true;
          }

          if (hasMoved) {
            li.style.transform = `translateX(${deltaX}px)`;
            if (Math.abs(deltaX) > 50) {
              li.classList.add('sender-list__item--swiping');
            }
          }
        }, { passive: true });

        li.addEventListener('touchend', () => {
          if (!isSwiping) return;
          isSwiping = false;

          if (!hasMoved) {
            li.style.transition = '';
            return;
          }

          const deltaX = currentX - startX;
          const threshold = li.offsetWidth * 0.4;

          li.style.transition = 'transform 0.3s ease, opacity 0.3s ease';

          if (Math.abs(deltaX) > threshold && Math.abs(deltaX) > 50) {
            li.style.transform = `translateX(${deltaX > 0 ? 100 : -100}%)`;
            li.style.opacity = '0';
            setTimeout(async () => {
              // Disconnect first if this sender is currently connected
              const currentState = app.stateHandler.getState();
              if (currentState.connectedSender?.deviceId === sender.deviceId) {
                await app.connection.disconnect();
              }
              const freshState = app.stateHandler.getState();
              const newList = freshState.discoveredSenders.filter(s => s.deviceId !== sender.deviceId);
              app.stateHandler.setState({ discoveredSenders: newList });
            }, 300);
          } else {
            li.style.transform = '';
            li.classList.remove('sender-list__item--swiping');
          }
        });
      }

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
