import { describe, it, expect, beforeEach } from 'bun:test';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
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
  isPlaying: false,
  isLoading: false,
  isDisabled: false,
  audioSources: [] as any[],
  processList: [] as any[],
  currentSource: { type: 'desktop' } as any,
  senderCapabilities: null,
  onToggle: noop,
  onPlayPause: noop,
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

  it('does not show play/pause button when not connected', () => {
    render(<SenderCard {...defaultProps()} />);
    expect(screen.queryByRole('button', { name: /Pause/i })).toBeNull();
    expect(screen.queryByRole('button', { name: /Resume/i })).toBeNull();
  });

  it('shows Pause button when connected and playing', () => {
    render(<SenderCard {...defaultProps()} isConnected isPlaying />);
    const btn = screen.getByRole('button', { name: /Pause/i });
    expect(btn).toBeTruthy();
  });

  it('shows Play button when connected and paused', () => {
    render(<SenderCard {...defaultProps()} isConnected isPlaying={false} />);
    const btn = screen.getByRole('button', { name: /Resume/i });
    expect(btn).toBeTruthy();
  });

  it('calls onPlayPause when play/pause button is clicked', () => {
    let called = false;
    const onPlayPause = () => { called = true; };
    render(<SenderCard {...defaultProps()} isConnected isPlaying onPlayPause={onPlayPause} />);
    const btn = screen.getByRole('button', { name: /Pause/i });
    fireEvent.click(btn);
    expect(called).toBe(true);
  });
});
