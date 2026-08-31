import { describe, it, expect, beforeEach } from 'bun:test';
import { render, screen, cleanup, act, fireEvent } from '@testing-library/react';
import { useAppStore } from '../../stores/app-store';
import { SettingsDrawer } from './SettingsDrawer';

/** The sliding panel. Queried directly so no library visibility rule can hide it. */
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

  describe('sliding', () => {
    // The bug these cover: the drawer used to stay in the DOM as an open modal
    // <dialog> after sliding out, so its backdrop ate every tap in the app while
    // showing nothing. Both properties asserted here are derived from `open`
    // alone, so there is no state left to drift.
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
        // A plain Event: the hook reads the hash, not the event.
        window.dispatchEvent(new Event('popstate'));
      });

      expect(panel().className).toContain('-translate-x-full');
      expect(panel().hasAttribute('inert')).toBe(true);
    });
  });
});
