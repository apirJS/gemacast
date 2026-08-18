import { afterEach, beforeEach, describe, expect, it } from 'bun:test';
import { act, cleanup, render, screen } from '@testing-library/react';
import { usePullToRefresh } from './use-pull-to-refresh';

// happy-dom has no `TouchEvent`, so we hand-build a plain `Event` and hang a
// `touches` list off it — the same shape the hook reads (`touches?.[0]?.clientY`).
function dispatchTouch(el: Element, type: string, clientY: number | null) {
  const event = new Event(type, { bubbles: true, cancelable: true });
  if (clientY !== null) {
    (event as unknown as { touches: Array<{ clientY: number }> }).touches = [{ clientY }];
  }
  el.dispatchEvent(event);
}

type HarnessProps = {
  onRefresh: () => void | Promise<void>;
};

function Harness({ onRefresh }: HarnessProps) {
  // minSpinMs 0 so the post-refresh hold resolves promptly under the test clock.
  const { ref, pull, refreshing } = usePullToRefresh<HTMLDivElement>({
    onRefresh,
    threshold: 64,
    minSpinMs: 0,
  });
  return (
    <div ref={ref} data-testid="scroller">
      <span data-testid="pull">{pull}</span>
      <span data-testid="refreshing">{String(refreshing)}</span>
    </div>
  );
}

let refreshCount = 0;
const onRefresh = () => {
  refreshCount += 1;
};

function scroller() {
  return screen.getByTestId('scroller');
}

function pullValue() {
  return Number(screen.getByTestId('pull').textContent);
}

beforeEach(() => {
  cleanup();
  refreshCount = 0;
});

afterEach(() => {
  cleanup();
});

/**
 * Mirrors the real tree: an overlay that renders *inside* the scroll container
 * but scrolls independently, marked so the pull gesture ignores it.
 */
function NestedHarness({ onRefresh }: HarnessProps) {
  const { ref, pull, refreshing } = usePullToRefresh<HTMLDivElement>({
    onRefresh,
    threshold: 64,
    minSpinMs: 0,
  });
  return (
    <div ref={ref} data-testid="scroller">
      <div data-pull-refresh-ignore="" data-testid="overlay">
        <span data-testid="overlay-child">a process row</span>
      </div>
      <span data-testid="pull">{pull}</span>
      <span data-testid="refreshing">{String(refreshing)}</span>
    </div>
  );
}

describe('usePullToRefresh', () => {
  it('triggers onRefresh when released past the threshold', async () => {
    render(<Harness onRefresh={onRefresh} />);
    const el = scroller();

    await act(async () => dispatchTouch(el, 'touchstart', 100));
    // delta 200 / resistance 2 = 100, capped to maxPull 96 (>= threshold 64).
    await act(async () => dispatchTouch(el, 'touchmove', 300));
    expect(pullValue()).toBe(96);

    await act(async () => dispatchTouch(el, 'touchend', null));
    expect(refreshCount).toBe(1);
  });

  it('does not trigger below the threshold and springs back', async () => {
    render(<Harness onRefresh={onRefresh} />);
    const el = scroller();

    await act(async () => dispatchTouch(el, 'touchstart', 100));
    // delta 60 / resistance 2 = 30, short of the 64 threshold.
    await act(async () => dispatchTouch(el, 'touchmove', 160));
    expect(pullValue()).toBe(30);

    await act(async () => dispatchTouch(el, 'touchend', null));
    expect(refreshCount).toBe(0);
    expect(pullValue()).toBe(0);
  });

  it('ignores the gesture when the scroller is not at the top', async () => {
    render(<Harness onRefresh={onRefresh} />);
    const el = scroller();
    Object.defineProperty(el, 'scrollTop', { value: 40, configurable: true });

    await act(async () => dispatchTouch(el, 'touchstart', 100));
    await act(async () => dispatchTouch(el, 'touchmove', 300));
    expect(pullValue()).toBe(0);

    await act(async () => dispatchTouch(el, 'touchend', null));
    expect(refreshCount).toBe(0);
  });

  it('hands back to native scroll when the finger moves up', async () => {
    render(<Harness onRefresh={onRefresh} />);
    const el = scroller();

    await act(async () => dispatchTouch(el, 'touchstart', 100));
    // Upward drag: delta is negative, so the hook never owns the gesture.
    await act(async () => dispatchTouch(el, 'touchmove', 60));
    expect(pullValue()).toBe(0);

    await act(async () => dispatchTouch(el, 'touchend', null));
    expect(refreshCount).toBe(0);
  });

  describe('nested scrollable regions', () => {
    it('ignores a downward drag that starts inside a marked overlay', async () => {
      render(<NestedHarness onRefresh={onRefresh} />);
      const child = screen.getByTestId('overlay-child');

      // Same gesture that pulls 96px on the bare container. Events bubble to the
      // scroller's listeners either way — the marker is what disarms it.
      await act(async () => dispatchTouch(child, 'touchstart', 100));
      await act(async () => dispatchTouch(child, 'touchmove', 300));
      expect(pullValue()).toBe(0);

      await act(async () => dispatchTouch(child, 'touchend', null));
      expect(refreshCount).toBe(0);
    });

    it('still pulls when the drag starts outside the overlay', async () => {
      render(<NestedHarness onRefresh={onRefresh} />);
      const el = scroller();

      // Proves the guard is scoped to the marked subtree and did not disable the
      // whole gesture — the failure mode a `closest` typo would produce.
      await act(async () => dispatchTouch(el, 'touchstart', 100));
      await act(async () => dispatchTouch(el, 'touchmove', 300));
      expect(pullValue()).toBe(96);

      await act(async () => dispatchTouch(el, 'touchend', null));
      expect(refreshCount).toBe(1);
    });
  });
});
