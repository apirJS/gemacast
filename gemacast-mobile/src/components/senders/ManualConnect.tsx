import { useState } from 'react';
import { ChevronDown } from 'lucide-react';
import { useManualConnect } from '../../hooks/use-manual-connect';
import { useAppStore } from '../../stores/app-store';
import { hasLiveSession } from '../../core/types';

/**
 * Collapsible form for connecting to a sender by IP address.
 * Collapsed by default to keep the main screen clean when discovery works.
 *
 * Hidden outright once a session is live: it is a way to *reach* a PC that
 * discovery missed, so with a stream already running it is dead weight above the
 * card that matters. Disconnecting brings it back.
 */
export function ManualConnect() {
  const { ip, setIp, isLoading, isDisabled, handleConnect } = useManualConnect();
  const [expanded, setExpanded] = useState(false);
  const status = useAppStore((s) => s.status);

  if (hasLiveSession(status)) return null;

  return (
    <div className="surface-card relative z-0 mb-1 rounded-lg overflow-hidden">
      <button
        type="button"
        className="flex w-full items-center justify-between px-4 py-3 text-sm font-medium text-card-foreground transition-colors hover:bg-accent/40"
        onClick={() => setExpanded(!expanded)}
        aria-expanded={expanded}
        aria-controls="manual-connect-form"
      >
        Connect by Address
        <ChevronDown
          className={`
            h-4 w-4 text-muted-foreground transition-transform duration-200
            ${expanded ? 'rotate-180' : 'rotate-0'}
          `}
        />
      </button>

      <div
        id="manual-connect-form"
        className={`
          grid transition-[grid-template-rows] duration-200 ease-out
          ${expanded ? 'grid-rows-[1fr]' : 'grid-rows-[0fr]'}
        `}
      >
        <div className="overflow-hidden">
          <div className="flex gap-2 px-4 pb-4 pt-1">
            <input
              type="text"
              value={ip}
              onChange={(e) => setIp(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && handleConnect()}
              placeholder="192.xx.xx.xx"
              className={`
                flex-1 min-w-0 rounded-[calc(var(--radius-default)-0.2rem)] border border-border bg-background
                px-3 py-1.5 text-[0.875rem] text-foreground outline-none
                placeholder:text-muted-foreground
                focus:border-primary focus:ring-1 focus:ring-primary
              `}
              disabled={isLoading}
              tabIndex={expanded ? 0 : -1}
            />
            <button
              type="button"
              className={`relative inline-flex items-center justify-center whitespace-nowrap rounded-[calc(var(--radius-default)-0.2rem)] border border-border bg-background px-4 py-1.5 text-[0.75rem] font-semibold text-foreground transition-all duration-150 hover:bg-accent disabled:pointer-events-none ${isDisabled && !isLoading ? 'opacity-50' : ''}`}
              onClick={handleConnect}
              disabled={isDisabled}
              tabIndex={expanded ? 0 : -1}
            >
              <span
                className={`transition-opacity duration-150 ${isLoading ? 'opacity-0' : 'opacity-100'}`}
              >
                Connect
              </span>
              {isLoading && (
                <span className="absolute left-1/2 top-1/2 inline-block h-3.5 w-3.5 -translate-x-1/2 -translate-y-1/2 animate-spin rounded-full border-[1.5px] border-current border-t-transparent" />
              )}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
