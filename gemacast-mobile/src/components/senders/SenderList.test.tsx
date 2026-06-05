import { describe, it, expect, beforeEach } from 'bun:test';
import { render, screen, cleanup } from '@testing-library/react';
import { useAppStore } from '../../stores/app-store';
import { Status } from '../../core/types';
import { SenderList } from './SenderList';

beforeEach(() => {
  cleanup();
  useAppStore.getState().init({
    deviceId: 'test',
    deviceName: 'Test',
    ip: '127.0.0.1',
  });
});

describe('SenderList', () => {
  it('shows empty state when scanning with no senders', () => {
    useAppStore.getState().setStatus(Status.Listening);
    render(<SenderList />);
    expect(screen.getByText(/Scanning for PCs/i)).toBeTruthy();
  });

  it('renders sender cards', () => {
    useAppStore.getState().setStatus(Status.Listening);
    useAppStore.getState().setDiscoveredSenders([
      { deviceId: 'pc-1', deviceName: 'Desktop PC', addr: '192.168.1.10:9000', isOffline: false },
      { deviceId: 'pc-2', deviceName: 'Laptop', addr: '192.168.1.11:9000', isOffline: false },
    ]);
    render(<SenderList />);
    expect(screen.getByText('Desktop PC')).toBeTruthy();
    expect(screen.getByText('Laptop')).toBeTruthy();
  });

  it('shows connect button for each sender', () => {
    useAppStore.getState().setStatus(Status.Listening);
    useAppStore
      .getState()
      .setDiscoveredSenders([
        { deviceId: 'pc-1', deviceName: 'My PC', addr: '192.168.1.10:9000', isOffline: false },
      ]);
    render(<SenderList />);
    expect(screen.getByText('Connect')).toBeTruthy();
  });

  it('shows disconnect for connected sender', () => {
    const sender = {
      deviceId: 'pc-1',
      deviceName: 'My PC',
      addr: '192.168.1.10:9000',
      isOffline: false,
    };
    useAppStore.getState().patch({
      status: Status.Connected,
      connectedSender: sender,
      discoveredSenders: [sender],
    });
    render(<SenderList />);
    expect(screen.getByText('Disconnect')).toBeTruthy();
  });

  it('shows ADB label for localhost senders', () => {
    useAppStore.getState().setStatus(Status.Listening);
    useAppStore
      .getState()
      .setDiscoveredSenders([
        { deviceId: 'adb-1', deviceName: 'ADB PC', addr: '127.0.0.1:9000', isOffline: false },
      ]);
    render(<SenderList />);
    expect(screen.getByText('ADB (USB Debug)')).toBeTruthy();
  });

  it('shows Pause button when status is Playing', () => {
    const sender = {
      deviceId: 'pc-1',
      deviceName: 'My PC',
      addr: '192.168.1.10:9000',
      isOffline: false,
    };
    useAppStore.getState().patch({
      status: Status.Playing,
      connectedSender: sender,
      discoveredSenders: [sender],
    });
    render(<SenderList />);
    expect(screen.getByRole('button', { name: /Pause/i })).toBeTruthy();
  });

  it('shows Resume button when status is Paused', () => {
    const sender = {
      deviceId: 'pc-1',
      deviceName: 'My PC',
      addr: '192.168.1.10:9000',
      isOffline: false,
    };
    useAppStore.getState().patch({
      status: Status.Paused,
      connectedSender: sender,
      discoveredSenders: [sender],
    });
    render(<SenderList />);
    expect(screen.getByRole('button', { name: /Resume/i })).toBeTruthy();
  });
});
