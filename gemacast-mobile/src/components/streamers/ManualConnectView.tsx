import { ChevronDown } from 'lucide-react';
import { useState } from 'react';

type ManualConnectViewProps = {
  visible: boolean;
  ip: string;
  loading: boolean;
  disabled: boolean;
  onIpChange: (ip: string) => void;
  onConnect: () => void;
};

export function ManualConnectView({
  visible,
  ip,
  loading,
  disabled,
  onIpChange,
  onConnect,
}: ManualConnectViewProps) {
  const [expanded, setExpanded] = useState(false);
  if (!visible) return null;

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
          className={`h-4 w-4 text-muted-foreground transition-transform duration-200 ${expanded ? 'rotate-180' : 'rotate-0'}`}
        />
      </button>

      <div
        id="manual-connect-form"
        className={`grid transition-[grid-template-rows] duration-200 ease-out ${expanded ? 'grid-rows-[1fr]' : 'grid-rows-[0fr]'}`}
      >
        <div className="overflow-hidden">
          <div className="flex gap-2 px-4 pb-4 pt-1">
            <input
              type="text"
              value={ip}
              onChange={(event) => onIpChange(event.target.value)}
              onKeyDown={(event) => event.key === 'Enter' && onConnect()}
              placeholder="192.xx.xx.xx"
              className="flex-1 min-w-0 rounded-[calc(var(--radius-default)-0.2rem)] border border-border bg-background px-3 py-1.5 text-[0.875rem] text-foreground outline-none placeholder:text-muted-foreground focus:border-primary focus:ring-1 focus:ring-primary"
              disabled={loading}
              tabIndex={expanded ? 0 : -1}
            />
            <button
              type="button"
              className={`relative inline-flex items-center justify-center whitespace-nowrap rounded-[calc(var(--radius-default)-0.2rem)] border border-border bg-background px-4 py-1.5 text-[0.75rem] font-semibold text-foreground transition-all duration-150 hover:bg-accent disabled:pointer-events-none ${disabled && !loading ? 'opacity-50' : ''}`}
              onClick={onConnect}
              disabled={disabled}
              tabIndex={expanded ? 0 : -1}
            >
              <span
                className={`transition-opacity duration-150 ${loading ? 'opacity-0' : 'opacity-100'}`}
              >
                Connect
              </span>
              {loading && (
                <span className="absolute left-1/2 top-1/2 inline-block h-3.5 w-3.5 -translate-x-1/2 -translate-y-1/2 animate-spin rounded-full border-[1.5px] border-current border-t-transparent" />
              )}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
