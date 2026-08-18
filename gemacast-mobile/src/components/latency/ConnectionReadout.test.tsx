import { describe, it, expect, beforeEach } from 'bun:test';
import { render, screen, cleanup } from '@testing-library/react';
import { useAppStore } from '../../stores/app-store';
import { Status } from '../../core/types';
import { ConnectionReadout } from './ConnectionReadout';

beforeEach(() => {
  cleanup();
  useAppStore.getState().init({
    deviceId: 'test',
    deviceName: 'Test',
    ip: '127.0.0.1',
  });
});

describe('ConnectionReadout', () => {
  // The pre-session states are the point of the component: it must occupy no
  // space at all rather than render a card holding placeholder text.
  it.each([
    ['Idle', Status.Idle],
    ['Listening', Status.Listening],
    ['Connecting', Status.Connecting],
  ])('renders nothing when %s', (_name, status) => {
    useAppStore.getState().setStatus(status);
    const { container } = render(<ConnectionReadout />);
    expect(container.innerHTML).toBe('');
  });

  it('renders the card with status and metrics when Connected', () => {
    useAppStore.getState().setStatus(Status.Connected);
    useAppStore.getState().updateMetrics({ bufferMs: 20, networkRttMs: 9, jitterMs: 4 });
    render(<ConnectionReadout />);
    expect(screen.getByText('Connected')).toBeTruthy();
    expect(screen.getByText('20')).toBeTruthy();
  });

  it('keeps the card up while Reconnecting so the stall is explained', () => {
    useAppStore.getState().patch({ status: Status.Reconnecting, reconnectAttempts: 2 });
    render(<ConnectionReadout />);
    expect(screen.getByText('Reconnecting (2/5)…')).toBeTruthy();
    // Metrics are meaningless with the link down, so that row stays hidden.
    expect(screen.queryByLabelText('Connection metrics')).toBeNull();
  });

  it('joins status and link with a separator only when a link is known', () => {
    useAppStore.getState().setStatus(Status.Connected);
    const { container: withoutLink } = render(<ConnectionReadout />);
    expect(withoutLink.textContent).not.toContain('|');

    cleanup();
    useAppStore.getState().setNetworkLinkPair({
      phone: 'wifi5Ghz',
      pc: 'wifi5Ghz',
      effective: 'wifi5Ghz',
      effectiveLabel: '5 GHz',
    });
    const { container: withLink } = render(<ConnectionReadout />);
    expect(withLink.textContent).toContain('|');
    expect(screen.getByText('5 GHz')).toBeTruthy();
  });
});
