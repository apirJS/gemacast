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
    expect(screen.getByText('42 ms')).toBeTruthy();
    expect(screen.getByText('18 ms')).toBeTruthy();
    expect(screen.getByText('3 ms')).toBeTruthy();
  });

  it('renders buffer metric when Playing', () => {
    useAppStore.getState().setStatus(Status.Playing);
    useAppStore.getState().updateMetrics({ bufferMs: 55 });
    render(<ConnectionMetrics />);
    expect(screen.getByText('55 ms')).toBeTruthy();
  });

  it('renders placeholders when metrics are null', () => {
    useAppStore.getState().setStatus(Status.Connected);
    render(<ConnectionMetrics />);
    expect(screen.getAllByText('-- ms')).toHaveLength(3);
  });
});
