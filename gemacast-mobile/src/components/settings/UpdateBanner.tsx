import { useUpdater } from '../../hooks/use-updater';
import { Download, RefreshCw, CheckCircle, AlertTriangle } from 'lucide-react';

export function UpdateBanner() {
  const { state, startDownload, installUpdate } = useUpdater();

  // Don't render anything when up-to-date or idle.
  if (state.status === 'up-to-date' || state.status === 'idle') {
    return null;
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
          <div className="flex items-center gap-2 min-w-0 flex-1">
            <CheckCircle className="h-4 w-4 shrink-0 text-green-500" />
            <span className="text-[0.85rem] font-medium text-foreground">
              Ready to install v{state.version}
            </span>
          </div>
          <button
            type="button"
            className="flex items-center gap-2 rounded-lg bg-green-600 px-4 py-2 text-[0.85rem] font-medium text-white transition-colors hover:bg-green-700 active:bg-green-800"
            onClick={installUpdate}
          >
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
        <div className="flex items-center gap-2 text-yellow-500">
          <AlertTriangle className="h-4 w-4 shrink-0" />
          <span className="text-[0.8rem]">{state.message}</span>
        </div>
      )}
    </div>
  );
}
