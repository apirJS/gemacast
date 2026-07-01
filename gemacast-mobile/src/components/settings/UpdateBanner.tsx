import { useUpdater } from '../../hooks/use-updater';
import { Download, RefreshCw, AlertTriangle, RotateCcw } from 'lucide-react';

export function UpdateBanner() {
  const { state, checkForUpdates, startDownload, installUpdate, retry } = useUpdater();

  if (state.status === 'up-to-date' || state.status === 'idle') {
    return (
      <div className="mb-4 flex items-center justify-between rounded-xl border border-border bg-accent/10 p-4">
        <span className="text-[0.85rem] text-muted-foreground">App is up to date</span>
        <button
          onClick={checkForUpdates}
          className="flex items-center gap-2 rounded-lg border border-border px-3 py-1.5 text-[0.8rem] font-medium text-muted-foreground transition-colors hover:bg-accent hover:text-foreground active:bg-accent/80"
        >
          <RefreshCw className="h-3.5 w-3.5" />
          Check
        </button>
      </div>
    );
  }

  return (
    <div className="mb-4 rounded-xl border border-border bg-accent/30 p-4">
      {state.status === 'checking' && (
        <div className="flex items-center gap-3 text-muted-foreground">
          <RefreshCw className="h-4 w-4 animate-spin" />
          <span className="text-[0.85rem]">Checking for updates...</span>
        </div>
      )}

      {state.status === 'available' && (
        <div className="flex items-center justify-between gap-3">
          <div className="min-w-0 flex-1">
            <p className="text-[0.9rem] font-semibold text-foreground">Update Available</p>
            <p className="text-[0.8rem] text-muted-foreground">v{state.version}</p>
          </div>
          <button
            type="button"
            className="flex items-center gap-2 rounded-lg bg-primary px-4 py-2 text-[0.85rem] font-medium text-primary-foreground transition-colors hover:bg-primary/90 active:bg-primary/80"
            onClick={startDownload}
          >
            <Download className="h-4 w-4" />
            Download
          </button>
        </div>
      )}

      {state.status === 'downloading' && (
        <div className="space-y-2">
          <div className="flex items-center justify-between">
            <span className="text-[0.85rem] font-medium text-foreground">Downloading...</span>
            <span className="text-[0.8rem] tabular-nums text-muted-foreground">
              {state.percent}%
            </span>
          </div>
          <div className="h-2 overflow-hidden rounded-full bg-accent">
            <div
              className="h-full rounded-full bg-primary transition-all duration-300 ease-out"
              style={{ width: `${state.percent}%` }}
            />
          </div>
        </div>
      )}

      {state.status === 'ready' && (
        <div className="flex items-center justify-between gap-3">
          <div className="min-w-0 flex-1">
            <p className="text-[0.9rem] font-semibold text-foreground">Ready to Install</p>
            <p className="text-[0.8rem] text-muted-foreground">v{state.version}</p>
          </div>
          <button
            type="button"
            className="flex items-center gap-2 rounded-lg bg-primary px-4 py-2 text-[0.85rem] font-medium text-primary-foreground transition-colors hover:bg-primary/90 active:bg-primary/80"
            onClick={installUpdate}
          >
            <Download className="h-4 w-4" />
            Install
          </button>
        </div>
      )}

      {state.status === 'installing' && (
        <div className="flex items-center gap-3 text-muted-foreground">
          <RefreshCw className="h-4 w-4 animate-spin" />
          <span className="text-[0.85rem]">Opening installer...</span>
        </div>
      )}

      {state.status === 'error' && (
        <div className="flex items-center justify-between gap-2">
          <div className="flex items-center gap-2 min-w-0 flex-1 text-yellow-500">
            <AlertTriangle className="h-4 w-4 shrink-0" />
            <span className="text-[0.8rem] truncate">{state.errorMessage}</span>
          </div>
          <button
            type="button"
            className="flex items-center gap-2 rounded-lg border border-border px-3 py-1.5 text-[0.8rem] font-medium text-muted-foreground transition-colors hover:bg-accent hover:text-foreground active:bg-accent/80"
            onClick={retry}
          >
            <RotateCcw className="h-3.5 w-3.5" />
            Retry
          </button>
        </div>
      )}
    </div>
  );
}
