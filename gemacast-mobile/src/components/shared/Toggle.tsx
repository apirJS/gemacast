type ToggleProps = {
  id: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
  labelOn?: string;
  labelOff?: string;
  disabled?: boolean;
};

export function Toggle({
  id,
  checked,
  onChange,
  labelOn = 'ON',
  labelOff = 'OFF',
  disabled = false,
}: ToggleProps) {
  return (
    <label className="relative inline-flex cursor-pointer">
      <input
        id={id}
        type="checkbox"
        className="peer sr-only"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
      />
      <div
        className={`
          relative flex h-8 w-20 items-center rounded-[20px] border border-border bg-muted p-0.5
          ${disabled ? 'pointer-events-none opacity-50' : ''}
        `}
      >
        <span
          className={`
            z-10 flex-1 text-center text-xs font-bold transition-colors duration-200
            ${checked ? 'text-muted-foreground' : 'text-primary-foreground'}
          `}
        >
          {labelOff}
        </span>
        <span
          className={`
            z-10 flex-1 text-center text-xs font-bold transition-colors duration-200
            ${checked ? 'text-primary-foreground' : 'text-muted-foreground'}
          `}
        >
          {labelOn}
        </span>
        <div
          className={`
            absolute left-[2px] top-[2px] h-[26px] w-[36px] rounded-[16px] bg-primary
            shadow-[0_1px_3px_rgba(0,0,0,0.2)] transition-transform duration-200 ease-[cubic-bezier(0.16,1,0.3,1)]
            ${checked ? 'translate-x-[38px]' : 'translate-x-0'}
          `}
        />
      </div>
    </label>
  );
}
