type SegmentOption<T extends string = string> = {
  value: T;
  label: string;
  disabled?: boolean;
};

type SegmentedControlProps<T extends string = string> = {
  name: string;
  options: SegmentOption<T>[];
  value: T;
  onChange: (value: T) => void;
  size?: 'default' | 'mini';
};

export function SegmentedControl<T extends string = string>({
  name,
  options,
  value,
  onChange,
  size = 'default',
}: SegmentedControlProps<T>) {
  const isMini = size === 'mini';
  return (
    <div
      className={`flex overflow-hidden rounded-lg border border-border bg-muted ${isMini ? 'w-auto' : 'w-full'}`}
    >
      {options.map((option) => (
        <label
          key={option.value}
          className={`
            flex-1 cursor-pointer border-r border-border text-center font-medium transition-all duration-200 select-none last:border-r-0
            ${isMini ? 'px-3 py-1.5 text-[0.78rem]' : 'px-3 py-3 text-[0.9rem]'}
            ${
              value === option.value
                ? 'bg-primary text-primary-foreground'
                : 'text-muted-foreground hover:bg-accent/50'
            }
            ${option.disabled ? 'pointer-events-none !bg-muted !text-muted-foreground opacity-40 grayscale' : ''}
          `}
        >
          <input
            type="radio"
            name={name}
            value={option.value}
            checked={value === option.value}
            disabled={option.disabled}
            className="sr-only"
            onChange={() => onChange(option.value)}
          />
          {option.label}
        </label>
      ))}
    </div>
  );
}
