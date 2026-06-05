export const HELP_CONTENT: Record<string, { title: string; body: string }> = {
  'buffer-preset': {
    title: 'Buffer Preset',
    body: 'Presets configure the adaptive jitter buffer that absorbs network timing variations.\n\n• Auto — Intelligently detects your network quality (5 GHz vs 2.4 GHz) and adapts automatically. Best for most users.\n• Wired — Minimal buffering for USB tethering or ADB. Sub-20ms latency.\n• Fast — Tuned for stable 5 GHz Wi-Fi. Very low latency with light jitter absorption.\n• Balanced — The default. Works well on most networks.\n• Stable — For congested or 2.4 GHz Wi-Fi. Absorbs periodic background scan stutters.\n• Resilient — For unreliable connections or screen-off streaming. Maximum stability.\n• Custom — Define your own parameters manually.',
  },
  'buffer-mode': {
    title: 'Buffer Mode',
    body: 'Static: Fixed buffer size. Predictable latency, but cannot adapt to changing network conditions.\n\nAdaptive: Dynamically adjusts buffer size based on real-time network jitter. Automatically grows during spikes and shrinks when stable.',
  },
  'static-depth': {
    title: 'Static Buffer Depth',
    body: 'The exact fixed latency (in ms) to maintain.\n\n• Lower = less latency, more stutters\n• Higher = more latency, rock-solid playback\n\nTypical values: 20–60ms for good networks.',
  },
  'min-depth': {
    title: 'Min Depth',
    body: 'The minimum allowed latency (in ms). The adaptive algorithm will never shrink below this value.\n\nIncrease this if you experience frequent micro-stutters on a generally good network.',
  },
  'comfort-cap': {
    title: 'Comfort Cap',
    body: 'The maximum allowed latency (in ms). The adaptive algorithm will never exceed this ceiling, limiting your worst-case latency during severe network drops.',
  },
  bounce: {
    title: 'Peak Decay Half-life',
    body: 'How quickly the buffer shrinks after expanding to absorb a jitter spike.\n\n• 0 = Auto Mode (Recommended)\n• Lower = Recovers latency faster, but less stable\n• Higher = Stays buffered longer, prevents cyclic stuttering',
  },
  resume: {
    title: 'Resume Threshold',
    body: 'After an audio drop-out, this is the percentage of buffer (0 to 1) that must refill before audio resumes.\n\n• Higher = Waits longer before playing, fewer repeated stutters\n• Lower = Resumes audio faster',
  },
  'exclusive-mode': {
    title: 'Exclusive Mode',
    body: 'When enabled, the Android audio output stream requests exclusive access to the audio hardware (Oboe SharingMode::Exclusive). This can reduce latency by bypassing the Android audio mixer.\n\nNote: Not all devices support exclusive mode. If the device denies the request, it will automatically fall back to shared mode.',
  },
  'connection-mode': {
    title: 'Connection Mode',
    body: 'WiFi: Standard wireless connection over your local network. Latency depends on Wi-Fi quality; 5 GHz band recommended.\n\nUSB: Audio streams over USB tethering. Very low latency (~0.5ms transit) over a physical cable.\n\nADB: Audio streams via USB debug bridge with TCP transport. Requires ADB reverse port forwarding. Uses length-prefixed TCP framing for reliable delivery.',
  },
  'audio-bitrate': {
    title: 'Audio Bitrate Quality',
    body: 'Controls the Opus encoder bitrate on the PC sender. Higher bitrate = better audio quality but more bandwidth.\n\n• 10–32 Kbps: Voice/FM quality\n• 64–96 Kbps: Good quality\n• 128 Kbps: Recommended default (high quality, low bandwidth)\n• 256–512 Kbps: Transparent quality\n• Uncompressed PCM: Zero codec latency, bypasses Opus entirely. Requires ~1.5 Mbps bandwidth for stereo 48kHz.',
  },
};
