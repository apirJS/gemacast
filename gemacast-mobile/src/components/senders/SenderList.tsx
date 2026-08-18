import { useAppStore } from '../../stores/app-store';
import { Status } from '../../core/types';
import type { AudioSource, DiscoveredSender } from '../../core/types';
import { connectToSender, disconnect, changeAudioSource } from '../../hooks/use-connection';
import { refreshSenders } from '../../hooks/use-discovery';
import { startPlayback, stopPlayback } from '../../hooks/use-audio';
import { usePullToRefresh } from '../../hooks/use-pull-to-refresh';
import { SenderCard } from './SenderCard';
import { EmptyState } from './EmptyState';
import { PullToRefreshIndicator } from './PullToRefreshIndicator';

const PULL_THRESHOLD = 64;

export function SenderList() {
  const senders = useAppStore((s) => s.discoveredSenders);
  const status = useAppStore((s) => s.status);
  const connectedSender = useAppStore((s) => s.connectedSender);
  const connectingSenderId = useAppStore((s) => s.connectingSenderId);
  const isLoading = useAppStore((s) => s.isLoading);
  const audioSources = useAppStore((s) => s.audioSources);
  const processList = useAppStore((s) => s.processList);
  const senderCapabilities = useAppStore((s) => s.senderCapabilities);
  const currentAudioSource = useAppStore((s) => s.currentAudioSource);

  const isListening = [
    Status.Listening,
    Status.Connecting,
    Status.Reconnecting,
    Status.Connected,
    Status.Playing,
    Status.Paused,
  ].includes(status);

  const isEmpty = senders.length === 0 && isListening;

  const handleToggle = async (sender: DiscoveredSender, isConnected: boolean) => {
    if (isConnected) {
      await disconnect();
      // Remove manual senders from list on disconnect
      if (sender.deviceId.startsWith('manual-')) {
        const state = useAppStore.getState();
        const newList = state.discoveredSenders.filter((s) => s.deviceId !== sender.deviceId);
        state.setDiscoveredSenders(newList);
      }
    } else {
      if (connectedSender) await disconnect();
      await connectToSender(sender);
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
    onRefresh: () => refreshSenders().then(() => {}),
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

          <ul className="flex flex-col gap-2 pb-2 min-h-80" aria-label="Discovered senders">
            {senders.map((sender) => {
              const isConnected = connectedSender?.deviceId === sender.deviceId;
              const isConnecting =
                status === Status.Connecting && connectingSenderId === sender.deviceId;
              const isPlaying =
                isConnected && (status === Status.Playing || status === Status.Connected);

              return (
                <SenderCard
                  key={sender.deviceId}
                  sender={sender}
                  isConnected={isConnected}
                  isConnecting={isConnecting}
                  isPlaying={isPlaying}
                  isLoading={isLoading && (isConnected || isConnecting)}
                  isDisabled={isLoading || status === Status.Connecting}
                  audioSources={isConnected ? audioSources : []}
                  processList={isConnected ? processList : []}
                  senderCapabilities={isConnected ? senderCapabilities : null}
                  currentSource={isConnected ? currentAudioSource : { type: 'desktop' }}
                  onToggle={() => handleToggle(sender, isConnected)}
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
