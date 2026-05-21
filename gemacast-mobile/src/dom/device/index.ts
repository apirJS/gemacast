import { App } from '../../App';
import { AppState, Status } from '../../types';
import { h } from '../utils';

function getStatusDetails(status: Status, attempts: number) {
  switch (status) {
    case Status.Idle:
      return { class: 'status-chip--idle', label: 'Idle' };
    case Status.Listening:
      return { class: 'status-chip--listening', label: 'Scanning…' };
    case Status.Connecting:
      return { class: 'status-chip--connecting', label: 'Connecting…' };
    case Status.Connected:
      return { class: 'status-chip--connected', label: 'Connected' };
    case Status.Playing:
      return { class: 'status-chip--playing', label: '● Playing' };
    case Status.Reconnecting:
      return {
        class: 'status-chip--reconnecting',
        label: attempts > 0 ? `Reconnecting (${attempts}/5)…` : 'Reconnecting…',
      };
    default:
      return { class: 'status-chip--idle', label: String(status) };
  }
}

export function setupDeviceAndStatus(app: App) {
  const deviceSection = document.querySelector('.device') as HTMLElement;
  const infoSection = document.querySelector('.info') as HTMLElement;

  const statusContainer = h('div', {});
  const existingChip = document.getElementById('status-chip');
  if (existingChip) {
    existingChip.replaceWith(statusContainer);
  } else {
    infoSection.insertBefore(statusContainer, infoSection.firstChild);
  }

  app.stateHandler.subscribe((state: AppState) => {
    deviceSection.innerHTML = '';
    deviceSection.appendChild(
      h('span', {
        className: 'device__name',
        textContent: state.deviceInfo.deviceName,
      }),
    );
    deviceSection.appendChild(
      h('span', {
        className: 'device__ip',
        textContent: `IP: ${state.deviceInfo.ip}`,
      }),
    );

    const details = getStatusDetails(state.status, state.reconnectAttempts);
    const chip = h(
      'div',
      {
        className: `status-chip ${details.class}`,
        id: 'status-chip',
        role: 'status',
        ariaLive: 'polite',
      },
      h('span', { className: 'status-chip__dot', ariaHidden: 'true' }),
      h('span', {
        className: 'status-chip__label',
        id: 'status-chip-label',
        textContent: details.label,
      }),
    );

    statusContainer.innerHTML = '';
    statusContainer.appendChild(chip);
  });
}
