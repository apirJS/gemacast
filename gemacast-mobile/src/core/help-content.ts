export const HELP_CONTENT: Record<string, { title: string; body: string }> = {
  'buffer-preset': {
    title: 'Buffer Preset',
    body: 'Choose how much audio Gemacast stores before playing it. More buffer = smoother audio but slightly delayed. Less buffer = faster response but more sensitive to Wi-Fi hiccups.\n\n• No Buffer — Plays audio instantly with no delay. Great for testing or rock-solid wired connections. Will crackle if your network has any hiccups.\n\n• Auto (Recommended) — Figures out the best setting for your network automatically. Just set it and forget it.\n\n• Wired — For USB cable or ADB connections. Ultra-low delay since the cable handles the heavy lifting.\n\n• Fast — For strong 5 GHz Wi-Fi (e.g., same room as your router). Very snappy, with just enough buffer to handle tiny glitches.\n\n• Balanced — Works well on most home networks. Good mix of low delay and smooth playback.\n\n• Stable — For weaker Wi-Fi (e.g., 2.4 GHz, different floor from router, or crowded apartment Wi-Fi). Adds more buffer to ride out interference.\n\n• Resilient — For unreliable connections or when you turn your screen off while streaming. Prioritizes uninterrupted playback over low delay.\n\n• Custom — Set each buffer parameter yourself. For advanced users only.',
  },
  'buffer-mode': {
    title: 'Buffer Mode',
    body: 'Controls whether the buffer size stays fixed or adjusts on its own.\n\nStatic — The buffer stays at one fixed size you choose. The delay never changes, but audio may stutter if your network gets worse.\nExample: Good for a wired setup where conditions never change.\n\nAdaptive — The buffer automatically grows when your network gets choppy, then shrinks back down when things stabilize.\nExample: If someone starts a video call on your Wi-Fi, the buffer expands briefly to keep your audio smooth, then tightens back up when the call ends.',
  },
  'static-depth': {
    title: 'Static Buffer Depth',
    body: "Sets the exact amount of delay (in milliseconds) before audio plays.\n\n• Lower values (e.g., 10–20 ms) — Audio feels nearly instant, but may stutter on imperfect networks.\n• Higher values (e.g., 40–80 ms) — Rock-solid playback, but you'll notice a slight delay.\n\nTip: Start around 30 ms and increase if you hear crackling or gaps.",
  },
  'min-depth': {
    title: 'Minimum Buffer Depth',
    body: "The lowest delay the adaptive buffer is allowed to reach (in milliseconds). Even when your network is perfect, the buffer won't go below this value.\n\nWhy change it? If you hear occasional tiny pops or glitches on an otherwise good connection, try raising this a bit (e.g., from 8 ms to 15 ms). It gives the buffer a bigger safety net.\n\nExample: On a solid 5 GHz Wi-Fi, a min depth of 5–10 ms is usually enough. On 2.4 GHz, try 25–50 ms.",
  },
  'comfort-cap': {
    title: 'Maximum Buffer Depth',
    body: "The highest delay the adaptive buffer is allowed to reach (in milliseconds). Even during severe network problems, the delay won't exceed this limit.\n\nWhy it matters: Without a cap, a big Wi-Fi hiccup could push the buffer to several seconds of delay. The comfort cap prevents that.\n\nExample: A cap of 150 ms means your audio is never more than 150 ms behind your PC — about the delay of a Bluetooth speaker. A cap of 500 ms gives more room to survive heavy interference.",
  },
  bounce: {
    title: 'Peak Decay Half-life',
    body: "After the buffer grows to handle a network hiccup, this controls how quickly it shrinks back to normal.\n\n• 0 (Auto) — Recommended. Lets Gemacast decide the best recovery speed.\n• Lower values (e.g., 500 ms) — Snaps back to low delay fast, but may stutter if the network hiccup isn't fully over.\n• Higher values (e.g., 5000+ ms) — Stays cautious and shrinks slowly, which is safer on unstable Wi-Fi.\n\nExample: If your Wi-Fi stutters every few seconds (e.g., microwave interference), use a higher value so the buffer doesn't keep bouncing up and down.",
  },
  resume: {
    title: 'Resume Threshold',
    body: "When audio cuts out due to a network drop, this controls how much buffer must refill before playback restarts.\n\nThe value goes from 0 to 1 (think of it as a percentage):\n• 0.2 (20%) — Resumes playback quickly, but risks another stutter if the network isn't stable yet.\n• 0.7 (70%) — Waits longer to build up a bigger safety cushion before resuming.\n\nExample: If you notice audio stopping and starting repeatedly, raise this to 0.5 or higher so Gemacast waits until it has enough buffered audio before playing again.",
  },
  'exclusive-mode': {
    title: 'Exclusive Mode',
    body: "Gives Gemacast direct access to your phone's audio hardware, bypassing Android's built-in audio mixer. This can slightly reduce delay.\n\nKeep in mind:\n• Not all phones support this — if yours doesn't, Gemacast will silently fall back to normal mode.\n• While active, other apps may not be able to play sound at the same time.\n\nRecommendation: Leave this off unless you need the absolute lowest latency and don't mind other apps being muted.",
  },
  'keep-screen-on': {
    title: 'Keep Screen On',
    body: 'Prevents your screen from dimming or locking while Gemacast is open.\n\nWhy this matters: Android aggressively slows down network and background activity when your screen turns off. This can cause audio to stutter, cut out, or have increased delay.\n\nRecommendation: Keep this ON while streaming for the best experience. If you want to save battery and accept occasional hiccups, turn it off and use the "Resilient" buffer preset to compensate.\n\nNote: The screen lock is automatically released when you leave the app or close it.',
  },
  'connection-mode': {
    title: 'Connection Mode',
    body: 'How your phone connects to the PC running Gemacast.\n\nWi-Fi — Connects wirelessly over your local network. No cables needed.\nBest on 5 GHz Wi-Fi. Typical delay: 20–100 ms depending on network quality.\nExample: Phone and PC on the same home Wi-Fi.\n\nUSB — Streams audio over a USB cable using USB tethering. Extremely fast.\nTypical delay: Under 5 ms. Requires enabling USB tethering on your phone.\nExample: Phone plugged into your PC via USB-C cable.\n\nADB — Streams audio over a USB cable using Android Debug Bridge. Requires developer tools.\nTypical delay: Under 5 ms. Requires USB debugging enabled and ADB set up on your PC.\nExample: Developers or power users who already have ADB configured.',
  },
  'audio-bitrate': {
    title: 'Audio Quality',
    body: "Controls how much the audio is compressed before being sent from your PC. Higher quality uses more network bandwidth.\n\n• 10–32 Kbps — Low quality, like a phone call or AM radio. Uses very little bandwidth.\n• 64–96 Kbps — Decent quality, like FM radio or a podcast. Good enough for casual listening.\n• 128 Kbps (Recommended) — High quality, similar to Spotify's normal setting. Great balance of quality and bandwidth.\n• 256–512 Kbps — Near-perfect quality. Hard to tell apart from the original. Good for music production monitoring.\n• Lossless (PCM) — Sends audio exactly as-is with zero compression. Perfect quality, but needs a strong connection (~1.5 Mbps). Best over USB or very strong Wi-Fi.",
  },
  'audio-gain': {
    title: 'Volume Boost',
    body: "Adjusts how loud the streamed audio plays on your phone, on top of your regular volume controls.\n\n• -24 dB — Nearly silent. Useful for bringing down very loud sources.\n• 0 dB — No change. Audio plays at its original volume.\n• +12 dB — About 4× louder. Useful when your PC audio is too quiet.\n\nExamples:\n• PC volume is low but you can't change it (e.g., in a meeting)? Boost to +6 or +12 dB.\n• Audio is too loud even at low phone volume? Set gain to -6 or -12 dB.\n\nYour gain setting is saved automatically and will be restored next time you connect.",
  },
};
