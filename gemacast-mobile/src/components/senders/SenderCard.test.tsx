import { describe, it, expect, beforeEach } from 'bun:test';
import { render, screen, cleanup } from '@testing-library/react';
import { useAppStore } from '../../stores/app-store';
import { SenderCard } from './SenderCard';

const makeSender = (overrides: Partial<{ deviceId: string; deviceName: string; addr: string }> = {}) => ({
  deviceId: 'pc-1', 
  deviceName: 'Desktop PC',
  addr: '192.168.1.10:9000',
  isOffline: false,
  ...overrides,
});

const noop = () => {};

const defaultProps = () => ({
  sender: makeSender(),
  isConnected: false,
  isConnecting: false,
  isLoading: false,
  isDisabled: false,
  audioSources: [] as any[],
  processList: [] as any[],
  currentSource: { type: 'desktop' } as any,
  senderCapabilities: null,
  onToggle: noop,
  onSourceChange: noop,
});

beforeEach(() => {
  cleanup();
  useAppStore.getState().init({
    deviceId: 'test',
    deviceName: 'Test',
    ip: '127.0.0.1',
  });
});

describe('SenderCard', () => {
  it('renders sender name and IP', () => {
    render(<SenderCard {...defaultProps()} />);
    expect(screen.getByText('Desktop PC')).toBeTruthy();
    expect(screen.getByText('192.168.1.10')).toBeTruthy();
  });

  it('shows Connect when not connected', () => {
    render(<SenderCard {...defaultProps()} />);
    expect(screen.getByText('Connect')).toBeTruthy();
  });

  it('shows Disconnect when connected', () => {
    render(<SenderCard {...defaultProps()} isConnected />);
    expect(screen.getByText('Disconnect')).toBeTruthy();
  });

  it('shows spinner when loading and connected', () => {
    render(<SenderCard {...defaultProps()} isConnected isLoading />);
    expect(screen.queryByText('Disconnect')).toBeNull();
    expect(screen.queryByText('Connect')).toBeNull();
  });

  it('shows ADB icon for localhost senders', () => {
    const sender = makeSender({ addr: '127.0.0.1:9000' });
    render(<SenderCard {...defaultProps()} sender={sender} />);
    expect(screen.getByText('ADB (USB Debug)')).toBeTruthy();
  });

  it('disables button when isDisabled', () => {
    render(<SenderCard {...defaultProps()} isDisabled />);
    const btn = screen.getByRole('button', { name: /Connect to/i });
    expect(btn.hasAttribute('disabled')).toBe(true);
  });

  it('shows ProcessSelect when connected with audio sources', () => {
    const sender = makeSender();
    render(
      <SenderCard
        {...defaultProps()}
        sender={sender}
        isConnected
        audioSources={[{ type: 'desktop' }]}
      />,
    );
    expect(screen.getByText('Desktop Audio')).toBeTruthy();
  });
});
