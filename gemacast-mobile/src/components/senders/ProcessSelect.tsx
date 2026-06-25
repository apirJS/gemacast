import { useState, useMemo, useCallback } from 'react';
import { Monitor, Volume2, Settings, ChevronDown } from 'lucide-react';
import type { AudioSource, ProcessInfo, DiscoveredSender } from '../../core/types';
import { useConnection } from '../../hooks/use-connection';

type ProcessSelectProps = {
  audioSources: AudioSource[];
  processList: ProcessInfo[];
  currentSource: AudioSource;
  onSourceChange: (source: AudioSource) => void;
  sender: DiscoveredSender;
  supportsProcessCapture: boolean;
};

export function ProcessSelect({
  audioSources,
  processList,
  currentSource,
  onSourceChange,
  sender,
  supportsProcessCapture,
}: ProcessSelectProps) {
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState('');
  const [isRefreshing, setIsRefreshing] = useState(false);
  const { fetchProcessList } = useConnection();

  const sortedProcesses = useMemo(() => {
    return [...processList].sort((a, b) => {
      if (a.hasAudioSession !== b.hasAudioSession) {
        return a.hasAudioSession ? -1 : 1;
      }
      return a.name.localeCompare(b.name);
    });
  }, [processList]);

  const filteredProcesses = useMemo(() => {
    if (!search.trim()) return sortedProcesses;
    const q = search.toLowerCase();
    return sortedProcesses.filter((p) => p.name.toLowerCase().includes(q));
  }, [sortedProcesses, search]);

  const currentLabel = useMemo(() => {
    if (currentSource.type === 'desktop') {
      return (
        <div className="flex min-w-0 items-center gap-1.5">
          <Monitor className="h-3.5 w-3.5 shrink-0" />
          <span className="truncate">Desktop Audio</span>
        </div>
      );
    }
    return (
      <div className="flex min-w-0 items-center gap-1.5">
        {currentSource.hasAudioSession ? (
          <Volume2 className="h-3.5 w-3.5 shrink-0" />
        ) : (
          <Settings className="h-3.5 w-3.5 shrink-0" />
        )}
        <span className="truncate">
          {currentSource.name} (PID: {currentSource.pid})
        </span>
      </div>
    );
  }, [currentSource]);

  const handleSelect = useCallback(
    (source: AudioSource) => {
      onSourceChange(source);
      setOpen(false);
      setSearch('');
    },
    [onSourceChange],
  );

  const hasDesktop = audioSources.some((s) => s.type === 'desktop');

  return (
    <div className="relative flex w-full items-center gap-2 pt-2">
      <span className="shrink-0 whitespace-nowrap text-[0.7rem] font-medium text-muted-foreground">
        Source:
      </span>
      <button
        type="button"
        className={`
          flex min-w-0 flex-1 items-center gap-1.5 rounded-[calc(var(--radius-default)-0.2rem)]
          border border-border bg-secondary px-2 py-1 text-[0.7rem] font-medium text-secondary-foreground
          transition-colors hover:border-primary focus-visible:border-primary focus-visible:shadow-[0_0_0_1px_var(--color-primary)] focus-visible:outline-none
        `}
        onClick={() => setOpen(!open)}
      >
        <div className="flex min-w-0 flex-1 items-center gap-1.5">{currentLabel}</div>
        <span
          className={`shrink-0 text-muted-foreground transition-transform duration-200 ${open ? 'rotate-180' : ''}`}
        >
          <ChevronDown className="h-2.5 w-2.5" />
        </span>
      </button>

      {open && (
        <div className="absolute left-0 right-0 top-full z-50 mt-1 overflow-hidden rounded-[var(--radius-default)] border border-border bg-card shadow-[0_8px_24px_rgba(0,0,0,0.12),0_2px_8px_rgba(0,0,0,0.08)] animate-[fade-in_150ms_ease-out]">
          <div className="flex items-stretch border-b border-border">
            <input
              type="text"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="Search process..."
              className={`
                flex-1 min-w-0 border-none bg-card px-[0.6rem] py-2 text-[0.7rem]
                text-card-foreground outline-none focus:bg-secondary
              `}
            />
            <button
              type="button"
              aria-label="Refresh process list"
              className={`
                flex w-8 shrink-0 items-center justify-center border-l border-border bg-transparent
                p-[0.4rem] text-muted-foreground transition-colors hover:bg-accent hover:text-card-foreground
              `}
              onClick={async (e) => {
                e.stopPropagation();
                if (isRefreshing) return;
                setIsRefreshing(true);
                try {
                  await Promise.all([
                    fetchProcessList(sender),
                    new Promise((r) => setTimeout(r, 600)),
                  ]);
                } finally {
                  setIsRefreshing(false);
                }
              }}
            >
              <svg
                xmlns="http://www.w3.org/2000/svg"
                width="14"
                height="14"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="2"
                strokeLinecap="round"
                strokeLinejoin="round"
                className={isRefreshing ? 'animate-spin' : ''}
              >
                <path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" />
                <path d="M3 3v5h5" />
              </svg>
            </button>
          </div>

          <div className="max-h-[16rem] min-h-[8rem] overflow-y-auto py-1">
            {hasDesktop && (
              <button
                type="button"
                className={`
                  flex w-full items-center px-3 py-2 text-left text-xs transition-colors
                  ${currentSource.type === 'desktop' ? 'bg-accent text-accent-foreground' : 'hover:bg-secondary'}
                `}
                onClick={() => handleSelect({ type: 'desktop' })}
              >
                <Monitor className="mr-2 h-3.5 w-3.5 shrink-0" /> Desktop Audio
              </button>
            )}

            {filteredProcesses.map((proc) => {
              const isSelected = currentSource.type === 'process' && currentSource.pid === proc.pid;
              const isProcessDisabled = !supportsProcessCapture;
              return (
                <button
                  key={proc.pid}
                  type="button"
                  disabled={isProcessDisabled}
                  title={
                    isProcessDisabled
                      ? 'Per-process capture is not available on this PC'
                      : undefined
                  }
                  className={`
                    flex w-full items-center gap-2 px-3 py-2 text-left text-xs transition-colors
                    ${isProcessDisabled ? 'cursor-not-allowed opacity-40' : ''}
                    ${isSelected && !isProcessDisabled ? 'bg-accent text-accent-foreground' : isProcessDisabled ? '' : 'hover:bg-secondary'}
                  `}
                  onClick={() =>
                    handleSelect({
                      type: 'process',
                      pid: proc.pid,
                      name: proc.name,
                      hasAudioSession: proc.hasAudioSession,
                    })
                  }
                >
                  <span className="w-5 shrink-0">
                    {proc.hasAudioSession ? (
                      <Volume2 className="h-3.5 w-3.5" />
                    ) : (
                      <Settings className="h-3.5 w-3.5 text-muted-foreground" />
                    )}
                  </span>
                  <span className="truncate">{proc.name}</span>
                  <span className="ml-auto shrink-0 text-[10px] text-muted-foreground">
                    PID: {proc.pid}
                  </span>
                </button>
              );
            })}

            {!supportsProcessCapture && filteredProcesses.length > 0 && (
              <p className="border-t border-border px-3 py-2 text-[10px] text-muted-foreground">
                Per-process capture unavailable on this PC
              </p>
            )}

            {filteredProcesses.length === 0 && (
              <p className="px-3 py-2 text-xs text-muted-foreground">No processes found</p>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
