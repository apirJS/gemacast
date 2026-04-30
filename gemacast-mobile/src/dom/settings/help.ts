export const HELP_TEXTS: Record<string, { title: string; body: string }> = {
  'buffer-preset': {
    title: 'Buffer Preset',
    body: `Presets control how much audio is buffered before playback — a trade-off between <b>latency</b> (delay) and <b>stability</b> (no stuttering).
<br><br>
<b>Low presets</b> (Wired, Fast): Minimal delay, best on clean <b>5 GHz Wi-Fi</b> or USB. May stutter on congested networks.
<br><br>
<b>High presets</b> (Stable, Resilient): More delay, but handles network jitter well. Best for <b>2.4 GHz Wi-Fi</b> or unreliable connections.
<br><br>
<b>Tip:</b> Start with <i>Balanced</i> and adjust from there.`,
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
  'buffer-mode': {
    title: 'Buffer Mode',
    body: `Choose how the buffer depth is determined.
<br><br>
<b>Static:</b> Locks the buffer to an exact fixed depth in milliseconds. The latency will never shift or adapt — what you set is what you get. Best for stable connections (USB, wired, controlled Wi-Fi).
<br><br>
<b>Adaptive:</b> Uses a smart Dual-EMA algorithm that dynamically adjusts buffer depth based on real-time network jitter. Automatically handles Wi-Fi bursts and background scans. Best for unpredictable wireless connections.`,
  },
  'static-depth': {
    title: 'Buffer Depth (ms)',
    body: `The exact fixed depth of the audio buffer in milliseconds.
<br><br>
Lower values = less latency but more risk of stutter on bad networks. Higher values = more stability but more delay.
<br><br>
<b>USB/ADB:</b> 10-30 ms<br>
<b>5 GHz Wi-Fi:</b> 40-80 ms<br>
<b>2.4 GHz Wi-Fi:</b> 100-300 ms`,
  },
};

export function initHelpModal() {
  const helpModal = document.getElementById('help-modal') as HTMLDialogElement;
  const helpClose = document.getElementById(
    'help-close-btn',
  ) as HTMLButtonElement;
  const helpTitle = document.getElementById('help-title') as HTMLElement;
  const helpBody = document.getElementById('help-body') as HTMLElement;

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
}
