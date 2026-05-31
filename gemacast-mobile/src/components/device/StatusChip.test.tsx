import { describe, it, expect, beforeEach } from 'bun:test';
import { render, screen, cleanup } from '@testing-library/react';
import { useAppStore } from '../../stores/app-store';
import { Status } from '../../core/types';
import { StatusChip } from './StatusChip';

beforeEach(() => {
  cleanup();
  useAppStore.getState().init({
    deviceId: 'test',
    deviceName: 'Test Phone',
    ip: '192.168.1.1',
  });
});

describe('StatusChip', () => {
  it('renders Idle label', () => {
    useAppStore.getState().setStatus(Status.Idle);
    render(<StatusChip />);
    expect(screen.getByText('Idle')).toBeTruthy();
  });

  it('renders Scanning label when Listening', () => {
    useAppStore.getState().setStatus(Status.Listening);
    render(<StatusChip />);
    expect(screen.getByText('Scanning…')).toBeTruthy();
  });

  it('renders Connected label', () => {
    useAppStore.getState().setStatus(Status.Connected);
    render(<StatusChip />);
    expect(screen.getByText('Connected')).toBeTruthy();
  });

  it('renders Playing label', () => {
    useAppStore.getState().setStatus(Status.Playing);
    render(<StatusChip />);
    expect(screen.getByText('Playing')).toBeTruthy();
  });

  it('renders Reconnecting with attempt count', () => {
    useAppStore.getState().patch({
      status: Status.Reconnecting,
      reconnectAttempts: 3,
    });
    render(<StatusChip />);
    expect(screen.getByText('Reconnecting (3/5)…')).toBeTruthy();
  });

  it('has role="status" for accessibility', () => {
    useAppStore.getState().setStatus(Status.Idle);
    render(<StatusChip />);
    expect(screen.getByRole('status')).toBeTruthy();
  });
});
