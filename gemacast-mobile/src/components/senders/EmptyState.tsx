import { Radio } from 'lucide-react';

export function EmptyState() {
  return (
    <div className="flex flex-col items-center justify-center gap-3 px-4 py-8 text-center animate-[fade-in_300ms_ease-out]">
      <div className="relative flex h-12 w-12 items-center justify-center">
        <div className="absolute inset-0 rounded-full bg-primary/15 animate-[scan-pulse_2s_ease-in-out_infinite]" />
        <Radio className="relative h-6 w-6 text-primary" />
      </div>
      <p className="m-0 text-[0.9rem] font-semibold text-foreground">
        Scanning for PCs on your network…
      </p>
      <p className="m-0 text-[0.8rem] text-muted-foreground">
        Make sure GemaCast is running on your PC
      </p>
    </div>
  );
}
