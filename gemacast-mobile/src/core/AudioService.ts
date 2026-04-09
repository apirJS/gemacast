import { invoke } from '@tauri-apps/api/core';
import { Result, ok, err, Status } from '../types';
import { GemaCastError } from '../error';
import { StateHandler } from './StateHandler';

export class AudioService {
  constructor(private stateHandler: StateHandler) {}

  public async startAudioPlayback(): Promise<Result<true, GemaCastError>> {
    this.stateHandler.setState({ isLoading: true });
    try {
      const state = this.stateHandler.getState();
      const sender = state.connectedSender;
      await invoke('start_audio_playback', {
        ip: sender ? sender.addr.split(':')[0] : null,
        deviceId: state.deviceInfo.deviceId,
        deviceName: state.deviceInfo.deviceName,
      });
      const current = this.stateHandler.getState();
      if (current.connectedSender) {
        this.stateHandler.setState({ status: Status.Playing, isLoading: false });
      } else {
        this.stateHandler.setState({ isLoading: false });
      }
      return ok(true);
    } catch (e) {
      const error = GemaCastError.failedToStartPlayback(e);
      this.stateHandler.setState({ error, isLoading: false });
      return err(error);
    }
  }

  public async stopAudioPlayback(): Promise<Result<true, GemaCastError>> {
    this.stateHandler.setState({ isLoading: true });
    try {
      const state = this.stateHandler.getState();
      const sender = state.connectedSender;
      await invoke('stop_audio_playback', {
        ip: sender ? sender.addr.split(':')[0] : null,
        deviceId: state.deviceInfo.deviceId,
      });
      this.stateHandler.setState({
        status: Status.Connected,
        isLoading: false,
      });
      return ok(true);
    } catch (e) {
      const error = GemaCastError.failedToStopPlayback(e);
      this.stateHandler.setState({ error, isLoading: false });
      return err(error);
    }
  }

  public updateAudioActive(isActive: boolean) {
    const state = this.stateHandler.getState();
    if (state.status === Status.Playing || state.status === Status.Connected) {
      this.stateHandler.setState({
        status: isActive ? Status.Playing : Status.Connected,
      });
    }
  }

  public async setRemoteVolume(level: number): Promise<void> {
    const { connectedSender } = this.stateHandler.getState();
    if (!connectedSender) return;

    const clamped = Math.max(0, Math.min(1, level));

    this.stateHandler.setState({
      connectedSender: {
        ...connectedSender,
        volume: clamped,
        isMuted: clamped === 0,
      },
    });

    try {
      await invoke('set_remote_system_volume', {
        ip: connectedSender.addr.split(':')[0],
        deviceId: connectedSender.deviceId,
        level: clamped,
      });
    } catch (e) {
      console.warn('set_remote_system_volume IPC failed:', e);
    }
  }

  public async toggleRemoteMute(): Promise<void> {
    const { connectedSender } = this.stateHandler.getState();
    if (!connectedSender) return;

    const newMuted = !(connectedSender.isMuted ?? false);

    this.stateHandler.setState({
      connectedSender: {
        ...connectedSender,
        isMuted: newMuted,
      },
    });

    try {
      await invoke('set_remote_system_mute', {
        ip: connectedSender.addr.split(':')[0],
        deviceId: connectedSender.deviceId,
        muted: newMuted,
      });
    } catch (e) {
      console.warn('set_remote_system_mute IPC failed:', e);
    }
  }
}
