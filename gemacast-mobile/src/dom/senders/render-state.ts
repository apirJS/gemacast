/**
 * Shared render state for the sender list.
 * Extracted to its own module to avoid circular imports
 * between index.ts and process-select.ts.
 */

let prevRenderHash = '';

export function getRenderHash(): string {
  return prevRenderHash;
}

export function setRenderHash(hash: string): void {
  prevRenderHash = hash;
}

export function invalidateRenderHash(): void {
  prevRenderHash = '';
}
