import { describe, it, expect, beforeEach } from 'bun:test';
import { render, screen, cleanup } from '@testing-library/react';
import { useAppStore } from '../../stores/app-store';
import { CustomJitterConfig } from './CustomJitterConfig';

beforeEach(() => {
  cleanup();
  useAppStore.getState().init({
    deviceId: 'test',
    deviceName: 'Test',
    ip: '127.0.0.1',
  });
});

const mockRenderHelpButton = () => null;

describe('CustomJitterConfig', () => {
  it('renders nothing when bufferPreset != custom', () => {
    useAppStore.getState().updateSettings({ bufferPreset: 'balanced' });
    const { container } = render(<CustomJitterConfig renderHelpButton={mockRenderHelpButton} />);
    expect(container.innerHTML).toBe('');
  });

  it('renders config fields when bufferPreset == custom', () => {
    useAppStore.getState().updateSettings({ bufferPreset: 'custom' });
    render(<CustomJitterConfig renderHelpButton={mockRenderHelpButton} />);
    expect(screen.getByText('Buffer Mode')).toBeTruthy();
    expect(screen.getByText('Preset Name')).toBeTruthy();
  });

  it('shows adaptive fields by default', () => {
    useAppStore.getState().updateSettings({ bufferPreset: 'custom' });
    render(<CustomJitterConfig renderHelpButton={mockRenderHelpButton} />);
    expect(screen.getByText('Min Depth')).toBeTruthy();
    expect(screen.getByText('Comfort Cap')).toBeTruthy();
  });

  it('Save Preset button disabled when name is empty', () => {
    useAppStore.getState().updateSettings({ bufferPreset: 'custom' });
    render(<CustomJitterConfig renderHelpButton={mockRenderHelpButton} />);
    const saveBtn = screen.getByText('Save Preset');
    expect(saveBtn.hasAttribute('disabled')).toBe(true);
  });

  it('does not show Delete Preset when not editing a saved preset', () => {
    useAppStore.getState().updateSettings({ bufferPreset: 'custom' });
    render(<CustomJitterConfig renderHelpButton={mockRenderHelpButton} />);
    expect(screen.queryByText('Delete Preset')).toBeNull();
  });

  it('shows Delete Preset when editing a saved preset', () => {
    const config = {
      minDepthMs: 42,
      comfortCapMs: 200,
      peakDecayHalflifeMs: 1000,
      resumeThresholdPct: 0.5,
    };
    useAppStore.getState().updateSettings({
      bufferPreset: 'saved-0',
      customJitterConfig: config,
      savedPresets: [{ name: 'My Preset', config }],
    });
    render(<CustomJitterConfig renderHelpButton={mockRenderHelpButton} />);
    expect(screen.getByText('Delete Preset')).toBeTruthy();
  });
});
