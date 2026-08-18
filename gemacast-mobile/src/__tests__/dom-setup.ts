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

// Always take these from happy-dom, even where Bun ships a native global (it
// does for `Event` and `CustomEvent`). The DOM realm must be internally
// consistent: happy-dom's `dispatchEvent` does `event instanceof <happy-dom
// Event>`, so a Bun-native `Event` dispatched onto a happy-dom node throws. A
// `!(key in globalThis)` guard here silently leaves those two shadowed by Bun's
// and breaks any test that constructs `new Event(...)` and dispatches it.
for (const key of domGlobals) {
  (globalThis as Record<string, unknown>)[key] = (win as unknown as Record<string, unknown>)[key];
}

if (typeof globalThis.window === 'undefined') {
  (globalThis as Record<string, unknown>).window = win;
}
if (typeof globalThis.navigator === 'undefined') {
  (globalThis as Record<string, unknown>).navigator = win.navigator;
}
