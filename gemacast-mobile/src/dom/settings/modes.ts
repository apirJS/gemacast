import { AppState, ConnectionMode, Status } from '../../types';
import type { App } from '../../App';

export function initModes(app: App) {
  const stateHandler = app.stateHandler;
  const excMode = document.getElementById('setting-exclusive-mode') as HTMLInputElement;
  const modes = document.getElementsByName('conn-mode') as NodeListOf<HTMLInputElement>;

  excMode.addEventListener('change', () => {
    const state = stateHandler.getState();
    stateHandler.setState({
      settings: { ...state.settings, exclusiveMode: excMode.checked },
    });
    if (state.connectedSender && state.status === Status.Playing) {
      app.connection.disconnect(false);
    }
  });

  const updateModes = () => {
    const currSettings = stateHandler.getState().settings;
    let nextMode = currSettings.mode;
    modes.forEach((m: HTMLInputElement) => {
      if (m.checked && !m.disabled) nextMode = m.value as ConnectionMode;
    });

    stateHandler.setState({
      settings: {
        ...currSettings,
        exclusiveMode: excMode.checked,
        mode: nextMode,
      },
    });
  };

  modes.forEach((m: HTMLInputElement) =>
    m.addEventListener('change', updateModes)
  );

  stateHandler.subscribe((state: AppState) => {
    const s = state.settings;
    excMode.checked = s.exclusiveMode;

    modes.forEach((m: HTMLInputElement) => {
      if (m.value === s.mode) m.checked = true;

      const isWifi = m.value === ConnectionMode.Wifi;
      const isUsb = m.value === ConnectionMode.Usb;
      const isAdb = m.value === ConnectionMode.Adb;

      if (isWifi) {
        m.disabled = !state.availableModes.wifi;
      } else if (isUsb) {
        m.disabled = !state.availableModes.usb;
      } else if (isAdb) {
        m.disabled = !state.availableModes.adb;
      }

      const label = m.closest('label');
      if (label) {
        if (m.disabled) label.classList.add('mode-btn--disabled');
        else label.classList.remove('mode-btn--disabled');
      }
    });
  });
}
