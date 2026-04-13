import { AppState, ConnectionMode, Status } from '../types';
import { JITTER_PRESETS, PRESET_DESCRIPTIONS, getPresetConfig } from '../core/presets';
import { invoke } from '@tauri-apps/api/core';
import type { App } from '../App';

const HELP_TEXTS: Record<string, { title: string; body: string }> = {
  'buffer-preset': {
    title: 'Buffer Preset',
    body: `Presets control how much audio is buffered before playback — a trade-off between <b>latency</b> (delay) and <b>stability</b> (no stuttering).
<br><br>
<b>Low presets</b> (Ultra Low → Responsive): Minimal delay, best on clean <b>5 GHz Wi-Fi</b> or USB. May stutter on congested networks.
<br><br>
<b>High presets</b> (Stable → Maximum): More delay, but handles network jitter well. Best for <b>2.4 GHz Wi-Fi</b> or unreliable connections.
<br><br>
<b>Tip:</b> Start with <i>Balanced</i>, slide left until you hear stuttering, then go back one step.`,
  },
  'min-depth': {
    title: 'Minimum Depth (ms)',
    body: `The absolute floor — the buffer will <b>never</b> drop below this amount of audio.
<br><br>
<b>Low values</b> (2-10 ms): Near-zero floor. Great for USB or perfect Wi-Fi. Risks starvation if the network hiccups.
<br><br>
<b>Higher values</b> (30-60 ms): Safety cushion. The system won't even attempt to go this low, preventing stuttering on jittery networks.
<br><br>
<b>Tip:</b> On Wi-Fi, keep this at <b>20+ ms</b>. On USB, you can safely use <b>2 ms</b>.`,
  },
  'comfort-cap': {
    title: 'Comfort Cap (ms)',
    body: `The maximum latency the buffer is allowed to grow to. Prevents runaway delay after network spikes.
<br><br>
<b>Tight cap</b> (50-100 ms): Keeps latency low but may stutter if the network needs more headroom.
<br><br>
<b>Wide cap</b> (200-400 ms): Allows the system to absorb bigger network disruptions at the cost of higher peak delay.
<br><br>
<b>Tip:</b> Set this to <b>2-4×</b> your typical network jitter. On 5 GHz (~20 ms jitter), try <b>80 ms</b>. On 2.4 GHz (~80 ms), try <b>300 ms</b>.`,
  },
  bounce: {
    title: 'Bounce Multiplier',
    body: `When the buffer runs empty (starvation), the system raises its target depth by this multiplier to prevent repeated stalls.
<br><br>
<b>Gentle</b> (1.1-1.3×): Small bump, recovers fast. Good for stable networks.
<br><br>
<b>Aggressive</b> (2.0×+): Big jump, very safe. Adds noticeable delay after each starvation event.
<br><br>
<b>Tip:</b> Use <b>1.1-1.3×</b> on reliable networks. Use <b>2.0×+</b> if you experience repeated short stutters.`,
  },
  resume: {
    title: 'Resume Threshold (%)',
    body: `After a starvation (buffer ran empty), audio mutes while the buffer refills. This sets how full the buffer must be before audio resumes.
<br><br>
<b>Low</b> (20-30%): Audio resumes quickly but risks re-stalling if the network is still unstable.
<br><br>
<b>High</b> (75-100%): Waits for a full buffer before resuming — slower but very safe.
<br><br>
<b>Tip:</b> <b>25-50%</b> works well for most cases.`,
  },
  wsola: {
    title: 'WSOLA Max Skip',
    body: `When latency is above the target, the system uses <b>WSOLA time-stretching</b> to gradually shed excess audio and catch up. This sets the maximum frames it can skip per audio callback.
<br><br>
<b>High</b> (3-4 frames): Catches up fast (~80 ms/sec), but may cause brief pitch artifacts.
<br><br>
<b>Low</b> (1 frame): Gentle, inaudible correction (~20 ms/sec) but slow to recover.
<br><br>
<b>Tip:</b> Use <b>2-3</b> for most cases. Use <b>1</b> if you hear audio glitches during latency correction.`,
  },
  'initial-comfort': {
    title: 'Initial Comfort (ms)',
    body: `The starting point for the buffer's target depth on <b>first connect</b> or <b>reconnect</b>.
<br><br>
Instead of starting low and bouncing up (which causes a temporary latency spike), the system starts here and settles down.
<br><br>
<b>Low</b> (2-15 ms): Start aggressively low. Best for USB or perfect 5 GHz.
<br><br>
<b>Higher</b> (30-60 ms): Start with a safety cushion. Avoids early starvation on jittery networks.
<br><br>
<b>Tip:</b> Set this close to your <b>expected stable latency</b>. On good 5 GHz, that's typically <b>30-50 ms</b>.`,
  },
  'fast-settle-speed': {
    title: 'Fast Settle Speed (multiplier)',
    body: `After connecting (or switching presets), the system enters a "fast settle" mode where the buffer converges to its optimal depth faster than normal.
<br><br>
This multiplier controls how much faster the bleed rate is during the settle window.
<br><br>
<b>High</b> (4-6×): Very aggressive convergence. Reaches optimal latency in under a second.
<br><br>
<b>Low</b> (1.2-2×): Gentle convergence. Takes a few seconds to reach optimal.
<br><br>
<b>Tip:</b> Higher values work best on stable networks. Lower values are safer on jittery connections.`,
  },
  'fast-settle-duration': {
    title: 'Fast Settle Duration (frames)',
    body: `How many audio frames the fast settle window lasts. After this many frames, the system returns to its normal, slower bleed rate.
<br><br>
At 48 kHz / 960 samples per frame, <b>200 frames ≈ 1 second</b> of real time.
<br><br>
<b>Short</b> (100-150 frames): Fast settle expires quickly — good if your network is consistent.
<br><br>
<b>Long</b> (200-400 frames): Extended fast convergence — better for variable networks that need more time to find the right depth.
<br><br>
<b>Tip:</b> <b>200</b> (~1 second) works well for most scenarios.`,
  },
  'exclusive-mode': {
    title: 'Exclusive Mode (Oboe)',
    body: `Bypasses Android's audio mixer for direct hardware access, saving <b>~20-40 ms</b> of system audio latency.
<br><br>
<b>ON:</b> Lower latency, but <b>no other app</b> can play sound while GemaCast is streaming.
<br><br>
<b>OFF:</b> Standard audio path with Android mixer. Slightly higher latency but other apps can play sounds normally.
<br><br>
<b>Note:</b> Requires disconnect and reconnect to take effect. Some devices may not support this — if audio breaks, turn it off.`,
  },
  'connection-mode': {
    title: 'Connection Mode',
    body: `How audio packets travel from your PC to this device.
<br><br>
<b>Wi-Fi:</b> Standard wireless. Works automatically when both devices are on the same network. Typical latency: <b>40-80 ms</b> on 5 GHz, <b>80-200 ms</b> on 2.4 GHz.
<br><br>
<b>USB:</b> Direct wired connection via USB tethering. Much lower and more stable latency (<b>~10-30 ms</b>). Requires USB tethering enabled on your phone.
<br><br>
<b>ADB:</b> Routes through Android Debug Bridge port forwarding. Developers only.`,
  },
};

export function initSettingsDrawer(app: App) {
  const stateHandler = app.stateHandler;
  const drawer = document.getElementById(
    'settings-drawer',
  ) as HTMLDialogElement;
  const openBtn = document.getElementById(
    'settings-open-btn',
  ) as HTMLButtonElement;
  const closeBtn = document.getElementById(
    'settings-close-btn',
  ) as HTMLButtonElement;
  const themeBtn = document.getElementById(
    'theme-toggle-btn',
  ) as HTMLButtonElement;

  const presetContainer = document.getElementById('setting-preset') as HTMLElement;
  const presetHeader = document.getElementById('custom-preset-header') as HTMLElement;
  const presetValue = document.getElementById('custom-preset-value') as HTMLElement;
  const presetDropdown = document.getElementById('custom-preset-dropdown') as HTMLElement;
  let currentPresetValue = 5;
  const customConfig = document.getElementById(
    'custom-jitter-config',
  ) as HTMLDivElement;

  const minDepth = document.getElementById(
    'setting-min-depth',
  ) as HTMLInputElement;
  const comfortCap = document.getElementById(
    'setting-comfort-cap',
  ) as HTMLInputElement;
  const bounce = document.getElementById('setting-bounce') as HTMLInputElement;
  const resume = document.getElementById('setting-resume') as HTMLInputElement;
  const wsola = document.getElementById('setting-wsola') as HTMLInputElement;
  const initialComfort = document.getElementById('setting-initial-comfort') as HTMLInputElement;
  const fastSettleMult = document.getElementById('setting-fast-settle-mult') as HTMLInputElement;
  const fastSettleFrames = document.getElementById('setting-fast-settle-frames') as HTMLInputElement;

  const excMode = document.getElementById(
    'setting-exclusive-mode',
  ) as HTMLInputElement;
  const modes = document.getElementsByName(
    'conn-mode',
  ) as NodeListOf<HTMLInputElement>;
  const customApplyBtn = document.getElementById(
    'custom-apply-btn',
  ) as HTMLButtonElement;
  const customResetBtn = document.getElementById(
    'custom-reset-btn',
  ) as HTMLButtonElement;

  const helpModal = document.getElementById('help-modal') as HTMLDialogElement;
  const helpClose = document.getElementById(
    'help-close-btn',
  ) as HTMLButtonElement;
  const helpTitle = document.getElementById('help-title') as HTMLElement;
  const helpBody = document.getElementById('help-body') as HTMLElement;

  const focusedInputs = new Set<HTMLElement>();
  const customInputs = [minDepth, comfortCap, bounce, resume, wsola, initialComfort, fastSettleMult, fastSettleFrames];
  customInputs.forEach((input) => {
    input.addEventListener('focus', () => focusedInputs.add(input));
    input.addEventListener('blur', () => focusedInputs.delete(input));
  });

  presetDropdown.innerHTML = '';
  const presetOptionsList: HTMLElement[] = [];
  JITTER_PRESETS.forEach((preset, i) => {
    const opt = document.createElement('div');
    opt.className = 'custom-select__option';
    
    const title = document.createElement('div');
    title.className = 'custom-select__option-title';
    title.textContent = preset;
    opt.appendChild(title);
    
    if (i !== 10 && PRESET_DESCRIPTIONS[i]) {
      const desc = document.createElement('div');
      desc.className = 'custom-select__option-desc';
      desc.textContent = PRESET_DESCRIPTIONS[i];
      opt.appendChild(desc);
    }
    
    opt.addEventListener('click', () => {
      currentPresetValue = i;
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

  stateHandler.subscribe((state: AppState) => {
    const s = state.settings;
    if (s.theme === 'dark') {
      document.documentElement.classList.add('dark-theme');
      document.documentElement.classList.remove('light-theme');
    } else {
      document.documentElement.classList.remove('dark-theme');
      document.documentElement.classList.add('light-theme');
    }

    currentPresetValue = s.bufferPreset;
    presetValue.textContent = JITTER_PRESETS[s.bufferPreset] || 'Custom';
    
    presetOptionsList.forEach((opt, i) => {
      if (i === s.bufferPreset) opt.classList.add('custom-select__option--selected');
      else opt.classList.remove('custom-select__option--selected');
    });

    customConfig.hidden = s.bufferPreset !== 10;

    if (!focusedInputs.has(minDepth))
      minDepth.value = s.customJitterConfig.minDepthMs.toString();
    if (!focusedInputs.has(comfortCap))
      comfortCap.value = s.customJitterConfig.comfortCapMs.toString();
    if (!focusedInputs.has(bounce))
      bounce.value = s.customJitterConfig.bounceMultiplier.toString();
    if (!focusedInputs.has(resume))
      resume.value = (s.customJitterConfig.resumeThresholdPct * 100).toString();
    if (!focusedInputs.has(wsola))
      wsola.value = s.customJitterConfig.wsolaMaxSkip.toString();
    if (!focusedInputs.has(initialComfort))
      initialComfort.value = s.customJitterConfig.initialComfortMs.toString();
    if (!focusedInputs.has(fastSettleMult))
      fastSettleMult.value = s.customJitterConfig.fastSettleMultiplier.toString();
    if (!focusedInputs.has(fastSettleFrames))
      fastSettleFrames.value = s.customJitterConfig.fastSettleFrames.toString();

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

  let lastNonCustomPreset = stateHandler.getState().settings.bufferPreset;
  if (lastNonCustomPreset === 10) lastNonCustomPreset = 5; // fallback to Stable
  let customApplied = false;

  const updateState = () => {
    const presetIdx = currentPresetValue;
    const currSettings = stateHandler.getState().settings;

    let nextCustom = currSettings.customJitterConfig;

    if (presetIdx !== 10) {
      lastNonCustomPreset = presetIdx;
    } else if (presetIdx === 10 && currSettings.bufferPreset !== 10) {
      if (!customApplied) {
        nextCustom = getPresetConfig(lastNonCustomPreset, nextCustom);
      }
    }

    let nextMode = currSettings.mode;
    modes.forEach((m: HTMLInputElement) => {
      if (m.checked && !m.disabled) nextMode = m.value as ConnectionMode;
    });

    stateHandler.setState({
      settings: {
        ...currSettings,
        bufferPreset: presetIdx,
        customJitterConfig: nextCustom,
        exclusiveMode: excMode.checked,
        mode: nextMode,
      },
    });

    if (presetIdx !== 10) {
      const activeConfig = getPresetConfig(presetIdx, nextCustom);
      console.log(
        '[settings] Sending update_jitter_config:',
        JSON.stringify(activeConfig),
      );
      invoke('update_jitter_config', { jitterConfig: activeConfig }).catch(
        console.warn,
      );
    }
  };

  excMode.addEventListener('change', () => {
    const state = stateHandler.getState();
    stateHandler.setState({
      settings: { ...state.settings, exclusiveMode: excMode.checked },
    });
    if (state.connectedSender && state.status === Status.Playing) {
      app.connection.disconnect(false);
    }
  });

  modes.forEach((m: HTMLInputElement) =>
    m.addEventListener('change', updateState),
  );

  customApplyBtn.addEventListener('click', () => {
    const currSettings = stateHandler.getState().settings;
    const custom = {
      minDepthMs: parseInt(minDepth.value, 10) || 0,
      comfortCapMs: parseInt(comfortCap.value, 10) || 0,
      bounceMultiplier: parseFloat(bounce.value) || 0,
      resumeThresholdPct: (parseFloat(resume.value) || 0) / 100.0,
      wsolaMaxSkip: parseInt(wsola.value, 10) || 0,
      initialComfortMs: parseInt(initialComfort.value, 10) || 0,
      fastSettleMultiplier: parseFloat(fastSettleMult.value) || 0,
      fastSettleFrames: parseInt(fastSettleFrames.value, 10) || 0,
    };
    customApplied = true;
    stateHandler.setState({
      settings: { ...currSettings, customJitterConfig: custom },
    });
    console.log('[settings] Custom Apply (locked):', JSON.stringify(custom));
    invoke('update_jitter_config', { jitterConfig: custom }).catch(
      console.warn,
    );
  });

  customResetBtn.addEventListener('click', () => {
    customApplied = false;
    const currSettings = stateHandler.getState().settings;
    const fromPreset = getPresetConfig(
      lastNonCustomPreset,
      currSettings.customJitterConfig,
    );
    stateHandler.setState({
      settings: { ...currSettings, customJitterConfig: fromPreset },
    });
  });

  themeBtn.addEventListener('click', () => {
    const curr = stateHandler.getState().settings;
    stateHandler.setState({
      settings: {
        ...curr,
        theme: curr.theme === 'dark' ? 'light' : 'dark',
      },
    });
  });

  document.querySelectorAll<HTMLButtonElement>('.help-btn').forEach((btn) => {
    btn.addEventListener('click', () => {
      const key = btn.dataset.help ?? '';
      const info = HELP_TEXTS[key];
      if (!info) return;
      helpTitle.textContent = info.title;
      helpBody.innerHTML = info.body;
      helpModal.showModal();
    });
  });
  helpClose.addEventListener('click', () => helpModal.close());
  helpModal.addEventListener('click', (e) => {
    if (e.target === helpModal) helpModal.close();
  });

  openBtn.addEventListener('click', () => drawer.showModal());
  closeBtn.addEventListener('click', () => drawer.close());
  drawer.addEventListener('click', (e) => {
    if (e.target === drawer) drawer.close();
  });
}
