import { useManualConnect } from '../../hooks/use-manual-connect';

/**
 * Pure form component for connecting to a sender by IP address.
 * All business logic (validation, probing, connect) lives in useManualConnect.
 */
export function ManualConnect() {
  const { ip, setIp, isLoading, isDisabled, handleConnect } = useManualConnect();

  return (
    <div className="relative z-0 mb-1 flex flex-col gap-3 rounded-[var(--radius-default)] border border-border bg-card p-4 shadow-sm">
      <h3 className="m-0 text-sm font-medium text-card-foreground">Connect by Address</h3>
      <div className="flex gap-2">
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
        />
        <button
          type="button"
          className={`relative inline-flex items-center justify-center whitespace-nowrap rounded-[calc(var(--radius-default)-0.2rem)] border border-border bg-background px-4 py-1.5 text-[0.75rem] font-semibold text-foreground transition-all duration-150 hover:bg-secondary disabled:pointer-events-none ${isDisabled && !isLoading ? 'opacity-50' : ''}`}
          onClick={handleConnect}
          disabled={isDisabled}
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
  );
}
