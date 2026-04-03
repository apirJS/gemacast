import { invoke } from '@tauri-apps/api/core';
import { Result, ok, err, DiscoveredSender, Status } from '../types';
import { GemaCastError } from '../error';
import { StateHandler } from './StateHandler';

const MAX_RECONNECT_ATTEMPTS = 5;
const RECONNECT_BACKOFF_MS = [1_000, 2_000, 4_000, 8_000, 16_000, 30_000];

export class ConnectionService {
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
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
    this.stateHandler.setState({
      isNetworkAvailable: false,
      connectionHealth: 'lost',
      error: GemaCastError.wifiDisconnected(),
    });

    const state = this.stateHandler.getState();
    if (state.connectedSender) {
      this.stateHandler.setState({
        status: Status.Reconnecting,
        connectedSender: null,
      });
      this.stateHandler.updateLatencyInfo(null, null, null, null);
    }
  }

  private handleNetworkOnline() {
    const state = this.stateHandler.getState();
    const isWifiError = state.error?.code === 'NETWORK_WIFI_DISCONNECTED';

    this.stateHandler.setState({
      isNetworkAvailable: true,
      error: isWifiError ? null : state.error,
    });

    if (state.status === Status.Reconnecting && state.lastConnectedSender) {
      this.startReconnectLoop();
    }
  }

  public handleSenderTimeout(senderId: string) {
    const list = this.stateHandler
      .getState()
      .discoveredSenders.filter((s) => s.deviceId !== senderId);
    this.stateHandler.setState({ discoveredSenders: list });

    const state = this.stateHandler.getState();
    if (state.connectedSender?.deviceId !== senderId) return;

    this.stateHandler.setState({
      connectionHealth: 'lost',
      status: Status.Reconnecting,
      connectedSender: null,
      lastConnectedSender: state.connectedSender,
      error: GemaCastError.senderTimeout(),
    });
    this.stateHandler.updateLatencyInfo(null, null, null, null);
    this.startReconnectLoop();
  }

  private startReconnectLoop() {
    this.clearReconnectTimer();

    const state = this.stateHandler.getState();
    const sender = state.lastConnectedSender;
    if (!sender) return;

    const attempt = state.reconnectAttempts;

    if (attempt >= MAX_RECONNECT_ATTEMPTS) {
      this.stateHandler.setState({
        status: Status.Listening,
        reconnectAttempts: 0,
        connectionHealth: 'ok',
        error: GemaCastError.reconnectFailed(),
      });
      return;
    }

    const delayMs =
      RECONNECT_BACKOFF_MS[Math.min(attempt, RECONNECT_BACKOFF_MS.length - 1)];

    this.stateHandler.setState({
      status: Status.Reconnecting,
      reconnectAttempts: attempt + 1,
    });

    this.reconnectTimer = setTimeout(async () => {
      const currentState = this.stateHandler.getState();
      if (!currentState.isNetworkAvailable) return;

      const isDiscovered = currentState.discoveredSenders.some(
        (s) => s.deviceId === sender.deviceId,
      );

      if (!isDiscovered) {
        this.startReconnectLoop();
        return;
      }

      try {
        const ip = sender.addr.split(':')[0];
        await invoke('connect_to_sender', {
          ip,
          deviceId: currentState.deviceInfo.deviceId,
          deviceName: currentState.deviceInfo.deviceName,
        });

        this.stateHandler.setState({
          connectedSender: sender,
          status: Status.Connected,
          connectionHealth: 'ok',
          reconnectAttempts: 0,
          error: null,
        });

        await this.audioResumer();
        this.stateHandler.setState({ status: Status.Playing });
      } catch {
        this.startReconnectLoop();
      }
    }, delayMs);
  }

  private clearReconnectTimer() {
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
  }

  public async connectToSender(
    sender: DiscoveredSender,
  ): Promise<Result<true, GemaCastError>> {
    this.clearReconnectTimer();
    this.stateHandler.setState({ isLoading: true, status: Status.Connecting });
    try {
      const state = this.stateHandler.getState();
      const ip = sender.addr.split(':')[0];
      await invoke('connect_to_sender', {
        ip,
        deviceId: state.deviceInfo.deviceId,
        deviceName: state.deviceInfo.deviceName,
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

  public async disconnect(): Promise<Result<true, GemaCastError>> {
    const state = this.stateHandler.getState();
    const sender = state.connectedSender;
    this.clearReconnectTimer();

    StateHandler.saveLastSender(null);
    this.stateHandler.setState({ isLoading: true });

    if (!sender) {
      this.stateHandler.setState({
        connectedSender: null,
        lastConnectedSender: null,
        status: Status.Listening,
        connectionHealth: 'ok',
        reconnectAttempts: 0,
        isLoading: false,
      });
      this.stateHandler.updateLatencyInfo(null, null, null, null);
      return ok(true);
    }

    try {
      const ip = sender.addr.split(':')[0];
      await invoke('disconnect_from_sender', {
        ip,
        deviceId: state.deviceInfo.deviceId,
      });
    } catch (e) {
      console.warn('disconnect_from_sender IPC failed:', e);
    }

    this.stateHandler.setState({
      connectedSender: null,
      lastConnectedSender: null,
      status: Status.Listening,
      connectionHealth: 'ok',
      reconnectAttempts: 0,
      isLoading: false,
    });
    this.stateHandler.updateLatencyInfo(null, null, null, null);
    return ok(true);
  }

  public handleForceDisconnect(forgetSender: boolean = true) {
    this.clearReconnectTimer();
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
    });
    this.stateHandler.updateLatencyInfo(null, null, null, null);
  }
}
