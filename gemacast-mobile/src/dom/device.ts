import { App } from '../App';
import { AppState, Status } from '../types';

function setChipStatus(
  chipEl: HTMLElement,
  labelEl: HTMLElement,
  status: Status,
  reconnectAttempts: number,
) {
  chipEl.className = chipEl.className.replace(/status-chip--\S+/g, '').trim();

  let chipClass: string;
  let label: string;

  switch (status) {
    case Status.Idle:
      chipClass = 'status-chip--idle';
      label = 'Idle';
      break;
    case Status.Listening:
      chipClass = 'status-chip--listening';
      label = 'Scanning…';
      break;
    case Status.Connecting:
      chipClass = 'status-chip--connecting';
      label = 'Connecting…';
      break;
    case Status.Connected:
      chipClass = 'status-chip--connected';
      label = 'Connected';
      break;
    case Status.Playing:
      chipClass = 'status-chip--playing';
      label = '● Playing';
      break;
    case Status.Reconnecting:
      chipClass = 'status-chip--reconnecting';
      label =
        reconnectAttempts > 0
          ? `Reconnecting (${reconnectAttempts}/5)…`
          : 'Reconnecting…';
      break;
    default:
      chipClass = 'status-chip--idle';
      label = String(status);
  }

  chipEl.classList.add('status-chip', chipClass);
  labelEl.textContent = label;
}

export function setupDeviceAndStatus(app: App) {
  const deviceNameEl = document.querySelector<HTMLSpanElement>('.device__name');
  const deviceIpEl = document.querySelector<HTMLSpanElement>('.device__ip');

  const statusChipEl = document.getElementById('status-chip');
  const statusChipLabelEl = document.getElementById('status-chip-label') as HTMLSpanElement | null;

  app.stateHandler.subscribe((state: AppState) => {
    if (deviceNameEl) deviceNameEl.textContent = state.deviceInfo.deviceName;
    if (deviceIpEl) deviceIpEl.textContent = `IP: ${state.deviceInfo.ip}`;

    if (statusChipEl && statusChipLabelEl) {
      setChipStatus(
        statusChipEl,
        statusChipLabelEl,
        state.status,
        state.reconnectAttempts,
      );
    }
  });
}