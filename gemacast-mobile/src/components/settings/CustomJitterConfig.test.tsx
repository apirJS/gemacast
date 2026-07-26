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
    useAppStore.getState().updateSettings({
      bufferPreset: 'custom',
      customJitterConfig: {
        minDepthMs: 25,
        comfortCapMs: 1000,
        peakDecayHalflifeMs: 0,
        resumeThresholdPct: 0.25,
        staticTargetMs: 60,
      },
    });
    render(<CustomJitterConfig renderHelpButton={mockRenderHelpButton} />);
    expect(screen.getByText('Buffer Depth')).toBeTruthy();
    expect(screen.getByText('Preset Name')).toBeTruthy();
  });

  it('does not show adaptive fields (Buffer Mode, Min Depth, Comfort Cap)', () => {
    useAppStore.getState().updateSettings({
      bufferPreset: 'custom',
      customJitterConfig: {
        minDepthMs: 25,
        comfortCapMs: 1000,
        peakDecayHalflifeMs: 0,
        resumeThresholdPct: 0.25,
        staticTargetMs: 60,
      },
    });
    render(<CustomJitterConfig renderHelpButton={mockRenderHelpButton} />);
    expect(screen.queryByText('Buffer Mode')).toBeNull();
    expect(screen.queryByText('Min Depth')).toBeNull();
    expect(screen.queryByText('Comfort Cap')).toBeNull();
  });

  it('Save Preset button disabled when name is empty', () => {
    useAppStore.getState().updateSettings({
      bufferPreset: 'custom',
      customJitterConfig: {
        minDepthMs: 25,
        comfortCapMs: 1000,
        peakDecayHalflifeMs: 0,
        resumeThresholdPct: 0.25,
        staticTargetMs: 60,
      },
    });
    render(<CustomJitterConfig renderHelpButton={mockRenderHelpButton} />);
    const saveBtn = screen.getByText('Save Preset');
    expect(saveBtn.hasAttribute('disabled')).toBe(true);
  });

  it('does not show Delete Preset when not editing a saved preset', () => {
    useAppStore.getState().updateSettings({
      bufferPreset: 'custom',
      customJitterConfig: {
        minDepthMs: 25,
        comfortCapMs: 1000,
        peakDecayHalflifeMs: 0,
        resumeThresholdPct: 0.25,
        staticTargetMs: 60,
      },
    });
    render(<CustomJitterConfig renderHelpButton={mockRenderHelpButton} />);
    expect(screen.queryByText('Delete Preset')).toBeNull();
  });

  it('shows Delete Preset when editing a saved preset', () => {
    const config = {
      minDepthMs: 42,
      comfortCapMs: 200,
      peakDecayHalflifeMs: 1000,
      resumeThresholdPct: 0.5,
      staticTargetMs: 60,
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
