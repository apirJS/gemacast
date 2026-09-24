import { useState, useRef } from 'react';
import { useSettingsController } from '../../controllers';

const MIN_DB = -24;
const MAX_DB = 12;
const STEP = 0.5;
const DEBOUNCE_MS = 100;

function formatDb(db: number): string {
  if (db === 0) return '±0 dB';
  const sign = db > 0 ? '+' : '';
  return `${sign}${db.toFixed(1)} dB`;
}

export function GainSlider() {
  const { settings, update } = useSettingsController();
  const [localDb, setLocalDb] = useState(settings.gainDb);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const handleChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const value = parseFloat(e.target.value);
    setLocalDb(value);

    if (debounceRef.current) clearTimeout(debounceRef.current);
    debounceRef.current = setTimeout(() => {
      void update({ gainDb: value });
    }, DEBOUNCE_MS);
  };

  const handleReset = () => {
    setLocalDb(0);
    void update({ gainDb: 0 });
  };

  // Calculate offset ratios (0 to 1)
  const offset = (localDb - MIN_DB) / (MAX_DB - MIN_DB);
  const zeroOffset = (0 - MIN_DB) / (MAX_DB - MIN_DB);

  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center justify-between">
        <span
          className={`text-sm font-semibold tabular-nums ${
            localDb === 0
              ? 'text-muted-foreground'
              : localDb > 0
                ? 'text-accent-yellow'
                : 'text-accent-aqua'
          }`}
        >
          {formatDb(localDb)}
        </span>
        {localDb !== 0 && (
          <button
            type="button"
            className="text-xs text-muted-foreground hover:text-foreground transition-colors"
            onClick={handleReset}
            aria-label="Reset gain to 0 dB"
          >
            Reset
          </button>
        )}
      </div>

      <div className="relative">
        {/* Center mark at 0 dB, accounting for 20px thumb width */}
        <div
          className="absolute top-1/2 h-3 w-px -translate-y-1/2 bg-muted-foreground/40 z-10 pointer-events-none"
          style={{ left: `calc(10px + (100% - 20px) * ${zeroOffset})` }}
        />

        <input
          type="range"
          min={MIN_DB}
          max={MAX_DB}
          step={STEP}
          value={localDb}
          onChange={handleChange}
          className="gain-slider w-full"
          aria-label="Audio gain"
          style={
            {
              '--fill-pos': `calc(10px + (100% - 20px) * ${offset})`,
              '--zero-pos': `calc(10px + (100% - 20px) * ${zeroOffset})`,
            } as React.CSSProperties
          }
        />
      </div>

      <div className="relative h-4 text-[0.65rem] text-muted-foreground/60">
        <span className="absolute left-0">{MIN_DB} dB</span>
        <span
          className="absolute -translate-x-1/2"
          style={{ left: `calc(10px + (100% - 20px) * ${zeroOffset})` }}
        >
          0 dB
        </span>
        <span className="absolute right-0">+{MAX_DB} dB</span>
      </div>
    </div>
  );
}
