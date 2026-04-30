import { invoke } from '@tauri-apps/api/core';
import { Result, ok, err, DiscoveredSender, Status } from '../types';
import { GemaCastError } from '../error';
import { StateHandler } from './StateHandler';
import { getPresetConfig } from './presets';

export class ConnectionService {
  private audioResumer: () => Promise<void>;

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

    if (state.connectedSender || state.status === Status.Playing) {
      this.stateHandler.setState({
        status: Status.Listening, // Immediately jump to Scanning
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
  }

  public handleSenderTimeout(senderId: string) {
    const currentState = this.stateHandler.getState();

    const list = currentState.discoveredSenders.filter(
      (s) => s.deviceId !== senderId,
    );
    this.stateHandler.setState({ discoveredSenders: list });

    if (currentState.connectedSender?.deviceId === senderId) {
      this.stateHandler.setState({
        connectionHealth: 'lost',
        status: Status.Listening,
        connectedSender: null,
        error: GemaCastError.senderTimeout(),
      });
      this.stateHandler.updateLatencyInfo(null, null, null, null);
      this.killPlayback().catch(console.warn);
    }
  }

  public async connectToSender(
    sender: DiscoveredSender,
  ): Promise<Result<true, GemaCastError>> {
    this.stateHandler.setState({
      isLoading: true,
      status: Status.Connecting,
      isSuspended: false,
    });
    try {
      const state = this.stateHandler.getState();
      const ip = sender.addr.split(':')[0];

      const settings = state.settings;
      const config = getPresetConfig(
        settings.bufferPreset,
        settings.customJitterConfig,
      );

      const transport =
        settings.mode === 'usb'
          ? 'usb'
          : settings.mode === 'wifi'
            ? 'wifi'
            : null;

      await invoke('connect_to_sender', {
        ip,
        deviceId: state.deviceInfo.deviceId,
        deviceName: state.deviceInfo.deviceName,
        mode: settings.mode,
        exclusiveMode: settings.exclusiveMode,
        jitterConfig: config,
        transport,
      });

      StateHandler.saveLastSender(sender);

      this.stateHandler.setState({
        connectedSender: sender,
        lastConnectedSender: sender,
        status: Status.Connected,
        connectionHealth: 'ok',
        reconnectAttempts: 0,
        error: null,
        isLoading: false,
      });

      await this.audioResumer();

      return ok(true);
    } catch (e) {
      const error = GemaCastError.failedToStartPlayback(e);
      this.stateHandler.setState({
        error,
        isLoading: false,
        status: Status.Listening,
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
    });
    this.stateHandler.updateLatencyInfo(null, null, null, null);
    return ok(true);
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
}
