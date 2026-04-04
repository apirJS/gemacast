import { invoke } from '@tauri-apps/api/core';
import { Result, ok, err, Status } from '../types';
import { GemaCastError } from '../error';
import { StateHandler } from './StateHandler';

export class AudioService {
  constructor(private stateHandler: StateHandler) {}

  public async startAudioPlayback(): Promise<Result<true, GemaCastError>> {
    this.stateHandler.setState({ isLoading: true });
    try {
      await invoke('start_audio_playback');
      this.stateHandler.setState({ status: Status.Playing, isLoading: false });
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
      await invoke('stop_audio_playback');
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

  public async setVolume(level: number): Promise<void> {
    this.stateHandler.setVolumeValue(level);
    try {
      await invoke('set_volume', { level: Math.max(0, Math.min(1, level)) });
    } catch (e) {
      console.warn('set_volume IPC failed:', e);
    }
  }

  public async toggleMute(): Promise<void> {
    const level = this.stateHandler.toggleMuteValue();
    try {
      await invoke('set_volume', { level });
    } catch (e) {
      console.warn('set_volume IPC failed:', e);
    }
  }
}
