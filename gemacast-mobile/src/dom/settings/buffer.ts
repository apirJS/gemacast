import { invoke } from '@tauri-apps/api/core';
import { AppState, PresetId, SavedPreset, JitterConfig } from '../../types';
import { JITTER_PRESETS, getPresetConfig } from '../../core/presets';
import type { App } from '../../App';

export function initBufferSettings(app: App) {
  const stateHandler = app.stateHandler;

  const presetContainer = document.getElementById('setting-preset') as HTMLElement;
  const presetHeader = document.getElementById('custom-preset-header') as HTMLElement;
  const presetValue = document.getElementById('custom-preset-value') as HTMLElement;
  const presetDropdown = document.getElementById('custom-preset-dropdown') as HTMLElement;
  let currentPresetValue: PresetId = 'balanced';
  const customConfig = document.getElementById('custom-jitter-config') as HTMLDivElement;

  const presetNameInput = document.getElementById('setting-preset-name') as HTMLInputElement;
  const staticFields = document.getElementById('static-fields') as HTMLDivElement;
  const adaptiveFields = document.getElementById('adaptive-fields') as HTMLDivElement;
  const staticDepth = document.getElementById('setting-static-depth') as HTMLInputElement;
  const bufferModeRadios = document.getElementsByName('buffer-mode') as NodeListOf<HTMLInputElement>;
  const customDeleteBtn = document.getElementById('custom-delete-btn') as HTMLButtonElement;
  const deleteConfirmDialog = document.getElementById('delete-confirm-dialog') as HTMLDialogElement;
  const deleteConfirmMsg = document.getElementById('delete-confirm-msg') as HTMLElement;
  const deleteConfirmCancel = document.getElementById('delete-confirm-cancel') as HTMLButtonElement;
  const deleteConfirmOk = document.getElementById('delete-confirm-ok') as HTMLButtonElement;

  let activeSavedPresetName: string | null = null;
  let customApplied = false;

  const minDepth = document.getElementById('setting-min-depth') as HTMLInputElement;
  const comfortCap = document.getElementById('setting-comfort-cap') as HTMLInputElement;
  const bounce = document.getElementById('setting-bounce') as HTMLInputElement;
  const resume = document.getElementById('setting-resume') as HTMLInputElement;

  const customApplyBtn = document.getElementById('custom-apply-btn') as HTMLButtonElement;
  const customResetBtn = document.getElementById('custom-reset-btn') as HTMLButtonElement;

  const validateSaveBtn = () => {
    const name = presetNameInput.value.trim();
    customApplyBtn.disabled = !name;
  };
  presetNameInput.addEventListener('input', validateSaveBtn);

  const syncBufferModeUI = () => {
    const checked = document.querySelector('input[name="buffer-mode"]:checked') as HTMLInputElement;
    const isStatic = checked?.value === 'static';
    staticFields.hidden = !isStatic;
    adaptiveFields.hidden = isStatic;
    validateSaveBtn();
  };
  bufferModeRadios.forEach((r) => r.addEventListener('change', () => {
    syncBufferModeUI();
    if (stateHandler.getState().settings.bufferPreset === 'custom') {
      applyLiveCustomConfig();
    }
  }));
  syncBufferModeUI();

  const renderSavedPresets = (savedPresets: SavedPreset[]) => {
    document.querySelectorAll('.custom-select__option--saved').forEach((el) => el.remove());

    savedPresets.forEach((sp) => {
      const opt = document.createElement('div');
      opt.className = 'custom-select__option custom-select__option--saved';
      opt.dataset.savedName = sp.name;
      const title = document.createElement('div');
      title.className = 'custom-select__option-title';
      title.textContent = sp.name;
      opt.appendChild(title);
      const desc = document.createElement('div');
      desc.className = 'custom-select__option-desc';
      desc.textContent = sp.config.staticTargetMs != null ? `Static: ${sp.config.staticTargetMs}ms` : 'Adaptive';
      opt.appendChild(desc);
      
      opt.addEventListener('click', () => {
        activeSavedPresetName = sp.name;
        currentPresetValue = 'custom';
        presetDropdown.hidden = true;
        presetValue.textContent = sp.name;
        stateHandler.setState({
          settings: {
            ...stateHandler.getState().settings,
            bufferPreset: 'custom',
            customJitterConfig: sp.config,
          },
        });
        invoke('update_jitter_config', { jitterConfig: sp.config }).catch(console.warn);

        if (sp.config.staticTargetMs != null) {
          (document.getElementById('buffer-mode-static') as HTMLInputElement).checked = true;
        } else {
          (document.getElementById('buffer-mode-adaptive') as HTMLInputElement).checked = true;
        }
        syncBufferModeUI();
        presetNameInput.value = sp.name;
        customApplied = true;
      });
      presetDropdown.appendChild(opt);
    });
  };

  customDeleteBtn.addEventListener('click', () => {
    if (!activeSavedPresetName) return;
    deleteConfirmMsg.textContent = `Delete "${activeSavedPresetName}"?`;
    deleteConfirmDialog.showModal();
  });
  deleteConfirmCancel.addEventListener('click', () => deleteConfirmDialog.close());
  deleteConfirmOk.addEventListener('click', () => {
    deleteConfirmDialog.close();
    if (!activeSavedPresetName) return;
    const curr = stateHandler.getState().settings;
    const next = (curr.savedPresets ?? []).filter((p) => p.name !== activeSavedPresetName);
    activeSavedPresetName = null;
    currentPresetValue = 'auto';
    stateHandler.setState({
      settings: { ...curr, savedPresets: next, bufferPreset: 'auto' },
    });
    const autoConfig = getPresetConfig('auto', curr.customJitterConfig);
    invoke('update_jitter_config', { jitterConfig: autoConfig }).catch(console.warn);
  });
  deleteConfirmDialog.addEventListener('click', (e) => {
    if (e.target === deleteConfirmDialog) deleteConfirmDialog.close();
  });

  presetDropdown.innerHTML = '';
  const presetOptionsList: HTMLElement[] = [];
  JITTER_PRESETS.forEach((preset) => {
    const opt = document.createElement('div');
    opt.className = 'custom-select__option';
    const title = document.createElement('div');
    title.className = 'custom-select__option-title';
    title.textContent = preset.name;
    opt.appendChild(title);

    if (preset.id !== 'custom' && preset.description) {
      const desc = document.createElement('div');
      desc.className = 'custom-select__option-desc';
      desc.textContent = preset.description;
      opt.appendChild(desc);
    }

    opt.addEventListener('click', () => {
      currentPresetValue = preset.id;
      activeSavedPresetName = null;
      presetDropdown.hidden = true;
      updateState();
    });
    presetOptionsList.push(opt);
    presetDropdown.appendChild(opt);
  });

  presetHeader.addEventListener('click', () => {
    presetDropdown.hidden = !presetDropdown.hidden;
  });

  document.addEventListener('click', (e) => {
    if (!presetContainer.contains(e.target as Node)) {
      presetDropdown.hidden = true;
    }
  });

  let lastNonCustomPreset: PresetId = stateHandler.getState().settings.bufferPreset;
  if (lastNonCustomPreset === 'custom') lastNonCustomPreset = 'balanced';

  const updateState = () => {
    const presetIdx = currentPresetValue;
    const currSettings = stateHandler.getState().settings;
    let nextCustom = currSettings.customJitterConfig;

    if (presetIdx !== 'custom') {
      lastNonCustomPreset = presetIdx;
    } else if (presetIdx === 'custom' && currSettings.bufferPreset !== 'custom') {
      if (!customApplied) {
        nextCustom = getPresetConfig('auto', nextCustom);
      }
    }

    stateHandler.setState({
      settings: {
        ...currSettings,
        bufferPreset: presetIdx,
        customJitterConfig: nextCustom,
      },
    });

    if (presetIdx !== 'custom') {
      const activeConfig = getPresetConfig(presetIdx, nextCustom);
      
      invoke('update_jitter_config', { jitterConfig: activeConfig }).catch(console.warn);
    }
  };

  const applyLiveCustomConfig = () => {
    const currSettings = stateHandler.getState().settings;
    const isStatic = (document.querySelector('input[name="buffer-mode"]:checked') as HTMLInputElement)?.value === 'static';

    let custom: JitterConfig;
    if (isStatic) {
      const depthMs = parseInt(staticDepth.value, 10) || 60;
      custom = {
        minDepthMs: depthMs,
        comfortCapMs: depthMs + 100,
        peakDecayHalflifeMs: 0,
        resumeThresholdPct: 0.5,
        staticTargetMs: depthMs,
      };
    } else {
      custom = {
        minDepthMs: parseInt(minDepth.value, 10) || 0,
        comfortCapMs: parseInt(comfortCap.value, 10) || 0,
        peakDecayHalflifeMs: parseFloat(bounce.value) || 0,
        resumeThresholdPct: (parseFloat(resume.value) || 0) / 100.0,
        staticTargetMs: null,
      };
    }

    customApplied = true;

    if (activeSavedPresetName !== null) {
      activeSavedPresetName = null;
      presetValue.textContent = 'Custom';
      document.querySelectorAll('.custom-select__option--saved').forEach(el => el.classList.remove('custom-select__option--selected'));
    }

    stateHandler.setState({
      settings: {
        ...currSettings,
        customJitterConfig: custom,
        bufferPreset: 'custom',
      },
    });

    
    invoke('update_jitter_config', { jitterConfig: custom }).catch(console.warn);
  };

  const liveInputs = [minDepth, comfortCap, bounce, resume, staticDepth];
  liveInputs.forEach(input => input.addEventListener('input', applyLiveCustomConfig));

  customApplyBtn.addEventListener('click', () => {
    const currSettings = stateHandler.getState().settings;
    const name = presetNameInput.value.trim();
    if (!name) return;

    const custom = currSettings.customJitterConfig;
    const prevSaved = currSettings.savedPresets ?? [];
    const filteredSaved = prevSaved.filter((p) => p.name.toLowerCase() !== name.toLowerCase());
    const savedPresets = [...filteredSaved, { name, config: custom }];

    activeSavedPresetName = name;
    stateHandler.setState({
      settings: {
        ...currSettings,
        savedPresets,
        bufferPreset: 'custom',
      },
    });
    presetNameInput.value = name;
    validateSaveBtn();
  });

  customResetBtn.addEventListener('click', () => {
    customApplied = false;
    const currSettings = stateHandler.getState().settings;
    const fromPreset = getPresetConfig(lastNonCustomPreset, currSettings.customJitterConfig);
    stateHandler.setState({
      settings: { ...currSettings, customJitterConfig: fromPreset },
    });
  });

  stateHandler.subscribe((state: AppState) => {
    const s = state.settings;
    customConfig.hidden = s.bufferPreset !== 'custom';

    customDeleteBtn.hidden = !activeSavedPresetName;

    if (activeSavedPresetName && s.bufferPreset === 'custom') {
      presetValue.textContent = activeSavedPresetName;
    } else {
      const activePresetDef = JITTER_PRESETS.find((p) => p.id === s.bufferPreset);
      presetValue.textContent = activePresetDef?.name || 'Custom';
      if (s.bufferPreset !== 'custom') activeSavedPresetName = null;
    }

    presetOptionsList.forEach((opt, i) => {
      const isSelected = JITTER_PRESETS[i].id === s.bufferPreset && !activeSavedPresetName;
      if (isSelected) opt.classList.add('custom-select__option--selected');
      else opt.classList.remove('custom-select__option--selected');
    });

    document.querySelectorAll<HTMLElement>('.custom-select__option--saved').forEach((el) => {
      const isSel = el.dataset.savedName === activeSavedPresetName;
      if (isSel) el.classList.add('custom-select__option--selected');
      else el.classList.remove('custom-select__option--selected');
    });

    if (!(window as any).__lastJitterConfig) {
      (window as any).__lastJitterConfig = JSON.stringify(s.customJitterConfig);
    }
    const currentJitterString = JSON.stringify(s.customJitterConfig);

    if ((window as any).__lastJitterConfig !== currentJitterString) {
      (window as any).__lastJitterConfig = currentJitterString;

      if (document.activeElement !== staticDepth) {
        staticDepth.value = (s.customJitterConfig.staticTargetMs ?? 60).toString();
      }

      if (document.activeElement !== minDepth)
        minDepth.value = s.customJitterConfig.minDepthMs.toString();
      if (document.activeElement !== comfortCap)
        comfortCap.value = s.customJitterConfig.comfortCapMs.toString();
      if (document.activeElement !== bounce) {
        const val = s.customJitterConfig.peakDecayHalflifeMs ?? (s.customJitterConfig as any).bounceMultiplier ?? 0;
        bounce.value = val.toString();
      }
      if (document.activeElement !== resume)
        resume.value = (s.customJitterConfig.resumeThresholdPct * 100).toString();
    }

    renderSavedPresets(s.savedPresets ?? []);
    validateSaveBtn();
  });
}
