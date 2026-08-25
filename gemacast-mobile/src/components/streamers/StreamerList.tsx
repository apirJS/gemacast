import { useAppStore } from '../../stores/app-store';
import { Status } from '../../core/types';
import type { AudioSource, DiscoveredStreamer } from '../../core/types';
import { connectToStreamer, disconnect, changeAudioSource } from '../../hooks/use-connection';
import { refreshStreamers } from '../../hooks/use-discovery';
import { startPlayback, stopPlayback } from '../../hooks/use-audio';
import { usePullToRefresh } from '../../hooks/use-pull-to-refresh';
import { StreamerCard } from './StreamerCard';
import { EmptyState } from './EmptyState';
import { PullToRefreshIndicator } from './PullToRefreshIndicator';

const PULL_THRESHOLD = 64;

export function StreamerList() {
  const streamers = useAppStore((s) => s.discoveredStreamers);
  const status = useAppStore((s) => s.status);
  const connectedStreamer = useAppStore((s) => s.connectedStreamer);
  const connectingStreamerId = useAppStore((s) => s.connectingStreamerId);
  const isLoading = useAppStore((s) => s.isLoading);
  const audioSources = useAppStore((s) => s.audioSources);
  const processList = useAppStore((s) => s.processList);
  const streamerCapabilities = useAppStore((s) => s.streamerCapabilities);
  const currentAudioSource = useAppStore((s) => s.currentAudioSource);

  const isListening = [
    Status.Listening,
    Status.Connecting,
    Status.Reconnecting,
    Status.Connected,
    Status.Playing,
    Status.Paused,
  ].includes(status);

  const isEmpty = streamers.length === 0 && isListening;

  const handleToggle = async (streamer: DiscoveredStreamer, isConnected: boolean) => {
    if (isConnected) {
      await disconnect();
      // Remove manual streamers from list on disconnect
      if (streamer.deviceId.startsWith('manual-')) {
        const state = useAppStore.getState();
        const newList = state.discoveredStreamers.filter((s) => s.deviceId !== streamer.deviceId);
        state.setDiscoveredStreamers(newList);
      }
    } else {
      if (connectedStreamer) await disconnect();
      await connectToStreamer(streamer);
    }
  };

  const handlePlayPause = async () => {
    const currentStatus = useAppStore.getState().status;
    if (currentStatus === Status.Playing || currentStatus === Status.Connected) {
      await stopPlayback();
    } else if (currentStatus === Status.Paused) {
      await startPlayback();
    }
  };

  const handleSourceChange = (source: AudioSource) => {
    changeAudioSource(source);
  };

  const {
    ref: scrollRef,
    pull,
    refreshing,
  } = usePullToRefresh<HTMLDivElement>({
    onRefresh: () => refreshStreamers().then(() => {}),
    threshold: PULL_THRESHOLD,
  });

  return (
    <section className="relative flex-1 min-h-0 flex flex-col overflow-hidden">
      <PullToRefreshIndicator pull={pull} refreshing={refreshing} threshold={PULL_THRESHOLD} />

      <div
        ref={scrollRef}
        className="flex-1 min-h-0 overflow-y-auto overscroll-contain hide-scrollbar"
      >
        <div
          style={{
            transform: `translateY(${refreshing ? PULL_THRESHOLD : pull}px)`,
            transition: pull > 0 || refreshing ? 'none' : 'transform 200ms ease',
          }}
        >
          {isEmpty && <EmptyState />}

          <ul className="flex flex-col gap-2 pb-2 min-h-80" aria-label="Discovered streamers">
            {streamers.map((streamer) => {
              const isConnected = connectedStreamer?.deviceId === streamer.deviceId;
              const isConnecting =
                status === Status.Connecting && connectingStreamerId === streamer.deviceId;
              const isPlaying =
                isConnected && (status === Status.Playing || status === Status.Connected);

              return (
                <StreamerCard
                  key={streamer.deviceId}
                  streamer={streamer}
                  isConnected={isConnected}
                  isConnecting={isConnecting}
                  isPlaying={isPlaying}
                  isLoading={isLoading && (isConnected || isConnecting)}
                  isDisabled={isLoading || status === Status.Connecting}
                  audioSources={isConnected ? audioSources : []}
                  processList={isConnected ? processList : []}
                  streamerCapabilities={isConnected ? streamerCapabilities : null}
                  currentSource={isConnected ? currentAudioSource : { type: 'desktop' }}
                  onToggle={() => handleToggle(streamer, isConnected)}
                  onPlayPause={handlePlayPause}
                  onSourceChange={handleSourceChange}
                />
              );
            })}
          </ul>
        </div>
      </div>
    </section>
  );
}
