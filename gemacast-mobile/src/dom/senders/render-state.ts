let prevRenderHash = '';
let forceNextRender = false;

export function getRenderHash(): string {
  return prevRenderHash;
}

export function setRenderHash(hash: string): void {
  prevRenderHash = hash;
}

export function invalidateRenderHash(): void {
  prevRenderHash = '';
}

export function getForceNextRender(): boolean {
  return forceNextRender;
}

export function setForceNextRender(force: boolean): void {
  forceNextRender = force;
}
