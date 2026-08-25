export const HELP_CONTENT: Record<string, { title: string; body: string }> = {
  'buffer-preset': {
    title: 'Buffer Preset',
    body: 'The buffer stores incoming audio before playing it, which gives late packets time to arrive. A bigger buffer means fewer drop-outs but more delay.\n\n• Auto — measures your connection and sets the buffer for you. Recommended.\n• Wired — small fixed buffer for cable connections. Lowest delay, but no margin for late packets. Switch to Auto if you get drop-outs.\n• Fast, Balanced, Stable, Resilient — fixed buffers, from smallest to largest. Move to the next one down if audio still breaks up.\n• Custom — set the buffer size yourself and save it with a name.\n• No Buffer — plays packets immediately. Only usable on a perfect connection.\n\nThe buffer size for each preset is shown next to its name. Saved presets appear in the same list.',
  },

  'static-depth': {
    title: 'Buffer Depth',
    body: 'Sets the buffer to a fixed size. Automatic sizing is turned off and this value is used exactly as entered.\n\nIncrease it if audio breaks up. Decrease it to reduce delay. Total delay changes by the same amount you change here.\n\nAccepts whole numbers from 0 to 5000 ms.',
  },

  'exclusive-mode': {
    title: 'Exclusive Mode',
    body: 'Requests a direct output path to the audio hardware instead of going through the Android mixer. Reduces playback delay slightly.\n\nNot all devices allow this. Gemacast tests for it when connecting; if the device refuses, the toggle is disabled and shows "Not supported on this device".\n\nOther apps may lose audio output while Gemacast is playing. Leave this off unless you need the lowest possible delay.',
  },
  'keep-screen-on': {
    title: 'Keep Screen On',
    body: 'Keeps the display on while the app is open.\n\nStreaming continues with the screen off, because the app runs a foreground service. However, some devices apply stricter power limits once the display is off, which can cause stuttering. Enable this if that happens. Otherwise leave it off to save battery.\n\nThe lock is released when you switch away from the app.',
  },
  'connection-mode': {
    title: 'Connection Mode',
    body: 'Selects how the phone connects to the PC, listed from lowest to highest delay.\n\n• USB — USB cable with USB tethering enabled on the phone. Lowest delay and the most consistent timing.\n• ADB — USB cable using Android Debug Bridge. Similar delay to USB. Use this when tethering is unavailable.\n• Wi-Fi — wireless. Higher delay than a cable, and it depends on the band: 5 GHz is usually stable, 2.4 GHz is congested and causes the most stuttering.\n\nGemacast detects the Wi-Fi band automatically and sets the buffer to match. The badge on the main screen shows which link it used.',
  },
  'audio-bitrate': {
    title: 'Audio Bitrate Quality',
    body: 'Sets how much the audio is compressed before sending. Each option in the list describes its quality level.\n\nA higher bitrate is not always better. A larger stream is more affected by network problems, so a high setting can sound worse than a moderate one on the same connection. Uncompressed PCM applies no compression at all and needs a fast, stable connection to be worth using.\n\n128 Kbps is the recommended setting and is difficult to distinguish from the original. Go higher only if you can hear a difference. Go lower if the stream keeps breaking up.',
  },
  'audio-gain': {
    title: 'Audio Gain',
    body: "Adjusts the volume of the incoming stream, applied on top of the phone's volume buttons.\n\n• 0 dB — no change. Use this unless the volume is wrong.\n• Above 0 dB — increases volume. Use when PC audio is too quiet. Too much boost will clip and distort loud parts.\n• Below 0 dB — decreases volume. Use when PC audio is too loud.\n\nRaising the volume on the PC is better than boosting here. This setting is saved between sessions.",
  },
  'connection-metrics': {
    title: 'Connection Metrics',
    body: 'Three live measurements of the connection, in milliseconds.\n\n• Buffer — how much audio is currently buffered before playback. This is the largest part of the total delay. It increases automatically when the connection gets worse.\n• RTT — round-trip time to the PC. Shows n/a on ADB, because the probe does not run over a loopback connection. This is normal.\n• Jitter — how much the packet arrival timing varies. High jitter is what causes stuttering, and it is what makes Buffer increase.\n\nGreen is normal, amber is borderline, red means the connection is failing. If Buffer is high, check Jitter first: move closer to the router, switch to 5 GHz, or use a USB cable.\n\nThe badge next to the status text shows which link the buffer was sized for. If the phone and PC report different links, the slower of the two is used.',
  },
};
