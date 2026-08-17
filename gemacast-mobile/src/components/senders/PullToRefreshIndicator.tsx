import { RefreshCw } from 'lucide-react';

type PullToRefreshIndicatorProps = {
  /** Current pull distance in px. */
  pull: number;
  /** True while the rescan is in flight. */
  refreshing: boolean;
  /** Distance at which a release triggers a refresh. */
  threshold: number;
};

// The spinner lives just above the scroll edge at rest and slides down to a
// small parked offset as the pull completes. The parent's `overflow-hidden`
// clips it while it sits above the edge, so it reads as sliding in from the
// top rather than floating in the middle of the list.
const HIDDEN_ABOVE = 44; // px the perch sits above the top edge at rest
const PARKED_BELOW = 8; // px below the top edge while refreshing

/**
 * The spinner that rides down with a pull-to-refresh drag. Presentational only:
 * it reads the distance/flag produced by `usePullToRefresh` and never owns state.
 */
export function PullToRefreshIndicator({
  pull,
  refreshing,
  threshold,
}: PullToRefreshIndicatorProps) {
  const progress = Math.min(1, pull / threshold);
  const armed = pull >= threshold;

  // Travel is clamped to the threshold: past it the spinner is parked, not dragged.
  const travel = refreshing ? threshold : Math.min(pull, threshold);
  const y = -HIDDEN_ABOVE + (travel / threshold) * (HIDDEN_ABOVE + PARKED_BELOW);

  return (
    <div
      aria-hidden={!refreshing}
      className="pointer-events-none absolute inset-x-0 top-0 z-10 flex justify-center"
      style={{
        transform: `translateY(${y}px)`,
        opacity: refreshing ? 1 : progress,
        transition: pull === 0 && !refreshing ? 'transform 200ms ease, opacity 200ms ease' : 'none',
      }}
    >
      <span className="flex h-8 w-8 items-center justify-center rounded-full border border-border bg-card shadow-sm">
        <RefreshCw
          className={`h-4 w-4 text-primary ${refreshing ? 'animate-spin' : ''}`}
          style={
            refreshing
              ? undefined
              : { transform: `rotate(${progress * 270}deg)`, opacity: armed ? 1 : 0.7 }
          }
        />
      </span>
    </div>
  );
}
