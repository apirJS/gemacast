export const HELP_CONTENT: Record<string, { title: string; body: string }> = {
  'buffer-preset': {
    title: 'Buffer Preset',
    body: 'Audio is held briefly before playing, so late packets have time to arrive. Bigger buffer, fewer drop-outs, more delay.\n\n• Auto — sizes it for you. Recommended.\n• Wired — smallest, for cables. No margin if a packet is late.\n• Fast → Resilient — fixed sizes, small to large. Move further down the list if audio breaks up.\n• Custom — your own size, saved by name.\n• No Buffer — plays instantly, needs a perfect connection.\n\nEach preset shows its size next to the name.',
  },

  'static-depth': {
    title: 'Buffer Depth',
    body: 'A fixed buffer size, used exactly as entered. Automatic sizing is off.\n\nRaise it if audio breaks up, lower it for less delay — your total delay moves by the same amount.\n\n0 to 5000 ms.',
  },

  'exclusive-mode': {
    title: 'Exclusive Mode',
    body: "Sends audio straight to the hardware instead of through Android's mixer, shaving off a little delay.\n\nOther apps may lose sound while Gemacast plays. Not every phone allows it — if yours refuses, the toggle is greyed out.\n\nLeave it off unless you need the lowest delay possible.",
  },
  'keep-screen-on': {
    title: 'Keep Screen On',
    body: 'Stops the screen turning off while the app is open.\n\nStreaming keeps working with the screen off, but some phones throttle harder once it is, which can cause stuttering. Turn this on if that happens — otherwise leave it off and save battery.',
  },
  'connection-mode': {
    title: 'Connection Mode',
    body: 'How the phone reaches the PC, lowest delay first.\n\n• USB — cable with USB tethering on. Lowest delay, steadiest timing.\n• ADB — cable over Android Debug Bridge. About the same. Use it when tethering is not available.\n• Wi-Fi — no cable, more delay. 5 GHz is usually fine; 2.4 GHz stutters the most.\n\nGemacast spots your Wi-Fi band and calculates the buffer to match. The badge on the main screen shows the link it used.',
  },
  'audio-bitrate': {
    title: 'Audio Bitrate Quality',
    body: 'How much the audio is compressed before sending.\n\nHigher is not always better — a bigger stream suffers more when the network hiccups, so a high setting can sound worse than a moderate one.\n\n128 Kbps is recommended and hard to tell from the original. Go lower if the stream keeps breaking up. Uncompressed PCM skips compression entirely and needs a fast, stable connection.',
  },
  'audio-gain': {
    title: 'Audio Gain',
    body: "Volume for the incoming stream, on top of your phone's volume buttons.\n\n• 0 dB — unchanged. Leave it here unless the volume is wrong.\n• Above 0 — louder, for quiet PC audio. Too much boost distorts.\n• Below 0 — quieter, for loud PC audio. Saved between sessions.",
  },
  'connection-metrics': {
    title: 'Connection Metrics',
    body: 'Live readings, in milliseconds.\n\n• Buffer — amount of audio (in ms) waiting to play. The biggest part of your delay, and it grows on its own when the connection gets worse.\n• RTT — network round trip latency to the PC. Shows n/a on ADB, which is normal.\n• Jitter — how uneven packet arrival is. This is what causes stuttering, and what pushes Buffer up.\n\nIf Buffer from "Auto" preset is too high, move closer to the router, switch to 5 GHz, or plug in a cable.',
  },
};
