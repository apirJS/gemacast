import { describe, it, expect, beforeEach } from 'bun:test';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { setupInvokeMock, makeDeviceInfo } from '../../__tests__/setup';
import { MatchPcVolumeToggle } from './MatchPcVolumeToggle';
import { useAppStore } from '../../stores/app-store';

beforeEach(() => {
  cleanup();
  localStorage.clear();
  setupInvokeMock({ set_match_pc_volume: undefined });
  useAppStore.getState().init(makeDeviceInfo());
});

describe('MatchPcVolumeToggle', () => {
  it('is disabled while the PC has not reported volume sync support', () => {
    useAppStore.getState().updateSettings({ matchPcVolume: true });
    render(<MatchPcVolumeToggle />);
    const toggle = screen.getByRole('checkbox');

    expect(toggle.hasAttribute('disabled')).toBe(true);
    expect(toggle.hasAttribute('checked')).toBe(false);
  });

  it('renders checked once a supporting PC is connected', () => {
    useAppStore.getState().setStreamerCapabilities({
      supportsProcessCapture: false,
      supportsVolumeSync: true,
    });
    useAppStore.getState().updateSettings({ matchPcVolume: true });
    render(<MatchPcVolumeToggle />);
    const toggle = screen.getByRole('checkbox');

    expect(toggle.hasAttribute('disabled')).toBe(false);
    expect(toggle.hasAttribute('checked')).toBe(true);
  });

  it('persists the setting when toggled', () => {
    useAppStore.getState().setStreamerCapabilities({
      supportsProcessCapture: false,
      supportsVolumeSync: true,
    });
    render(<MatchPcVolumeToggle />);
    fireEvent.click(screen.getByRole('checkbox'));

    expect(useAppStore.getState().settings.matchPcVolume).toBe(true);
  });
});
