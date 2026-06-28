import { invoke } from '@tauri-apps/api/core';
import type {
  AudioSource,
  BitratePreset,
  ConnectionMode,
  JitterConfig,
  ProcessInfo,
  SenderCapabilities,
} from './types';

export function resolveBitrate(preset: BitratePreset, customKbps: number): number | null {
  if (preset === 'raw') return null;
  if (preset === 'custom') return customKbps * 1000;
  return parseInt(preset, 10) * 1000;
}

export type ConnectArgs = {
  ip: string;
  deviceId: string;
  deviceName: string;
  mode: ConnectionMode | string;
  exclusiveMode: boolean;
  jitterConfig: JitterConfig;
  bitratePreset: BitratePreset;
  customBitrateKbps: number;
  transport: string | null;
};

export type DisconnectArgs = {
  ip: string;
  deviceId: string;
};

export type PlaybackArgs = {
  ip: string | null;
  deviceId: string;
  deviceName?: string;
};

export const tauriBridge = {
  checkForUpdate: () =>
    invoke<{ version: string; downloadUrl: string; sha256: string | null } | null>(
      'check_for_update',
    ),

  downloadUpdate: (args: { url: string; sha256?: string | null }) =>
    invoke<string>('download_update', args),

  installApk: (args: { path: string }) => invoke('install_apk', args),

  cleanupStaleUpdates: () => invoke('cleanup_stale_updates'),

  connectToSender: (args: ConnectArgs) =>
    invoke('connect_to_sender', {
      ip: args.ip,
      deviceId: args.deviceId,
      deviceName: args.deviceName,
      mode: args.mode,
      exclusiveMode: args.exclusiveMode,
      jitterConfig: args.jitterConfig,
      bitrate: resolveBitrate(args.bitratePreset, args.customBitrateKbps),
      transport: args.transport,
    }),

  disconnectFromSender: (args: DisconnectArgs) => invoke('disconnect_from_sender', args),

  startAudioPlayback: (args: PlaybackArgs) => invoke('start_audio_playback', args),

  stopAudioPlayback: (args: Omit<PlaybackArgs, 'deviceName'>) =>
    invoke('stop_audio_playback', args),

  killPlayback: () => invoke('kill_playback'),

  notifyStreamingStopped: () => invoke('notify_streaming_stopped'),

  startListeningForSenders: (args: { deviceId: string; mode: ConnectionMode }) =>
    invoke('start_listening_for_senders', args),

  stopListeningForSenders: () => invoke('stop_listening_for_senders'),

  getNetworkState: () =>
    invoke<{
      localIp: string;
      networkId: string;
      modes: { wifi: boolean; usb: boolean; adb: boolean };
    }>('get_network_state'),

  getLocalIp: () => invoke<string>('get_local_ip'),

  getNetworkIdentifier: () => invoke<string>('get_network_identifier'),

  getConnectionStatus: () =>
    invoke<{ wifi: boolean; usb: boolean; adb: boolean }>('get_connection_status'),

  getAudioSources: (args: { ip: string }) =>
    invoke<[AudioSource[], SenderCapabilities]>('get_audio_sources', args),

  changeAudioSource: (args: { ip: string; deviceId: string; source: AudioSource }) =>
    invoke('change_audio_source', args),

  getProcessList: (args: { ip: string }) => invoke<ProcessInfo[]>('get_process_list', args),

  updateJitterConfig: (args: { jitterConfig: JitterConfig }) =>
    invoke('update_jitter_config', args),

  changeAudioBitrate: (args: { ip: string; deviceId: string; bitrate: number | null }) =>
    invoke('change_audio_bitrate', args),

  probeSender: (args: { ip: string; deviceId: string }) => invoke('probe_sender', args),

  establishWebsocket: (args: { senderIp: string; deviceId: string }) =>
    invoke('establish_websocket', args),

  setAudioGain: (args: { gainDb: number }) => invoke('set_audio_gain', args),
};
