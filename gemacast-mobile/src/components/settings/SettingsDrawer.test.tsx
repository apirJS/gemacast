import { describe, it, expect, beforeEach } from 'bun:test';
import { render, screen, cleanup } from '@testing-library/react';
import { useAppStore } from '../../stores/app-store';
import { SettingsDrawer } from './SettingsDrawer';

beforeEach(() => {
  cleanup();
  useAppStore.getState().init({
    deviceId: 'test',
    deviceName: 'Test',
    ip: '127.0.0.1',
  });
});

describe('SettingsDrawer', () => {
  it('renders settings button', () => {
    render(<SettingsDrawer />);
    expect(screen.getByLabelText('Open settings')).toBeTruthy();
  });

  it('renders Buffer Preset label', () => {
    render(<SettingsDrawer />);
    expect(screen.getAllByText('Buffer Preset').length).toBeGreaterThanOrEqual(1);
  });

  it('renders Audio Bitrate label', () => {
    render(<SettingsDrawer />);
    expect(screen.getAllByText('Audio Bitrate Quality').length).toBeGreaterThanOrEqual(1);
  });

  it('renders Mode label', () => {
    render(<SettingsDrawer />);
    expect(screen.getAllByText('Mode').length).toBeGreaterThanOrEqual(1);
  });
});
