import { AppState, ConnectionMode, Status } from '../../types';
import type { App } from '../../App';
import { h } from '../utils';
import { toastManager } from '../toast';

export function initModes(app: App) {
  const stateHandler = app.stateHandler;
  
  const connWrapper = document.querySelector('input[name="conn-mode"]')?.closest('.segmented-control') as HTMLElement | null;
  const excWrapper = document.getElementById('setting-exclusive-mode')?.closest('.toggle-switch') as HTMLElement | null;

  if (!connWrapper || !excWrapper) return;

  const excModeInput = h('input', {
    type: 'checkbox',
    id: 'setting-exclusive-mode',
    onChange: () => {
      const state = stateHandler.getState();
      stateHandler.setState({
        settings: { ...state.settings, exclusiveMode: excModeInput.checked },
      });
      toastManager.showSuccess(`Exclusive mode ${excModeInput.checked ? 'enabled' : 'disabled'}`);
      if (state.connectedSender && state.status === Status.Playing) {
        app.connection.disconnect(false);
      }
    }
  });

  excWrapper.innerHTML = '';
  excWrapper.appendChild(excModeInput);
  excWrapper.appendChild(h('div', { className: 'toggle-switch__slider' },
    h('span', { className: 'toggle-switch__label--off', textContent: 'OFF' }),
    h('span', { className: 'toggle-switch__label--on', textContent: 'ON' })
  ));

  const updateModes = (val: ConnectionMode) => {
    const currSettings = stateHandler.getState().settings;
    if (currSettings.mode !== val) {
      stateHandler.setState({
        settings: { ...currSettings, exclusiveMode: excModeInput.checked, mode: val },
      });
      toastManager.showSuccess(`Connection mode set to ${val.toUpperCase()}`);
    }
  };

  const createModeRadio = (val: ConnectionMode, labelText: string, state: AppState) => {
    let disabled = true;
    if (val === ConnectionMode.Wifi) disabled = !state.availableModes.wifi;
    if (val === ConnectionMode.Usb) disabled = !state.availableModes.usb;
    if (val === ConnectionMode.Adb) disabled = !state.availableModes.adb;

    const id = `conn-mode-${val}`;
    const input = h('input', {
      type: 'radio',
      name: 'conn-mode',
      id,
      value: val,
      checked: state.settings.mode === val,
      disabled,
      onChange: () => updateModes(val)
    });

    const label = h('label', { 
      htmlFor: id, 
      textContent: labelText,
      className: disabled ? 'mode-btn--disabled' : ''
    });

    return { input, label };
  };

  app.stateHandler.subscribe((state: AppState) => {
    excModeInput.checked = state.settings.exclusiveMode;

    connWrapper.innerHTML = '';
    const wifi = createModeRadio(ConnectionMode.Wifi, 'Wifi', state);
    const usb = createModeRadio(ConnectionMode.Usb, 'USB', state);
    const adb = createModeRadio(ConnectionMode.Adb, 'ADB', state);

    connWrapper.appendChild(wifi.input);
    connWrapper.appendChild(wifi.label);
    connWrapper.appendChild(usb.input);
    connWrapper.appendChild(usb.label);
    connWrapper.appendChild(adb.input);
    connWrapper.appendChild(adb.label);
  });
}
