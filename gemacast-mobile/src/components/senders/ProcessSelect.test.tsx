import { describe, it, expect, beforeEach, mock } from 'bun:test';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { ProcessSelect } from './ProcessSelect';

beforeEach(() => {
  cleanup();
});

describe('ProcessSelect', () => {
  const mockFetchProcessList = mock(() => Promise.resolve());

  beforeEach(() => {
    // Mock the useConnection hook
    mock.module('../../hooks/use-connection', () => ({
      useConnection: () => ({ fetchProcessList: mockFetchProcessList }),
    }));
    mockFetchProcessList.mockClear();
  });

  const defaultProps = {
    audioSources: [{ type: 'desktop' as const }],
    processList: [
      { pid: 100, name: 'Spotify.exe', hasAudioSession: true },
      { pid: 200, name: 'Notepad.exe', hasAudioSession: false },
    ],
    currentSource: { type: 'desktop' as const },
    onSourceChange: mock(),
    sender: { deviceId: '123', deviceName: 'PC', addr: '10.0.0.1:9000', isOffline: false },
    supportsProcessCapture: true,
  };

  it('renders current source label', () => {
    render(<ProcessSelect {...defaultProps} />);
    expect(screen.getByText('Desktop Audio')).toBeTruthy();
  });

  it('renders process label when current source is process', () => {
    const props = {
      ...defaultProps,
      currentSource: {
        type: 'process' as const,
        pid: 100,
        name: 'Spotify.exe',
        hasAudioSession: true,
      },
    };
    render(<ProcessSelect {...props} />);
    expect(screen.getByText('Spotify.exe (PID: 100)')).toBeTruthy();
  });

  it('opens dropdown when clicked', () => {
    render(<ProcessSelect {...defaultProps} />);
    const trigger = screen.getByRole('button');
    fireEvent.click(trigger);

    // Dropdown should be open
    expect(screen.getByPlaceholderText('Search process...')).toBeTruthy();
    expect(screen.getByText('Spotify.exe')).toBeTruthy();
  });

  it('filters processes based on search', () => {
    render(<ProcessSelect {...defaultProps} />);
    fireEvent.click(screen.getByRole('button'));

    const input = screen.getByPlaceholderText('Search process...');
    fireEvent.change(input, { target: { value: 'note' } });

    expect(screen.getByText('Notepad.exe')).toBeTruthy();
    expect(screen.queryByText('Spotify.exe')).toBeNull();
  });

  it('calls onSourceChange when a process is selected', () => {
    render(<ProcessSelect {...defaultProps} />);
    fireEvent.click(screen.getByRole('button'));

    const spotifyBtn = screen.getByText('Spotify.exe').closest('button');
    fireEvent.click(spotifyBtn!);

    expect(defaultProps.onSourceChange).toHaveBeenCalledWith({
      type: 'process',
      pid: 100,
      name: 'Spotify.exe',
      hasAudioSession: true,
    });
  });

  it('calls fetchProcessList when refresh is clicked', async () => {
    render(<ProcessSelect {...defaultProps} />);
    fireEvent.click(screen.getByRole('button'));

    const refreshBtn = screen.getByLabelText('Refresh process list');
    fireEvent.click(refreshBtn);

    expect(mockFetchProcessList).toHaveBeenCalledWith(defaultProps.sender);
  });
});
