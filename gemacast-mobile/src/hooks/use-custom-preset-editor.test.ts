import { describe, it, expect, beforeEach } from 'bun:test';
import { renderHook, act, cleanup } from '@testing-library/react';
import { useAppStore } from '../stores/app-store';
import { useCustomPresetEditor } from './use-custom-preset-editor';
import type { JitterConfig } from '../core/types';

const autoConfig: JitterConfig = {
  minDepthMs: 8,
  comfortCapMs: 1500,
  peakDecayHalflifeMs: 0,
  resumeThresholdPct: 0.25,
};

beforeEach(() => {
  cleanup();
  useAppStore.getState().init({
    deviceId: 'test-device',
    deviceName: 'Test Phone',
    ip: '127.0.0.1',
  });
});

describe('useCustomPresetEditor', () => {
  it('isCustom is false when bufferPreset is not custom', () => {
    useAppStore.getState().updateSettings({ bufferPreset: 'balanced' });
    const { result } = renderHook(() => useCustomPresetEditor());
    expect(result.current.isCustom).toBe(false);
  });

  it('isCustom is true when bufferPreset is custom', () => {
    useAppStore.getState().updateSettings({ bufferPreset: 'custom' });
    const { result } = renderHook(() => useCustomPresetEditor());
    expect(result.current.isCustom).toBe(true);
  });

  it('canSave is false when presetName is empty', () => {
    useAppStore.getState().updateSettings({ bufferPreset: 'custom' });
    const { result } = renderHook(() => useCustomPresetEditor());
    expect(result.current.canSave).toBe(false);
  });

  it('canSave is true when presetName is set and config is valid', () => {
    useAppStore.getState().updateSettings({ bufferPreset: 'custom' });
    const { result } = renderHook(() => useCustomPresetEditor());
    act(() => result.current.setPresetName('My Preset'));
    expect(result.current.canSave).toBe(true);
  });

  it('canSave is false when config matches original saved preset and name is unchanged', () => {
    const savedConfig: JitterConfig = { ...autoConfig };
    useAppStore.getState().updateSettings({
      bufferPreset: 'saved-0',
      customJitterConfig: savedConfig,
      savedPresets: [{ name: 'Existing', config: savedConfig }],
    });
    const { result } = renderHook(() => useCustomPresetEditor());
    expect(result.current.canSave).toBe(false);
  });

  it('handleSave adds preset to savedPresets', () => {
    useAppStore.getState().updateSettings({ bufferPreset: 'custom' });
    const { result } = renderHook(() => useCustomPresetEditor());
    act(() => result.current.setPresetName('New Preset'));
    act(() => result.current.handleSave());
    const settings = useAppStore.getState().settings;
    expect(settings.savedPresets.length).toBe(1);
    expect(settings.savedPresets[0].name).toBe('New Preset');
  });

  it('handleSave retains presetName after saving to show editing state', () => {
    useAppStore.getState().updateSettings({ bufferPreset: 'custom' });
    const { result } = renderHook(() => useCustomPresetEditor());
    act(() => result.current.setPresetName('Test'));
    act(() => result.current.handleSave());
    expect(result.current.presetName).toBe('Test');
  });

  it('handleReset returns Auto config when editing unsaved preset', () => {
    useAppStore.getState().updateSettings({
      bufferPreset: 'custom',
      customJitterConfig: {
        minDepthMs: 999,
        comfortCapMs: 999,
        peakDecayHalflifeMs: 999,
        resumeThresholdPct: 0.99,
      },
    });
    const { result } = renderHook(() => useCustomPresetEditor());
    act(() => result.current.handleReset());
    const config = useAppStore.getState().settings.customJitterConfig;
    expect(config.minDepthMs).toBe(8); // Auto preset value
    expect(config.comfortCapMs).toBe(1500); // Auto preset value
  });

  it('handleReset returns saved config when editing a saved preset', () => {
    const savedConfig: JitterConfig = {
      minDepthMs: 42,
      comfortCapMs: 200,
      peakDecayHalflifeMs: 1000,
      resumeThresholdPct: 0.5,
    };
    useAppStore.getState().updateSettings({
      bufferPreset: 'saved-0',
      customJitterConfig: savedConfig,
      savedPresets: [{ name: 'My Saved', config: savedConfig }],
    });
    const { result } = renderHook(() => useCustomPresetEditor());
    // Modify config
    act(() => result.current.updateField({ minDepthMs: 999 }));
    act(() => result.current.handleReset());
    const config = useAppStore.getState().settings.customJitterConfig;
    // Reset should return original saved config
    expect(config.minDepthMs).toBe(42);
  });

  it('delete flow: requestDelete opens dialog, cancelDelete closes it', () => {
    useAppStore.getState().updateSettings({ bufferPreset: 'custom' });
    const { result } = renderHook(() => useCustomPresetEditor());
    expect(result.current.isDeleteDialogOpen).toBe(false);
    act(() => result.current.requestDelete());
    expect(result.current.isDeleteDialogOpen).toBe(true);
    act(() => result.current.cancelDelete());
    expect(result.current.isDeleteDialogOpen).toBe(false);
  });

  it('confirmDelete removes preset and sets config to Auto', () => {
    const savedConfig: JitterConfig = { ...autoConfig, minDepthMs: 99 };
    useAppStore.getState().updateSettings({
      bufferPreset: 'saved-0',
      customJitterConfig: savedConfig,
      savedPresets: [{ name: 'To Delete', config: savedConfig }],
    });
    const { result } = renderHook(() => useCustomPresetEditor());
    act(() => result.current.requestDelete());
    act(() => result.current.confirmDelete());
    const settings = useAppStore.getState().settings;
    expect(settings.savedPresets.length).toBe(0);
    expect(settings.bufferPreset).toBe('custom');
    expect(settings.customJitterConfig.minDepthMs).toBe(8); // Auto
  });

  it('setBufferMode to static sets staticTargetMs', () => {
    useAppStore.getState().updateSettings({ bufferPreset: 'custom' });
    const { result } = renderHook(() => useCustomPresetEditor());
    act(() => result.current.setBufferMode('static'));
    expect(result.current.bufferMode).toBe('static');
    const config = useAppStore.getState().settings.customJitterConfig;
    expect(config.staticTargetMs).toBe(60);
  });

  it('setBufferMode to adaptive clears staticTargetMs', () => {
    useAppStore.getState().updateSettings({
      bufferPreset: 'custom',
      customJitterConfig: { ...autoConfig, staticTargetMs: 60 },
    });
    const { result } = renderHook(() => useCustomPresetEditor());
    act(() => result.current.setBufferMode('adaptive'));
    expect(result.current.bufferMode).toBe('adaptive');
    const config = useAppStore.getState().settings.customJitterConfig;
    expect(config.staticTargetMs).toBeNull();
  });

  it('isValid is false when config has invalid values', () => {
    useAppStore.getState().updateSettings({
      bufferPreset: 'custom',
      customJitterConfig: {
        minDepthMs: NaN,
        comfortCapMs: 150,
        peakDecayHalflifeMs: 3500,
        resumeThresholdPct: 0.4,
      },
    });
    const { result } = renderHook(() => useCustomPresetEditor());
    expect(result.current.isValid).toBe(false);
    expect(result.current.canSave).toBe(false);
  });

  it('isEditingSaved is true when bufferPreset is saved-X', () => {
    const savedConfig: JitterConfig = { ...autoConfig, minDepthMs: 77 };
    useAppStore.getState().updateSettings({
      bufferPreset: 'saved-0',
      customJitterConfig: savedConfig,
      savedPresets: [{ name: 'Saved', config: savedConfig }],
    });
    const { result } = renderHook(() => useCustomPresetEditor());
    expect(result.current.isEditingSaved).toBe(true);
  });
});
