type StatusChipViewProps = {
  label: string;
  tone: string;
};

export function StatusChipView({ label, tone }: StatusChipViewProps) {
  return (
    <span
      role="status"
      aria-live="polite"
      className={`text-xs font-medium tracking-wide whitespace-nowrap transition-colors duration-300 ${tone}`}
    >
      {label}
    </span>
  );
}
