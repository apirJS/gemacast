export function fmt(ms: number | null): string {
  return ms !== null ? `${ms.toFixed(1)} ms` : '-- ms';
}
