import { describe, it, expect, beforeEach } from 'bun:test';
import { render, screen, cleanup } from '@testing-library/react';
import { useAppStore } from '../../stores/app-store';
import { DeviceInfo } from './DeviceInfo';

beforeEach(() => {
  cleanup();
  useAppStore.getState().init({
    deviceId: 'abc-123',
    deviceName: 'Pixel 9',
    ip: '192.168.1.42',
  });
});

describe('DeviceInfo', () => {
  it('renders device name', () => {
    render(<DeviceInfo />);
    expect(screen.getByText('Pixel 9')).toBeTruthy();
  });

  it('renders IP address', () => {
    render(<DeviceInfo />);
    expect(screen.getByText('IP: 192.168.1.42')).toBeTruthy();
  });
});
