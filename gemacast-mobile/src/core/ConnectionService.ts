import { invoke } from '@tauri-apps/api/core';
import {
  Result,
  ok,
  err,
  DiscoveredSender,
  Status,
  AudioSource,
  SenderCapabilities,
  ProcessInfo,
} from '../types';
import { GemaCastError } from '../error';
import { StateHandler } from './StateHandler';
import { getPresetConfig } from './presets';
import { toastManager } from '../dom/toast';

export class ConnectionService {
  private audioResumer: () => Promise<void>;
  private probeTimer: number | null = null;

  constructor(
    private stateHandler: StateHandler,
    audioResumer: () => Promise<void>,
  ) {
    this.audioResumer = audioResumer;
    this.registerNetworkListeners();
  }

  private registerNetworkListeners() {
    window.addEventListener('online', () => this.handleNetworkOnline());
    window.addEventListener('offline', () => this.handleNetworkOffline());
  }

  private handleNetworkOffline() {
    const state = this.stateHandler.getState();

    this.stateHandler.setState({
      isNetworkAvailable: false,
      connectionHealth: 'lost',
      discoveredSenders: [], // Instantaneously clear the sender list
    });

    toastManager.showWarning('Network offline');

    if (state.connectedSender || state.status === Status.Playing) {
      this.stateHandler.setState({
        status: Status.Listening,
        connectedSender: null,
      });
      this.stateHandler.updateLatencyInfo(null, null, null, null);
    }
  }

  private handleNetworkOnline() {
    this.stateHandler.setState({
      isNetworkAvailable: true,
      error: null,
    });
    toastManager.showInfo('Network online');
  }

  public handleSenderTimeout(deviceId: string) {
    const currentState = this.stateHandler.getState();

    const list = currentState.discoveredSenders.filter(
      (s) => s.deviceId !== deviceId,
    );
    this.stateHandler.setState({ discoveredSenders: list });

    if (currentState.connectedSender?.deviceId === deviceId) {
      this.stateHandler.setState({
        connectionHealth: 'lost',
        status: Status.Listening,
        connectedSender: null,
        error: GemaCastError.senderTimeout(),
      });
      this.stateHandler.updateLatencyInfo(null, null, null, null);
      this.killPlayback().catch(console.warn);

      toastManager.showWarning('Connection lost');
    }
  }

  public async connectToSender(
    sender: DiscoveredSender,
  ): Promise<Result<true, GemaCastError>> {
    this.stateHandler.setState({
      isLoading: true,
      status: Status.Connecting,
      isSuspended: false,
      connectingSenderId: sender.deviceId,
    });
    try {
      const state = this.stateHandler.getState();
      const ip = sender.addr.split(':')[0];

      const settings = state.settings;
      const config = getPresetConfig(
        settings.bufferPreset,
        settings.customJitterConfig,
      );

      const isManual = sender.deviceId.startsWith('manual-');
      const connectionMode = isManual ? 'wifi' : settings.mode;

      const transport =
        connectionMode === 'usb'
          ? 'usb'
          : connectionMode === 'wifi'
            ? 'wifi'
            : null;

      await invoke('connect_to_sender', {
        ip,
        deviceId: state.deviceInfo.deviceId,
        deviceName: state.deviceInfo.deviceName,
        mode: connectionMode,
        exclusiveMode: settings.exclusiveMode,
        jitterConfig: config,
        transport,
      });

      // Establish WebSocket so the PC can push disconnect events to us.
      // Fire-and-forget: a WS failure must never abort the audio connection.
      invoke('establish_websocket', {
        senderIp: ip,
        deviceId: state.deviceInfo.deviceId,
      }).catch((e) => console.warn('WebSocket setup failed (non-fatal):', e));

      StateHandler.saveLastSender(sender);

      this.stateHandler.setState({
        connectedSender: sender,
        connectingSenderId: null,
        lastConnectedSender: sender,
        status: Status.Connected,
        connectionHealth: 'ok',
        reconnectAttempts: 0,
        error: null,
        isLoading: false,
      });

      await this.audioResumer();

      this.fetchAudioSources(sender).catch(console.warn);
      this.fetchProcessList(sender).catch(console.warn);

      this.startProbeTimer(ip, state.deviceInfo.deviceId);

      toastManager.showSuccess('Connected');
      return ok(true);
    } catch (e) {
      const error = GemaCastError.failedToStartPlayback(e);
      this.stateHandler.setState({
        error,
        isLoading: false,
        status: Status.Listening,
        connectingSenderId: null,
      });
      return err(error);
    }
  }

  public async disconnect(
    forgetSender: boolean = true,
  ): Promise<Result<true, GemaCastError>> {
    const state = this.stateHandler.getState();
    const sender = state.connectedSender;

    if (forgetSender) {
      StateHandler.saveLastSender(null);
    }
    this.stateHandler.setState({ isLoading: true });
    this.stopProbeTimer();

    if (!sender) {
      this.stateHandler.setState({
        connectedSender: null,
        lastConnectedSender: forgetSender ? null : state.lastConnectedSender,
        status: Status.Listening,
        connectionHealth: 'ok',
        reconnectAttempts: 0,
        isLoading: false,
        isSuspended: !forgetSender,
      });
      this.stateHandler.updateLatencyInfo(null, null, null, null);
      await invoke('notify_streaming_stopped').catch(console.warn);
      if (forgetSender) toastManager.showInfo('Disconnected');
      return ok(true);
    }

    try {
      const ip = sender.addr.split(':')[0];
      await invoke('disconnect_from_sender', {
        ip,
        deviceId: state.deviceInfo.deviceId,
      });
      this.killPlayback().catch(console.warn);
    } catch (e) {
      console.warn('disconnect_from_sender IPC failed:', e);
      this.killPlayback().catch(console.warn);
    }

    this.stateHandler.setState({
      connectedSender: null,
      lastConnectedSender: forgetSender ? null : sender,
      status: Status.Listening,
      connectionHealth: 'ok',
      reconnectAttempts: 0,
      isLoading: false,
      isSuspended: !forgetSender,
      audioSources: [],
      senderCapabilities: null,
      processList: [],
    });
    this.stateHandler.updateLatencyInfo(null, null, null, null);
    if (forgetSender) toastManager.showInfo('Disconnected');
    return ok(true);
  }

  private async fetchAudioSources(sender: DiscoveredSender): Promise<void> {
    try {
      const ip = sender.addr.split(':')[0];
      const result = await invoke<[AudioSource[], SenderCapabilities]>(
        'get_audio_sources',
        { ip },
      );
      this.stateHandler.setState({
        audioSources: result[0],
        senderCapabilities: result[1],
      });
    } catch (e) {
      console.warn('Failed to fetch audio sources:', e);
      // Default to desktop-only if fetch fails
      this.stateHandler.setState({
        audioSources: [{ type: 'desktop' }],
        senderCapabilities: { supportsProcessCapture: false },
      });
    }
  }

  public async changeAudioSource(
    source: AudioSource,
  ): Promise<Result<true, GemaCastError>> {
    const state = this.stateHandler.getState();
    const sender = state.connectedSender;
    if (!sender) return err(GemaCastError.from('No sender connected'));

    try {
      const ip = sender.addr.split(':')[0];
      await invoke('change_audio_source', {
        ip,
        deviceId: state.deviceInfo.deviceId,
        source,
      });
      toastManager.showSuccess('Audio source changed');
      return ok(true);
    } catch (e: any) {
      console.error('Failed to change source:', e);
      return err(GemaCastError.from(e));
    }
  }

  public async fetchProcessList(sender: DiscoveredSender): Promise<void> {
    try {
      const ip = sender.addr.split(':')[0];
      const processes = await invoke<ProcessInfo[]>('get_process_list', { ip });
      this.stateHandler.setState({ processList: processes });
    } catch (e) {
      console.warn('Failed to fetch process list:', e);
      this.stateHandler.setState({ processList: [] });
    }
  }

  public handleForceDisconnect(forgetSender: boolean = true) {
    if (forgetSender) {
      StateHandler.saveLastSender(null);
    }
    const state = this.stateHandler.getState();
    this.stateHandler.setState({
      connectedSender: null,
      lastConnectedSender: forgetSender ? null : state.lastConnectedSender,
      status: Status.Listening,
      connectionHealth: 'ok',
      reconnectAttempts: 0,
      isSuspended: !forgetSender,
    });
    this.stateHandler.updateLatencyInfo(null, null, null, null);
    invoke('notify_streaming_stopped').catch(console.warn);

    // Stop the backend task on forced disconnects
    this.killPlayback().catch(console.warn);
  }

  /**
   * Forcefully kills the backend playback task.
   */
  public async killPlayback(): Promise<void> {
    try {
      await invoke('kill_playback');
    } catch (e) {
      console.warn('kill_playback IPC failed:', e);
    }
  }

  private startProbeTimer(ip: string, deviceId: string) {
    this.stopProbeTimer();
    this.probeTimer = window.setInterval(() => {
      invoke('probe_sender', { ip, deviceId }).catch((e) => {
        console.warn('Failed to probe sender via HTTP:', e);
      });
    }, 5000);
  }

  private stopProbeTimer() {
    if (this.probeTimer) {
      clearInterval(this.probeTimer);
      this.probeTimer = null;
    }
  }
}
