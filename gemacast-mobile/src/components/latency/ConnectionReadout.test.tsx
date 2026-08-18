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

  it.each([
    [
      'a hotspot pair',
      {
        pair: { phone: 'wifiUnknown', pc: 'wifi5Ghz', effective: 'wifi5Ghz' },
        shown: '5 GHz',
        absent: 'WiFi',
        title: 'Phone WiFi, PC 5 GHz — buffer tuned for 5 GHz',
      },
    ],
    [
      'a real band split',
      {
        pair: { phone: 'wifi2_4Ghz', pc: 'wifi5Ghz', effective: 'wifi2_4Ghz' },
        shown: '2.4 GHz',
        absent: '5 GHz',
        title: 'Phone 2.4 GHz, PC 5 GHz — buffer tuned for 2.4 GHz',
      },
    ],
    [
      // `effective_link()` rule 4 answers `WifiUnknown` for this pair, so
      // `effective` is neither side and "the other one" is undefinable.
      'a pair whose effective side is neither',
      {
        pair: { phone: 'unknown', pc: 'ethernet', effective: 'wifiUnknown' },
        shown: 'WiFi',
        absent: 'Ethernet',
        title: 'Phone Unknown, PC Ethernet — buffer tuned for WiFi',
      },
    ],
  ] as const)('shows only the effective link for %s', (_name, { pair, shown, absent, title }) => {
    useAppStore.getState().setStatus(Status.Connected);
    useAppStore.getState().setNetworkLinkPair({ ...pair, effectiveLabel: shown });
    const { container } = render(<ConnectionReadout />);

    expect(screen.getByText(shown)).toBeTruthy();
    expect(container.textContent).not.toContain('via');
    expect(container.textContent).not.toContain(absent);

    // The dropped side stays reachable by assistive tech, and naming both sides
    // explicitly is what keeps the rule-4 pair correct.
    expect(container.querySelector('#network-link-badge')?.getAttribute('title')).toBe(title);
  });
});
