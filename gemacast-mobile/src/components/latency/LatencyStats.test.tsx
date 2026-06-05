import { describe, it, expect, beforeEach } from 'bun:test';
import { render, screen, cleanup } from '@testing-library/react';
import { useAppStore } from '../../stores/app-store';
import { Status } from '../../core/types';
import { LatencyStats } from './LatencyStats';

beforeEach(() => {
  cleanup();
  useAppStore.getState().init({
    deviceId: 'test',
    deviceName: 'Test',
    ip: '127.0.0.1',
  });
});

describe('LatencyStats', () => {
  it('renders nothing when Idle', () => {
    useAppStore.getState().setStatus(Status.Idle);
    const { container } = render(<LatencyStats />);
    expect(container.innerHTML).toBe('');
  });

  it('renders nothing when Listening', () => {
    useAppStore.getState().setStatus(Status.Listening);
    const { container } = render(<LatencyStats />);
    expect(container.innerHTML).toBe('');
  });

  it('renders stats when Connected', () => {
    useAppStore.getState().setStatus(Status.Connected);
    useAppStore.getState().updateLatency({ current: 42, avg: 38, max: 90, min: 12 });
    render(<LatencyStats />);
    expect(screen.getByText('42 ms')).toBeTruthy();
    expect(screen.getByText('38 ms')).toBeTruthy();
    expect(screen.getByText('90 ms')).toBeTruthy();
    expect(screen.getByText('12 ms')).toBeTruthy();
  });

  it('renders stats when Playing', () => {
    useAppStore.getState().setStatus(Status.Playing);
    useAppStore.getState().updateLatency({ current: 55, avg: 50, max: 100, min: 20 });
    render(<LatencyStats />);
    expect(screen.getByText('55 ms')).toBeTruthy();
  });

  it('renders placeholder when latency is null', () => {
    useAppStore.getState().setStatus(Status.Connected);
    render(<LatencyStats />);
    expect(screen.getAllByText('-- ms')).toHaveLength(4);
  });
});
