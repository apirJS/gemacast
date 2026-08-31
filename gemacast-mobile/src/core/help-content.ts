export const HELP_CONTENT: Record<string, { title: string; body: string }> = {
  'buffer-preset': {
    title: 'Buffer Preset',
    body: 'The app holds a little audio back before playing it, so anything arriving late still makes it in time. A bigger buffer means fewer drop-outs but more delay.\n\n• Auto — picks the size for you. Recommended.\n• Wired — the smallest. For USB or ADB only.\n• Fast to Resilient — fixed sizes, small to large. Pick a bigger one if the audio breaks up.\n• Custom — your own size, saved by name.\n• No Buffer — plays instantly. Only works on a perfect connection.\n\nEach preset shows its size next to the name.',
  },

  'static-depth': {
    title: 'Buffer Depth',
    body: 'The exact buffer size to use. Automatic sizing is turned off.\n\nA bigger number means fewer drop-outs but more delay.\n\nAnything from 0 to 5000 milliseconds.',
  },

  'exclusive-mode': {
    title: 'Exclusive Mode',
    body: "Sends audio straight to your phone's audio hardware instead of through Android, which saves a little delay.\n\nOther apps may lose their sound while Gemacast is playing. Some phones do not allow this at all — the toggle is greyed out if yours does not.\n\nLeave it off unless you want the lowest delay possible.",
  },
  'keep-screen-on': {
    title: 'Keep Screen On',
    body: 'Keeps the screen awake while the app is open.\n\nAudio keeps playing with the screen off, but some phones slow the app down (throttle the wifi chip & performance) once it is off, which can cause stuttering and buffer bloat. Turn this on if that happens. Otherwise leave it off and save battery.',
  },
  'auto-reconnect': {
    title: 'Auto Reconnect',
    body: 'When you open the app again, it connects to the last PC you used, on the same connection mode. If you are on a different mode, it switches for you.\n\nIf the connection drops while you are in the app, it reconnects by itself.\n\nIt will not reconnect if:\n• You disconnected it yourself\n• The PC disconnected you from its system tray\n• That connection mode is not available — cable unplugged, or Wi-Fi off',
  },
  'connection-mode': {
    title: 'Connection Mode',
    body: 'How the phone reaches the PC, listed from lowest delay to highest.\n\n• USB — cable, with USB tethering turned on in Android settings. Lowest delay and the steadiest.\n• ADB — cable, using Android developer mode. About as good as USB. Use it if your phone cannot tether.\n• Wi-Fi — no cable, but more delay. 5 GHz is usually fine; 2.4 GHz stutters the most.\n\nThe app works out which one you are on and sets the buffer to match. The badge on the main screen shows what it found.',
  },
  'audio-bitrate': {
    title: 'Audio Bitrate Quality',
    body: 'How much the audio is compressed before being sent.\n\nHigher is not always better. 128 Kbps is recommended and is hard to tell apart from the original. Go lower if the audio keeps breaking up. Uncompressed sends the audio with no compression at all and needs a fast, steady connection.',
  },
  'audio-gain': {
    title: 'Audio Gain',
    body: "Volume for the stream, on top of your phone's volume buttons.\n\n• 0 dB — no change. Leave it here unless the volume is wrong.\n• Above 0 — louder, for quiet PC audio. Too much makes it distort.\n• Below 0 — quieter, for loud PC audio.\n\nYour setting is remembered for next time.",
  },
  'connection-metrics': {
    title: 'Connection Metrics',
    body: 'Live numbers, all in milliseconds.\n\n• Buffer — how much audio is waiting to play. This is the biggest part of your delay, and it grows on its own when the connection gets worse.\n• RTT — how long a message takes to reach the PC and come back. Shows n/a on ADB, which is normal.\n• Jitter — how unevenly the audio is arriving. This is what causes stuttering, and what pushes Buffer up.\n\nIf Buffer is high on the Auto preset, move closer to the router, switch to 5 GHz, or use a cable.',
  },
};
