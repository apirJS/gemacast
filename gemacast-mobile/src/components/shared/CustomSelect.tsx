import { useState, useRef, useCallback } from 'react';

export type SelectOption<T extends string = string> = {
  value: T;
  label: string;
  description?: string;
  disabled?: boolean;
};

type CustomSelectProps<T extends string = string> = {
  id: string;
  options: SelectOption<T>[];
  value: T;
  onChange: (value: T) => void;
  renderOption?: (option: SelectOption<T>, isSelected: boolean) => React.ReactNode;
};

export function CustomSelect<T extends string = string>({
  id,
  options,
  value,
  onChange,
  renderOption,
}: CustomSelectProps<T>) {
  const [open, setOpen] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);

  const selectedOption = options.find((o) => o.value === value);

  const handleSelect = useCallback(
    (optionValue: T) => {
      onChange(optionValue);
      setOpen(false);
    },
    [onChange],
  );

  const handleBlur = useCallback((e: React.FocusEvent) => {
    if (!containerRef.current?.contains(e.relatedTarget as Node)) {
      setOpen(false);
    }
  }, []);

  return (
    <div
      id={id}
      ref={containerRef}
      className="relative"
      onBlur={handleBlur}
    >
      <button
        type="button"
        className={`
          flex w-full items-center justify-between rounded-lg
          border border-border bg-background p-3 text-base text-foreground
          transition-colors hover:bg-accent
        `}
        onClick={() => setOpen(!open)}
        aria-expanded={open}
        aria-haspopup="listbox"
      >
        <span>{selectedOption?.label ?? 'Select...'}</span>
        <span
          className={`ml-2 text-xs text-muted-foreground transition-transform duration-200 ${
            open ? 'rotate-180' : ''
          }`}
        >
          ▼
        </span>
      </button>

      {open && (
        <div
          role="listbox"
          className={`
            absolute z-50 mt-1 w-full overflow-hidden rounded-lg
            border border-border bg-background shadow-[0_4px_12px_rgba(0,0,0,0.2)]
            animate-[fade-in_150ms_ease-out]
          `}
        >
          <div className="max-h-64 overflow-y-auto">
            {options.map((option) => {
              const isSelected = option.value === value;
              return (
                <button
                  key={option.value}
                  type="button"
                  role="option"
                  aria-selected={isSelected}
                  disabled={option.disabled}
                  className={`
                    flex w-full flex-col p-3 text-left
                    transition-colors
                    ${isSelected ? 'bg-accent text-accent-foreground' : 'text-foreground hover:bg-muted'}
                    ${option.disabled ? 'pointer-events-none opacity-40' : ''}
                  `}
                  onClick={() => handleSelect(option.value)}
                >
                  {renderOption ? (
                    renderOption(option, isSelected)
                  ) : (
                    <>
                      <span className="font-medium">{option.label}</span>
                      {option.description && (
                        <span className="mt-0.5 text-xs text-muted-foreground">
                          {option.description}
                        </span>
                      )}
                    </>
                  )}
                </button>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}
