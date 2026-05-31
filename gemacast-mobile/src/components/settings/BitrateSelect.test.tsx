import { describe, it, expect, beforeEach } from 'bun:test';
import { render, screen, cleanup } from '@testing-library/react';
import { useAppStore } from '../../stores/app-store';
import { BitrateSelect } from './BitrateSelect';

beforeEach(() => {
  cleanup();
  useAppStore.getState().init({
    deviceId: 'test',
    deviceName: 'Test',
    ip: '127.0.0.1',
  });
});

describe('BitrateSelect', () => {
  // Label was moved to SettingsDrawer

  it('does not show custom input by default', () => {
    useAppStore.getState().updateSettings({ bitratePreset: '128' });
    render(<BitrateSelect />);
    expect(screen.queryByText('Apply')).toBeNull();
  });
});
