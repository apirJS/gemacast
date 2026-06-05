import { Window } from 'happy-dom';

const win = new Window({ url: 'http://localhost' });

const domGlobals = [
  'document',
  'HTMLElement',
  'HTMLDivElement',
  'HTMLInputElement',
  'HTMLButtonElement',
  'HTMLDialogElement',
  'HTMLLabelElement',
  'HTMLSpanElement',
  'Element',
  'Node',
  'Text',
  'DocumentFragment',
  'MutationObserver',
  'getComputedStyle',
  'requestAnimationFrame',
  'cancelAnimationFrame',
  'CustomEvent',
  'Event',
] as const;

for (const key of domGlobals) {
  if (!(key in globalThis)) {
    (globalThis as Record<string, unknown>)[key] = (win as unknown as Record<string, unknown>)[key];
  }
}

if (typeof globalThis.window === 'undefined') {
  (globalThis as Record<string, unknown>).window = win;
}
if (typeof globalThis.navigator === 'undefined') {
  (globalThis as Record<string, unknown>).navigator = win.navigator;
}
