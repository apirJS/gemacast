import { describe, it, expect, beforeEach } from 'bun:test';
import { render, screen, cleanup } from '@testing-library/react';
import { useAppStore } from '../../stores/app-store';
import { Status } from '../../core/types';
import { StreamerList } from './StreamerList';

beforeEach(() => {
  cleanup();
  useAppStore.getState().init({
    deviceId: 'test',
    deviceName: 'Test',
    ip: '127.0.0.1',
  });
});

describe('StreamerList', () => {
  it('shows empty state when scanning with no streamers', () => {
    useAppStore.getState().setStatus(Status.Listening);
    render(<StreamerList />);
    expect(screen.getByText(/Scanning for PCs/i)).toBeTruthy();
  });

  it('renders streamer cards', () => {
    useAppStore.getState().setStatus(Status.Listening);
    useAppStore.getState().setDiscoveredStreamers([
      { deviceId: 'pc-1', deviceName: 'Desktop PC', addr: '192.168.1.10:9000', isOffline: false },
      { deviceId: 'pc-2', deviceName: 'Laptop', addr: '192.168.1.11:9000', isOffline: false },
    ]);
    render(<StreamerList />);
    expect(screen.getByText('Desktop PC')).toBeTruthy();
    expect(screen.getByText('Laptop')).toBeTruthy();
  });

  it('shows connect button for each streamer', () => {
    useAppStore.getState().setStatus(Status.Listening);
    useAppStore
      .getState()
      .setDiscoveredStreamers([
        { deviceId: 'pc-1', deviceName: 'My PC', addr: '192.168.1.10:9000', isOffline: false },
      ]);
    render(<StreamerList />);
    expect(screen.getByText('Connect')).toBeTruthy();
  });

  it('shows disconnect for connected streamer', () => {
    const streamer = {
      deviceId: 'pc-1',
      deviceName: 'My PC',
      addr: '192.168.1.10:9000',
      isOffline: false,
    };
    useAppStore.getState().patch({
      status: Status.Connected,
      connectedStreamer: streamer,
      discoveredStreamers: [streamer],
    });
    render(<StreamerList />);
    expect(screen.getByText('Disconnect')).toBeTruthy();
  });

  it('shows ADB label for localhost streamers', () => {
    useAppStore.getState().setStatus(Status.Listening);
    useAppStore
      .getState()
      .setDiscoveredStreamers([
        { deviceId: 'adb-1', deviceName: 'ADB PC', addr: '127.0.0.1:9000', isOffline: false },
      ]);
    render(<StreamerList />);
    expect(screen.getByText('ADB (USB Debug)')).toBeTruthy();
  });

  it('shows Pause button when status is Playing', () => {
    const streamer = {
      deviceId: 'pc-1',
      deviceName: 'My PC',
      addr: '192.168.1.10:9000',
      isOffline: false,
    };
    useAppStore.getState().patch({
      status: Status.Playing,
      connectedStreamer: streamer,
      discoveredStreamers: [streamer],
    });
    render(<StreamerList />);
    expect(screen.getByRole('button', { name: /Pause/i })).toBeTruthy();
  });

  it('shows Resume button when status is Paused', () => {
    const streamer = {
      deviceId: 'pc-1',
      deviceName: 'My PC',
      addr: '192.168.1.10:9000',
      isOffline: false,
    };
    useAppStore.getState().patch({
      status: Status.Paused,
      connectedStreamer: streamer,
      discoveredStreamers: [streamer],
    });
    render(<StreamerList />);
    expect(screen.getByRole('button', { name: /Resume/i })).toBeTruthy();
  });

  it('scrolls without a visible scrollbar', () => {
    useAppStore.getState().setStatus(Status.Listening);
    render(<StreamerList />);
    const scroller = screen.getByLabelText('Discovered streamers').closest('.overflow-y-auto');
    // Scrolling must survive hiding the bar — `hide-scrollbar` only suppresses
    // the chrome, so the container has to keep `overflow-y-auto`.
    expect(scroller).toBeTruthy();
    expect(scroller?.className).toContain('hide-scrollbar');
    expect(scroller?.className).not.toContain('custom-scrollbar');
  });
});
