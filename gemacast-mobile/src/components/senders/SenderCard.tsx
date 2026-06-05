import { Monitor, Usb, Play, Pause } from 'lucide-react';
import type { AudioSource, DiscoveredSender, ProcessInfo, SenderCapabilities } from '../../core/types';
import { ProcessSelect } from './ProcessSelect';

type SenderCardProps = {
  sender: DiscoveredSender;
  isConnected: boolean;
  isConnecting: boolean;
  isPlaying: boolean;
  isLoading: boolean;
  isDisabled: boolean;
  audioSources: AudioSource[];
  processList: ProcessInfo[];
  senderCapabilities: SenderCapabilities | null;
  currentSource: AudioSource;
  onToggle: () => void;
  onPlayPause: () => void;
  onSourceChange: (source: AudioSource) => void;
};

/**
 * Pure presentational component for a single sender entry.
 * All business logic (connect/disconnect, play/pause, source changes) is
 * driven by callback props from the parent.
 */
export function SenderCard({
  sender,
  isConnected,
  isConnecting,
  isPlaying,
  isLoading,
  isDisabled,
  audioSources,
  processList,
  currentSource,
  onToggle,
  onPlayPause,
  onSourceChange,
}: SenderCardProps) {
  const isAdb = sender.addr.startsWith('127.0.0.1');
  const showLoading = isLoading && (isConnected || isConnecting);
  const hasSource = isConnected && (audioSources.length > 0 || processList.length > 0);

  return (
    <li
      className={`
        relative flex items-center justify-between gap-4 rounded-[var(--radius-default)] border bg-transparent
        px-5 py-4 transition-all duration-200 animate-[fade-in_200ms_ease-out]
        ${hasSource ? 'flex-wrap' : ''}
        ${isConnected ? 'border-primary shadow-[0_0_0_1px_var(--color-primary)]' : 'border-border hover:border-primary'}
      `}
    >
      <div className={`flex items-center gap-3 overflow-hidden ${hasSource ? 'min-w-0 flex-1' : 'min-w-0'}`}>
        <div className={`flex shrink-0 ${isConnected ? 'text-primary' : 'text-muted-foreground'}`}>
          {isAdb ? <Usb className="h-5 w-5" /> : <Monitor className="h-5 w-5" />}
        </div>

        <div className="flex min-w-0 flex-col gap-0.5">
          <p className="truncate text-sm font-medium text-card-foreground">
            {sender.deviceName}
          </p>
          <p className="truncate text-xs text-muted-foreground">
            {isAdb ? 'ADB (USB Debug)' : sender.addr.split(':')[0]}
          </p>
        </div>
      </div>

      <div className="flex shrink-0 items-center gap-1.5">
        <button
          type="button"
          disabled={isDisabled}
          className={`
            inline-flex shrink-0 items-center justify-center gap-1.5 whitespace-nowrap rounded-[calc(var(--radius-default)-0.2rem)] border border-transparent px-3 py-1.5 text-xs font-semibold transition-all duration-150
            ${showLoading ? 'pointer-events-none' : ''}
            ${
              isConnected
                ? 'bg-destructive text-destructive-foreground hover:opacity-90'
                : 'border-border bg-secondary text-secondary-foreground hover:bg-primary hover:text-primary-foreground'
            }
          `}
          onClick={onToggle}
          aria-label={`${isConnected ? 'Disconnect from' : 'Connect to'} ${sender.deviceName}`}
        >
          {showLoading ? (
            <span className="inline-block h-2.5 w-2.5 animate-spin rounded-full border-[1.5px] border-current border-t-transparent" />
          ) : isConnected ? (
            'Disconnect'
          ) : (
            'Connect'
          )}
        </button>

        {isConnected && (
          <button
            type="button"
            disabled={isDisabled}
            className="inline-flex shrink-0 items-center justify-center rounded-[calc(var(--radius-default)-0.2rem)] border border-border bg-secondary p-1.5 text-secondary-foreground transition-all duration-150 hover:bg-primary hover:text-primary-foreground"
            onClick={onPlayPause}
            aria-label={isPlaying ? `Pause ${sender.deviceName}` : `Resume ${sender.deviceName}`}
          >
            {isPlaying ? <Pause className="h-3.5 w-3.5" /> : <Play className="h-3.5 w-3.5" />}
          </button>
        )}
      </div>

      {hasSource && (
        <ProcessSelect
          audioSources={audioSources}
          processList={processList}
          currentSource={currentSource}
          onSourceChange={onSourceChange}
          sender={sender}
        />
      )}
    </li>
  );
}
