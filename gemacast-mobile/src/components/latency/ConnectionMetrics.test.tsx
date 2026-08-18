import { describe, it, expect, beforeEach } from 'bun:test';
import { render, screen, cleanup } from '@testing-library/react';
import { useAppStore } from '../../stores/app-store';
import { Status } from '../../core/types';
import { ConnectionMetrics } from './ConnectionMetrics';

beforeEach(() => {
  cleanup();
  useAppStore.getState().init({
    deviceId: 'test',
    deviceName: 'Test',
    ip: '127.0.0.1',
  });
});

describe('ConnectionMetrics', () => {
  it('renders nothing when Idle', () => {
    useAppStore.getState().setStatus(Status.Idle);
    const { container } = render(<ConnectionMetrics />);
    expect(container.innerHTML).toBe('');
  });

  it('renders nothing when Listening', () => {
    useAppStore.getState().setStatus(Status.Listening);
    const { container } = render(<ConnectionMetrics />);
    expect(container.innerHTML).toBe('');
  });

  it('renders the three metrics when Connected', () => {
    useAppStore.getState().setStatus(Status.Connected);
    useAppStore.getState().updateMetrics({ bufferMs: 42, networkRttMs: 18, jitterMs: 3 });
    render(<ConnectionMetrics />);
    // The value and its unit are separate spans (18px digits / 10px "ms"), so
    // each number is asserted on its own node rather than as "42 ms".
    expect(screen.getByText('42')).toBeTruthy();
    expect(screen.getByText('18')).toBeTruthy();
    expect(screen.getByText('3')).toBeTruthy();
    expect(screen.getAllByText('ms')).toHaveLength(3);
  });

  it('renders buffer metric when Playing', () => {
    useAppStore.getState().setStatus(Status.Playing);
    useAppStore.getState().updateMetrics({ bufferMs: 55 });
    render(<ConnectionMetrics />);
    expect(screen.getByText('55')).toBeTruthy();
  });

  it('renders placeholders when metrics are null', () => {
    useAppStore.getState().setStatus(Status.Connected);
    render(<ConnectionMetrics />);
    expect(screen.getAllByText('--')).toHaveLength(3);
  });

  it('tints each metric by its own health band', () => {
    useAppStore.getState().setStatus(Status.Connected);
    // Buffer ok (<=30), RTT degraded (>30, <=80), jitter lost (>25).
    useAppStore.getState().updateMetrics({ bufferMs: 12, networkRttMs: 45, jitterMs: 40 });
    render(<ConnectionMetrics />);
    expect(screen.getByText('12').className).toContain('text-status-ok');
    expect(screen.getByText('45').className).toContain('text-status-warn');
    expect(screen.getByText('40').className).toContain('text-status-lost');
  });

  it('marks RTT as not applicable on an ADB link instead of showing a pending value', () => {
    useAppStore.getState().setStatus(Status.Connected);
    useAppStore.getState().setNetworkLinkPair({
      phone: 'adb',
      pc: 'adb',
      effective: 'adb',
      effectiveLabel: 'ADB',
    });
    // The control-channel probe loop does not run over loopback, so RTT is
    // permanently null there — "n/a" rather than a "--" that implies pending.
    useAppStore.getState().updateMetrics({ bufferMs: 8, jitterMs: 1 });
    render(<ConnectionMetrics />);
    expect(screen.getByText('n/a')).toBeTruthy();
    expect(screen.queryByText('--')).toBeNull();
  });
});
