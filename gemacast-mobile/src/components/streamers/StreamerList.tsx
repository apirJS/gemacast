import { Status } from '../../core/types';
import { useStreamerListController } from '../../controllers';
import { usePullToRefresh } from '../../hooks/use-pull-to-refresh';
import { StreamerCard } from './StreamerCard';
import { EmptyState } from './EmptyState';
import { PullToRefreshIndicator } from './PullToRefreshIndicator';

const PULL_THRESHOLD = 64;

export function StreamerList() {
  const controller = useStreamerListController();

  const {
    ref: scrollRef,
    pull,
    refreshing,
  } = usePullToRefresh<HTMLDivElement>({
    onRefresh: () => controller.refreshStreamers().then(() => undefined),
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
          {controller.isEmpty && <EmptyState />}

          <ul className="flex flex-col gap-2 pb-2 min-h-80" aria-label="Discovered streamers">
            {controller.streamers.map((streamer) => {
              const isConnected = controller.connectedStreamer?.deviceId === streamer.deviceId;
              const isConnecting =
                controller.status === Status.Connecting &&
                controller.connectingStreamerId === streamer.deviceId;
              const isPlaying =
                isConnected &&
                (controller.status === Status.Playing || controller.status === Status.Connected);

              return (
                <StreamerCard
                  key={streamer.deviceId}
                  streamer={streamer}
                  isConnected={isConnected}
                  isConnecting={isConnecting}
                  isPlaying={isPlaying}
                  isLoading={controller.isLoading && (isConnected || isConnecting)}
                  isDisabled={controller.isLoading || controller.status === Status.Connecting}
                  audioSources={isConnected ? controller.audioSources : []}
                  processList={isConnected ? controller.processList : []}
                  streamerCapabilities={isConnected ? controller.streamerCapabilities : null}
                  currentSource={isConnected ? controller.currentAudioSource : { type: 'desktop' }}
                  onToggle={() => void controller.toggleConnection(streamer, isConnected)}
                  onPlayPause={() => void controller.togglePlayback()}
                  onSourceChange={(source) => void controller.changeAudioSource(source)}
                  onRefreshProcesses={() => controller.refreshProcessList(streamer)}
                />
              );
            })}
          </ul>
        </div>
      </div>
    </section>
  );
}
