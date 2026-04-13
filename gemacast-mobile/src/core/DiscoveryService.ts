import { invoke } from '@tauri-apps/api/core';
import { Result, ok, err, DiscoveredSender, Status, ConnectionMode } from '../types';
import { GemaCastError } from '../error';
import { StateHandler } from './StateHandler';

export class DiscoveryService {
  constructor(
    private stateHandler: StateHandler,
    private autoReconnectCallback: (sender: DiscoveredSender) => void,
  ) {}

  public async startListening(mode: ConnectionMode): Promise<Result<true, GemaCastError>> {
    this.stateHandler.setState({ isLoading: true });
    try {
      const state = this.stateHandler.getState();
      await invoke('start_listening_for_senders', { 
        deviceId: state.deviceInfo.deviceId,
        mode 
      });
      this.stateHandler.setState({
        status: Status.Listening,
        isLoading: false,
      });
      return ok(true);
    } catch (e) {
      const error = GemaCastError.failedToStartDiscovery(e);
      this.stateHandler.setState({ error, isLoading: false });
      return err(error);
    }
  }

  public async stopListening(): Promise<Result<true, GemaCastError>> {
    try {
      await invoke('stop_listening_for_senders');
      this.stateHandler.setState({ status: Status.Idle });
      return ok(true);
    } catch (e) {
      const error = GemaCastError.failedToStopDiscovery(e);
      this.stateHandler.displayError(error);
      return err(error);
    }
  }

  public updateDiscoveredSender(sender: DiscoveredSender) {
    const currentState = this.stateHandler.getState();
    const list = [...currentState.discoveredSenders];
    const index = list.findIndex((s) => s.deviceId === sender.deviceId);

    let connectedSender = currentState.connectedSender;
    if (sender.isOffline) {
      if (index >= 0) list.splice(index, 1);

      // If we are actively connected/playing to this sender and it went offline,
      // clear the connection state. The PC may have intentionally stopped broadcasting.
      // We rely on `handleSenderTimeout` for re-connect logic on unintentional drops.
      if (currentState.connectedSender?.deviceId === sender.deviceId) {
        connectedSender = null;
        this.stateHandler.setState({
          discoveredSenders: list,
          connectedSender: null,
          status: Status.Listening,
          connectionHealth: 'ok',
          reconnectAttempts: 0,
        });
        this.stateHandler.updateLatencyInfo(null, null, null, null);
        return;
      }
    } else {
      if (index >= 0) {
        list[index] = sender;
      } else {
        list.push(sender);
      }
      if (connectedSender?.deviceId === sender.deviceId) {
        connectedSender = sender;
      }
    }

    this.stateHandler.setState({
      discoveredSenders: list,
      connectedSender
    });

    if (
      !sender.isOffline &&
      currentState.status === Status.Listening &&
      currentState.lastConnectedSender?.deviceId === sender.deviceId &&
      !currentState.isSuspended
    ) {
      this.autoReconnectCallback(sender);
    }
  }
}
