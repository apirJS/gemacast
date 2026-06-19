import { useAppStore } from '../stores/app-store';
import { useToastStore } from '../stores/toast-store';
import { tauriBridge } from '../core/tauri-bridge';
import { GemaCastError } from '../core/error';
import { getPresetConfig } from '../core/presets';
import { saveLastSender } from '../core/persistence';
import { Status } from '../core/types';
import type { AudioSource, DiscoveredSender, Result } from '../core/types';
import { ok, err } from '../core/types';

const store = useAppStore;
const toast = useToastStore;

async function connectWithRetry(
  args: Parameters<typeof tauriBridge.connectToSender>[0],
  maxRetries: number,
  delayMs: number,
): Promise<void> {
  let lastError: unknown;
  for (let attempt = 0; attempt <= maxRetries; attempt++) {
    try {
      await tauriBridge.connectToSender(args);
      return;
    } catch (e) {
      lastError = e;
      if (attempt < maxRetries) {
        console.warn(`Connection attempt ${attempt + 1} failed, retrying in ${delayMs}ms...`, e);
        await new Promise((r) => setTimeout(r, delayMs));
      }
    }
  }
  throw lastError;
}

export async function connectToSender(
  sender: DiscoveredSender,
): Promise<Result<true, GemaCastError>> {
  store.getState().patch({
    isLoading: true,
    status: Status.Connecting,
    isSuspended: false,
    connectingSenderId: sender.deviceId,
  });

  try {
    const state = store.getState();
    const ip = sender.addr.split(':')[0];
    const settings = state.settings;
    const config = getPresetConfig(settings.bufferPreset, settings.customJitterConfig);

    const isManual = sender.deviceId.startsWith('manual-');
    const connectionMode = isManual ? 'wifi' : settings.mode;
    const transport = connectionMode === 'usb' ? 'usb' : connectionMode === 'wifi' ? 'wifi' : null;

    const args = {
      ip,
      deviceId: state.deviceInfo.deviceId,
      deviceName: state.deviceInfo.deviceName,
      mode: connectionMode,
      exclusiveMode: settings.exclusiveMode,
      jitterConfig: config,
      bitratePreset: settings.bitratePreset,
      customBitrateKbps: settings.customBitrateKbps,
      transport,
    };

    // ADB mode uses TCP transport which takes longer to initialize
    const isAdbMode = connectionMode === 'adb';
    await connectWithRetry(args, isAdbMode ? 4 : 2, isAdbMode ? 500 : 300);

    tauriBridge
      .establishWebsocket({ senderIp: ip, deviceId: state.deviceInfo.deviceId })
      .catch((e) => console.warn('WebSocket setup failed (non-fatal):', e));

    saveLastSender(sender);

    store.getState().dismissError();
    store.getState().patch({
      connectedSender: sender,
      connectingSenderId: null,
      lastConnectedSender: sender,
      status: Status.Connected,
      connectionHealth: 'ok',
      reconnectAttempts: 0,
      isLoading: false,
    });

    fetchAudioSources(sender);
    fetchProcessList(sender);
    startProbing(ip, state.deviceInfo.deviceId);

    // Re-apply persisted audio gain setting
    const gainDb = store.getState().settings.gainDb;
    if (gainDb !== 0) {
      tauriBridge.setAudioGain({ gainDb }).catch((e) => {
        console.warn('Failed to re-apply audio gain:', e);
      });
    }

    toast.getState().show('success', 'Connected');
    return ok(true);
  } catch (e) {
    const error = GemaCastError.failedToStartPlayback(e);
    store.getState().displayError(error);
    store.getState().patch({
      isLoading: false,
      status: Status.Listening,
      connectingSenderId: null,
    });
    return err(error);
  }
}

let isDisconnecting = false;

export async function disconnect(
  forgetSender: boolean = true,
): Promise<Result<true, GemaCastError>> {
  const state = store.getState();

  // Idempotency guard to prevent echo loops and redundant toasts
  if (state.status === Status.Listening || state.status === Status.Idle || isDisconnecting) {
    return ok(true);
  }

  isDisconnecting = true;

  const sender = state.connectedSender;

  if (forgetSender) saveLastSender(null);

  // Optimistically update status to catch echoes during async IPC calls
  store.getState().patch({ status: Status.Listening });
  store.getState().setLoading(true);
  stopProbing();

  try {
    if (!sender) {
      store.getState().patch({
        connectedSender: null,
        lastConnectedSender: forgetSender ? null : state.lastConnectedSender,
        status: Status.Listening,
        connectionHealth: 'ok',
        reconnectAttempts: 0,
        isLoading: false,
        isSuspended: !forgetSender,
      });
      store.getState().resetLatency();
      tauriBridge.notifyStreamingStopped().catch(console.warn);
      if (forgetSender) toast.getState().show('info', 'Disconnected');
      return ok(true);
    }

    try {
      const ip = sender.addr.split(':')[0];
      await tauriBridge.disconnectFromSender({
        ip,
        deviceId: state.deviceInfo.deviceId,
      });
      await new Promise((r) => setTimeout(r, 150));
      tauriBridge.killPlayback().catch(console.warn);
    } catch (e) {
      console.warn('disconnect_from_sender IPC failed:', e);
      await new Promise((r) => setTimeout(r, 150));
      tauriBridge.killPlayback().catch(console.warn);
    }

    store.getState().patch({
      connectedSender: null,
      lastConnectedSender: forgetSender ? null : sender,
      status: Status.Listening,
      connectionHealth: 'ok',
      reconnectAttempts: 0,
      isLoading: false,
      isSuspended: !forgetSender,
      audioSources: [],
      currentAudioSource: { type: 'desktop' },
      senderCapabilities: null,
      processList: [],
    });
    store.getState().resetLatency();
    if (forgetSender) toast.getState().show('info', 'Disconnected');
    return ok(true);
  } finally {
    isDisconnecting = false;
  }
}

export function handleSenderTimeout(deviceId: string) {
  const state = store.getState();
  const list = state.discoveredSenders.filter((s) => s.deviceId !== deviceId);
  store.getState().setDiscoveredSenders(list);

  if (state.connectedSender?.deviceId === deviceId) {
    store.getState().displayError(GemaCastError.senderTimeout());
    store.getState().patch({
      connectionHealth: 'lost',
      status: Status.Listening,
      connectedSender: null,
    });
    store.getState().resetLatency();
    tauriBridge.killPlayback().catch(console.warn);
  }
}

export function handleForceDisconnect(forgetSender: boolean = true) {
  const state = store.getState();
  if (
    state.status === Status.Listening ||
    state.status === Status.Idle ||
    state.status === Status.Connecting
  ) {
    return;
  }

  if (forgetSender) saveLastSender(null);
  store.getState().patch({
    connectedSender: null,
    lastConnectedSender: forgetSender ? null : state.lastConnectedSender,
    status: Status.Listening,
    connectionHealth: 'ok',
    reconnectAttempts: 0,
    isSuspended: !forgetSender,
  });
  store.getState().resetLatency();
  tauriBridge.notifyStreamingStopped().catch(console.warn);
  tauriBridge.killPlayback().catch(console.warn);
}

export async function changeAudioSource(source: AudioSource): Promise<Result<true, GemaCastError>> {
  const state = store.getState();
  const sender = state.connectedSender;
  if (!sender) return err(GemaCastError.from('No sender connected'));

  try {
    const ip = sender.addr.split(':')[0];
    await tauriBridge.changeAudioSource({
      ip,
      deviceId: state.deviceInfo.deviceId,
      source,
    });
    store.getState().setCurrentAudioSource(source);
    toast.getState().show('success', 'Audio source changed');
    return ok(true);
  } catch (e) {
    console.error('Failed to change source:', e);
    return err(GemaCastError.from(e));
  }
}

export function killPlayback() {
  tauriBridge.killPlayback().catch(console.warn);
}

export function useConnection() {
  return {
    connectToSender,
    disconnect,
    handleSenderTimeout,
    handleForceDisconnect,
    changeAudioSource,
    killPlayback,
    fetchProcessList,
  };
}

let probeTimer: ReturnType<typeof setInterval> | null = null;

function startProbing(ip: string, deviceId: string) {
  stopProbing();
  probeTimer = setInterval(() => {
    tauriBridge.probeSender({ ip, deviceId }).catch((e) => {
      console.warn('Failed to probe sender via HTTP:', e);
    });
  }, 5000);
}

function stopProbing() {
  if (probeTimer) {
    clearInterval(probeTimer);
    probeTimer = null;
  }
}

async function fetchAudioSources(sender: DiscoveredSender) {
  try {
    const ip = sender.addr.split(':')[0];
    const result = await tauriBridge.getAudioSources({ ip });
    store.getState().patch({
      audioSources: result[0],
      senderCapabilities: result[1],
    });
  } catch (e) {
    console.warn('Failed to fetch audio sources:', e);
    store.getState().patch({
      audioSources: [{ type: 'desktop' }],
      senderCapabilities: { supportsProcessCapture: false },
    });
  }
}

export async function fetchProcessList(sender: DiscoveredSender) {
  try {
    const ip = sender.addr.split(':')[0];
    const processes = await tauriBridge.getProcessList({ ip });
    store.getState().setProcessList(processes);
  } catch (e) {
    console.warn('Failed to fetch process list:', e);
    store.getState().setProcessList([]);
  }
}
