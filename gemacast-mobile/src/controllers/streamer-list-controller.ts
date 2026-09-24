import { Status, type AudioSource, type DiscoveredStreamer } from '../core/types';
import { useAppStore } from '../stores/app-store';
import { connectionController } from './connection-controller';
import { discoveryController } from './discovery-controller';
import { playbackController } from './playback-controller';

const DISCOVERY_VISIBLE_STATUSES = [
  Status.Listening,
  Status.Connecting,
  Status.Reconnecting,
  Status.Connected,
  Status.Playing,
  Status.Paused,
];

export function useStreamerListController() {
  const streamers = useAppStore((state) => state.discoveredStreamers);
  const status = useAppStore((state) => state.status);
  const connectedStreamer = useAppStore((state) => state.connectedStreamer);
  const connectingStreamerId = useAppStore((state) => state.connectingStreamerId);
  const isLoading = useAppStore((state) => state.isLoading);
  const audioSources = useAppStore((state) => state.audioSources);
  const processList = useAppStore((state) => state.processList);
  const streamerCapabilities = useAppStore((state) => state.streamerCapabilities);
  const currentAudioSource = useAppStore((state) => state.currentAudioSource);

  const toggleConnection = async (
    streamer: DiscoveredStreamer,
    connected: boolean,
  ): Promise<void> => {
    if (connected) {
      await connectionController.disconnect();
      if (streamer.deviceId.startsWith('manual-')) {
        const state = useAppStore.getState();
        state.setDiscoveredStreamers(
          state.discoveredStreamers.filter((item) => item.deviceId !== streamer.deviceId),
        );
      }
      return;
    }

    if (useAppStore.getState().connectedStreamer) await connectionController.disconnect();
    await connectionController.connect(streamer);
  };

  const togglePlayback = async (): Promise<void> => {
    const currentStatus = useAppStore.getState().status;
    if (currentStatus === Status.Playing || currentStatus === Status.Connected) {
      await playbackController.stop();
    } else if (currentStatus === Status.Paused) {
      await playbackController.start();
    }
  };

  return {
    streamers,
    status,
    connectedStreamer,
    connectingStreamerId,
    isLoading,
    audioSources,
    processList,
    streamerCapabilities,
    currentAudioSource,
    isEmpty: streamers.length === 0 && DISCOVERY_VISIBLE_STATUSES.includes(status),
    toggleConnection,
    togglePlayback,
    changeAudioSource: (source: AudioSource) => connectionController.changeAudioSource(source),
    refreshStreamers: () => discoveryController.refresh(),
    refreshProcessList: (streamer: DiscoveredStreamer) =>
      connectionController.refreshProcessList(streamer),
  };
}
