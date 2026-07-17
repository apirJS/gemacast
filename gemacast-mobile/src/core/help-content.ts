export const HELP_CONTENT: Record<string, { title: string; body: string }> = {
  'buffer-preset': {
    title: 'Buffer Preset',
    body: 'Choose how much audio Gemacast stores before playing it. More buffer = smoother audio but slightly delayed. Less buffer = faster response but more sensitive to Wi-Fi hiccups.\n\n• No Buffer — Plays audio instantly with no delay. Great for testing or rock-solid wired connections. Will crackle if your network has any hiccups.\n\n• Auto (Recommended) — Figures out the best setting for your network automatically. Just set it and forget it.\n\n• Wired — For USB cable or ADB connections. Ultra-low delay since the cable handles the heavy lifting.\n\n• Fast — For strong 5 GHz Wi-Fi (e.g., same room as your router). Very snappy, with just enough buffer to handle tiny glitches.\n\n• Balanced — Works well on most home networks. Good mix of low delay and smooth playback.\n\n• Stable — For weaker Wi-Fi (e.g., 2.4 GHz, different floor from router, or crowded apartment Wi-Fi). Adds more buffer to ride out interference.\n\n• Resilient — For unreliable connections or when you turn your screen off while streaming. Prioritizes uninterrupted playback over low delay.\n\n• Custom — Set each buffer parameter yourself. For advanced users only.',
  },

  'static-depth': {
    title: 'Static Buffer Depth',
    body: "Sets the exact amount of delay (in milliseconds) before audio plays.\n\n• Lower values (e.g., 10–20 ms) — Audio feels nearly instant, but may stutter on imperfect networks.\n• Higher values (e.g., 40–80 ms) — Rock-solid playback, but you'll notice a slight delay.\n\nTip: Start around 30 ms and increase if you hear crackling or gaps.",
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
