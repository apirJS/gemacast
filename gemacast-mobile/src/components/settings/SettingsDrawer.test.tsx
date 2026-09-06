import { describe, it, expect, beforeEach } from 'bun:test';
import { render, screen, cleanup, act, fireEvent } from '@testing-library/react';
import { useAppStore } from '../../stores/app-store';
import { SettingsDrawer } from './SettingsDrawer';

const panel = () => document.querySelector('[role="dialog"][aria-label="Settings"]')!;

beforeEach(() => {
  cleanup();
  window.location.hash = '';
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

  it('renders Match Volume With PC label', () => {
    render(<SettingsDrawer />);
    expect(screen.getAllByText('Match Volume With PC').length).toBeGreaterThanOrEqual(1);
  });

  it('explains an unreported volume level while the PC cannot sync it', () => {
    render(<SettingsDrawer />);
    expect(screen.getAllByText('Not reported by this PC').length).toBeGreaterThanOrEqual(1);
  });

  it('drops the hint once the PC reports volume sync support', () => {
    useAppStore.getState().setStreamerCapabilities({
      supportsProcessCapture: false,
      supportsVolumeSync: true,
    });
    render(<SettingsDrawer />);
    expect(screen.queryByText('Not reported by this PC')).toBeNull();
  });

  describe('sliding', () => {
    it('keeps a closed drawer off-screen and inert, so it cannot swallow taps', () => {
      render(<SettingsDrawer />);
      expect(panel().className).toContain('-translate-x-full');
      expect(panel().hasAttribute('inert')).toBe(true);
    });

    it('slides in and becomes interactive when opened', () => {
      render(<SettingsDrawer />);
      fireEvent.click(screen.getByLabelText('Open settings'));
      expect(panel().className).toContain('translate-x-0');
      expect(panel().className).not.toContain('-translate-x-full');
      expect(panel().hasAttribute('inert')).toBe(false);
    });

    it('closes when a back navigation leaves the settings hash', () => {
      render(<SettingsDrawer />);
      fireEvent.click(screen.getByLabelText('Open settings'));
      expect(panel().hasAttribute('inert')).toBe(false);

      act(() => {
        window.location.hash = '';
        window.dispatchEvent(new Event('popstate'));
      });

      expect(panel().className).toContain('-translate-x-full');
      expect(panel().hasAttribute('inert')).toBe(true);
    });
  });
});
