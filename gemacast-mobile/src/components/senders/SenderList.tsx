import { useCallback } from 'react';
import { useAppStore } from '../../stores/app-store';
import { Status } from '../../core/types';
import type { AudioSource, DiscoveredSender } from '../../core/types';
import { connectToSender, disconnect, changeAudioSource } from '../../hooks/use-connection';
import { SenderCard } from './SenderCard';
import { EmptyState } from './EmptyState';

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
  ].includes(status);

  const isEmpty = senders.length === 0 && isListening;

  const handleToggle = useCallback(async (sender: DiscoveredSender, isConnected: boolean) => {
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
  }, [connectedSender]);

  const handleSourceChange = useCallback((source: AudioSource) => {
    changeAudioSource(source);
  }, []);

  return (
    <section>
      {isEmpty && <EmptyState />}

      <ul className="flex flex-col gap-2" aria-label="Discovered senders">
        {senders.map((sender) => {
          const isConnected = connectedSender?.deviceId === sender.deviceId;
          const isConnecting =
            status === Status.Connecting && connectingSenderId === sender.deviceId;

          return (
            <SenderCard
              key={sender.deviceId}
              sender={sender}
              isConnected={isConnected}
              isConnecting={isConnecting}
              isLoading={isLoading && (isConnected || isConnecting)}
              isDisabled={isLoading || status === Status.Connecting}
              audioSources={isConnected ? audioSources : []}
              processList={isConnected ? processList : []}
              senderCapabilities={isConnected ? senderCapabilities : null}
              currentSource={isConnected ? currentAudioSource : { type: 'desktop' }}
              onToggle={() => handleToggle(sender, isConnected)}
              onSourceChange={handleSourceChange}
            />
          );
        })}
      </ul>
    </section>
  );
}
