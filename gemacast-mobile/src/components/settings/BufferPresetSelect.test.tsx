import { describe, it, expect, beforeEach } from 'bun:test';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { BufferPresetSelect } from './BufferPresetSelect';
import { useAppStore } from '../../stores/app-store';

const defaultSettings = {
  bufferPreset: 'auto',
  customJitterConfig: { adaptive: true, targetMs: 150 },
  savedPresets: [
    { name: 'My Preset', config: { adaptive: false, targetMs: 50, staticTargetMs: 50 } },
  ],
};

beforeEach(() => {
  cleanup();
  useAppStore.getState().updateSettings(defaultSettings as any);
});

describe('BufferPresetSelect', () => {
  it('renders correctly with current preset', () => {
    render(<BufferPresetSelect />);
    expect(screen.getByText('Auto')).toBeTruthy();
  });

  it('renders all presets including saved ones when opened', () => {
    render(<BufferPresetSelect />);
    const trigger = screen.getAllByRole('button')[0];
    fireEvent.click(trigger);

    // Built-in presets
    expect(screen.getByText('Wired')).toBeTruthy();
    expect(screen.getByText('Custom')).toBeTruthy();
    // Saved preset
    expect(screen.getByText('My Preset')).toBeTruthy();
  });

  it('updates bufferPreset for built-in preset', () => {
    render(<BufferPresetSelect />);
    const trigger = screen.getAllByRole('button')[0];
    fireEvent.click(trigger);

    fireEvent.click(screen.getByText('Wired'));

    expect(useAppStore.getState().settings.bufferPreset).toBe('wired');
  });

  it('updates customJitterConfig from auto when custom is selected', () => {
    render(<BufferPresetSelect />);
    const trigger = screen.getAllByRole('button')[0];
    fireEvent.click(trigger);

    fireEvent.click(screen.getByText('Custom'));

    expect(useAppStore.getState().settings.bufferPreset).toBe('custom');
    expect(useAppStore.getState().settings.customJitterConfig.minDepthMs).toBe(8);
    expect(useAppStore.getState().settings.customJitterConfig.comfortCapMs).toBe(1500);
  });

  it('updates from saved preset', () => {
    render(<BufferPresetSelect />);
    const trigger = screen.getAllByRole('button')[0];
    fireEvent.click(trigger);

    fireEvent.click(screen.getByText('My Preset'));

    expect(useAppStore.getState().settings.bufferPreset).toBe('saved-0');
    expect(useAppStore.getState().settings.customJitterConfig).toMatchObject({
      adaptive: false,
      targetMs: 50,
      staticTargetMs: 50,
    });
  });
});
