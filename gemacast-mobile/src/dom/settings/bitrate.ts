import { invoke } from '@tauri-apps/api/core';
import type { App } from '../../App';
import type { BitratePreset, AppState } from '../../types';
import { Status } from '../../types';
import { toastManager } from '../toast';

type BitrateOption = {
  id: BitratePreset;
  label: string;
  category: string;
};

const BITRATE_OPTIONS: BitrateOption[] = [
  { id: '10', label: '10 Kbps', category: 'VoIP' },
  { id: '24', label: '24 Kbps', category: 'VoIP' },
  { id: '32', label: '32 Kbps', category: 'VoIP' },
  { id: '64', label: '64 Kbps', category: 'Standard' },
  { id: '96', label: '96 Kbps', category: 'Standard' },
  { id: '128', label: '128 Kbps', category: 'High (Default)' },
  { id: '256', label: '256 Kbps', category: 'High' },
  { id: '450', label: '450 Kbps', category: 'Very High' },
  { id: '512', label: '512 Kbps', category: 'Very High' },
  { id: 'raw', label: 'Uncompressed', category: 'Raw PCM' },
  { id: 'custom', label: 'Custom', category: '' },
];

function getDisplayLabel(preset: BitratePreset, customKbps: number): string {
  if (preset === 'custom') return `Custom — ${customKbps} Kbps`;
  const opt = BITRATE_OPTIONS.find(o => o.id === preset);
  if (!opt) return '128 Kbps — High (Default)';
  return opt.category ? `${opt.label} — ${opt.category}` : opt.label;
}

export function initBitrateSettings(app: App) {
  const header = document.getElementById('custom-bitrate-header') as HTMLElement;
  const valueEl = document.getElementById('custom-bitrate-value') as HTMLElement;
  const dropdown = document.getElementById('custom-bitrate-dropdown') as HTMLElement;
  const customRow = document.getElementById('bitrate-custom-row') as HTMLElement;
  const customInput = document.getElementById('bitrate-custom-input') as HTMLInputElement;
  const customApply = document.getElementById('bitrate-custom-apply') as HTMLButtonElement;

  if (!header || !valueEl || !dropdown || !customRow || !customInput || !customApply) return;

  let isOpen = false;
  // Track the last preset to only populate the input on preset transition,
  // NOT on every state update (which would overwrite user typing).
  let lastPreset: BitratePreset | null = null;

  const toggle = () => {
    isOpen = !isOpen;
    dropdown.hidden = !isOpen;
  };

  header.addEventListener('click', toggle);

  // Close dropdown on outside click
  document.addEventListener('click', (e) => {
    const target = e.target as Node;
    const selectEl = document.getElementById('setting-bitrate');
    if (selectEl && !selectEl.contains(target)) {
      isOpen = false;
      dropdown.hidden = true;
    }
  });

  const resolveBitrate = (preset: BitratePreset, customKbps: number): number | null => {
    if (preset === 'raw') return null;
    if (preset === 'custom') return customKbps * 1000;
    return parseInt(preset, 10) * 1000;
  };

  const applyLiveBitrate = async (preset: BitratePreset, customKbps: number) => {
    const state = app.stateHandler.getState();
    if ((state.status === Status.Connected || state.status === Status.Playing) && state.connectedSender) {
      const bitrate = resolveBitrate(preset, customKbps);
      const ip = state.connectedSender.addr.split(':')[0];
      try {
        await invoke('change_audio_bitrate', {
          ip,
          deviceId: state.deviceInfo.deviceId,
          bitrate
        });
      } catch (e) {
        console.error('Failed to change live bitrate', e);
      }
    }
  };

  const selectPreset = (id: BitratePreset) => {
    const state = app.stateHandler.getState();
    app.stateHandler.setState({
      settings: { ...state.settings, bitratePreset: id },
    });
    isOpen = false;
    dropdown.hidden = true;
    if (id !== 'custom') {
      const label = getDisplayLabel(id, state.settings.customBitrateKbps);
      toastManager.showSuccess(`Audio quality: ${label}`);
      applyLiveBitrate(id, state.settings.customBitrateKbps);
    }
  };

  // Validate and update Apply button state
  const validateCustomInput = () => {
    const raw = customInput.value.trim();
    const val = parseInt(raw, 10);
    const isValid = raw !== '' && !isNaN(val) && val >= 6 && val <= 512;
    customApply.disabled = !isValid;
  };

  // Block non-numeric input
  customInput.addEventListener('keydown', (e) => {
    const allowed = ['Backspace', 'Delete', 'Tab', 'Escape', 'Enter',
      'ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End'];
    if (allowed.includes(e.key)) return;
    if ((e.ctrlKey || e.metaKey) && ['a', 'c', 'v', 'x'].includes(e.key.toLowerCase())) return;
    if (!/^\d$/.test(e.key)) {
      e.preventDefault();
    }
  });

  customInput.addEventListener('input', validateCustomInput);

  customApply.addEventListener('click', () => {
    const val = parseInt(customInput.value, 10);
    if (isNaN(val) || val < 6 || val > 512) {
      toastManager.showWarning('Bitrate must be between 6 and 512 Kbps');
      return;
    }
    const state = app.stateHandler.getState();
    app.stateHandler.setState({
      settings: { ...state.settings, customBitrateKbps: val, bitratePreset: 'custom' },
    });
    toastManager.showSuccess(`Custom bitrate: ${val} Kbps`);
    applyLiveBitrate('custom', val);
  });

  app.stateHandler.subscribe((state: AppState) => {
    const preset = state.settings.bitratePreset;
    valueEl.textContent = getDisplayLabel(preset, state.settings.customBitrateKbps);

    // Show/hide custom row
    const wasCustom = lastPreset === 'custom';
    const isCustom = preset === 'custom';
    customRow.hidden = !isCustom;

    // Only populate the input when transitioning INTO custom mode,
    // not on every state update (which would clobber user typing).
    if (isCustom && !wasCustom) {
      customInput.value = String(state.settings.customBitrateKbps);
      validateCustomInput();
    }

    lastPreset = preset;

    // Rebuild dropdown options
    dropdown.innerHTML = '';
    for (const opt of BITRATE_OPTIONS) {
      const optionEl = document.createElement('div');
      optionEl.className = 'custom-select__option';
      if (opt.id === preset) {
        optionEl.classList.add('custom-select__option--selected');
      }

      const title = document.createElement('span');
      title.className = 'custom-select__option-title';
      title.textContent = opt.category
        ? `${opt.label} — ${opt.category}`
        : opt.label;
      optionEl.appendChild(title);

      optionEl.addEventListener('click', () => selectPreset(opt.id));
      dropdown.appendChild(optionEl);

      // Separators between category groups
      if (opt.id === '32' || opt.id === '96' || opt.id === '256' || opt.id === '512') {
        const sep = document.createElement('div');
        sep.className = 'custom-select__separator';
        dropdown.appendChild(sep);
      }
    }
  });
}
