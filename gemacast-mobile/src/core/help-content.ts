export const HELP_CONTENT: Record<string, { title: string; body: string }> = {
  'buffer-preset': {
    title: 'Buffer Preset',
    body: 'Controls how much audio is stored before playing. More buffer = smoother but slightly delayed. Less = faster but more sensitive to Wi-Fi issues.\n\n• No Buffer — Zero delay. Only for wired connections.\n• Auto — Adapts to your network. Best for most users.\n• Wired — For USB/ADB cable connections.\n• Fast — Strong Wi-Fi, same room as router.\n• Balanced — Good for most home networks.\n• Stable — Weak Wi-Fi or interference-heavy environments.\n• Resilient — Unreliable networks or screen-off streaming.\n• Custom — Manual control. Advanced users only.',
  },

  'static-depth': {
    title: 'Static Buffer Depth',
    body: 'How many milliseconds of audio to store before playing. Lower = less delay but may stutter. Higher = smoother but more delay.\n\nStart at 30 ms. Increase if you hear crackling.',
  },

  'exclusive-mode': {
    title: 'Exclusive Mode',
    body: "Gives Gemacast direct hardware access, bypassing Android's audio mixer. Can reduce delay slightly.\n\nTrade-off: other apps may go silent while active. Not all devices support this — the toggle is disabled if your device lacks the required MMAP audio HAL.\n\nLeave off unless you need the absolute lowest latency.",
  },
  'keep-screen-on': {
    title: 'Keep Screen On',
    body: 'Prevents your screen from turning off while the app is open.\n\nAndroid throttles network activity when the screen is off, which can cause stuttering or latency increase.\n\nAutomatically released when you leave the app.',
  },
  'connection-mode': {
    title: 'Connection Mode',
    body: 'How your phone connects to the PC.\n\n• Wi-Fi — Wireless, no cables. Works best on 5 GHz. Typical delay: 20–100 ms.\n• USB — Over a USB cable with USB tethering enabled. Delay: under 5 ms.\n• ADB — Over a USB cable using Android Debug Bridge. Requires developer setup. Delay: under 5 ms.',
  },
  'audio-bitrate': {
    title: 'Audio Quality',
    body: 'Controls audio compression. Higher = better sound but uses more bandwidth.\n\n• 10–32 Kbps — Phone-call quality.\n• 64–96 Kbps — Podcast quality.\n• 128 Kbps — Recommended. Great balance of quality and bandwidth.\n• 256–512 Kbps — Near-lossless. For critical listening.\n• Lossless — Zero compression. Needs a strong connection (~1.5 Mbps).',
  },
  'audio-gain': {
    title: 'Volume Boost',
    body: "Adjusts streamed audio volume on top of your phone's volume controls.\n\n• 0 dB — No change.\n• Positive — Louder. Use when PC audio is too quiet.\n• Negative — Quieter. Use when PC audio is too loud.\n\nSaved automatically between sessions.",
  },
};
