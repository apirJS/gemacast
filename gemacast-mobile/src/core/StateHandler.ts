import {
  AppState,
  StateSubscriber,
  Status,
  DeviceInfo,
  ConnectionHealth,
  DiscoveredSender,
} from '../types';
import { GemaCastError } from '../error';

const LS_LAST_SENDER = 'gemacast_last_sender';

export class StateHandler {
  private state: AppState;
  private subscribers: StateSubscriber[] = [];

  constructor(deviceInfo: DeviceInfo) {
    const lastConnectedSender = StateHandler.loadLastSender();

    this.state = {
      deviceInfo,
      status: Status.Idle,
      discoveredSenders: [],
      connectedSender: null,
      lastConnectedSender,
      error: null,
      connectionHealth: 'ok',
      isNetworkAvailable: navigator.onLine,
      isLoading: false,
      isSuspended: false,
      reconnectAttempts: 0,
      latency: { current: null, avg: null, max: null, min: null },
    };
  }

  public getState(): AppState {
    return this.state;
  }

  public subscribe(callback: StateSubscriber): () => void {
    this.subscribers.push(callback);
    callback(this.state);

    return () => {
      this.subscribers = this.subscribers.filter((sub) => sub !== callback);
    };
  }

  public setState(partial: Partial<AppState>) {
    this.state = { ...this.state, ...partial };
    this.subscribers.forEach((cb) => cb(this.state));
  }

  public displayError(error: string | GemaCastError) {
    this.setState({
      error: error instanceof GemaCastError ? error : GemaCastError.from(error),
    });
  }

  public dismissError() {
    this.setState({ error: null });
  }

  public setConnectionHealth(health: ConnectionHealth) {
    this.setState({ connectionHealth: health });
  }

  public updateLatencyInfo(
    current: number | null,
    avg: number | null,
    max: number | null,
    min: number | null,
  ) {
    this.setState({
      latency: { current, avg, max, min },
    });
  }

  public static loadLastSender(): DiscoveredSender | null {
    try {
      const raw = localStorage.getItem(LS_LAST_SENDER);
      return raw ? (JSON.parse(raw) as DiscoveredSender) : null;
    } catch {
      return null;
    }
  }

  public static saveLastSender(sender: DiscoveredSender | null) {
    if (sender) {
      localStorage.setItem(LS_LAST_SENDER, JSON.stringify(sender));
    } else {
      localStorage.removeItem(LS_LAST_SENDER);
    }
  }
}
