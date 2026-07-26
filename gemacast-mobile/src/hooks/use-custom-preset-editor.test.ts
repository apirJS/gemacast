import { describe, it, expect, beforeEach } from 'bun:test';
import { renderHook, act, cleanup } from '@testing-library/react';
import { useAppStore } from '../stores/app-store';
import { useCustomPresetEditor } from './use-custom-preset-editor';
import type { JitterConfig } from '../core/types';

const staticConfig: JitterConfig = {
  minDepthMs: 25,
  comfortCapMs: 1000,
  peakDecayHalflifeMs: 0,
  resumeThresholdPct: 0.25,
  staticTargetMs: 60,
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
    useAppStore.getState().updateSettings({
      bufferPreset: 'custom',
      customJitterConfig: staticConfig,
    });
    const { result } = renderHook(() => useCustomPresetEditor());
    expect(result.current.isCustom).toBe(true);
  });

  it('canSave is false when presetName is empty', () => {
    useAppStore.getState().updateSettings({
      bufferPreset: 'custom',
      customJitterConfig: staticConfig,
    });
    const { result } = renderHook(() => useCustomPresetEditor());
    expect(result.current.canSave).toBe(false);
  });

  it('canSave is true when presetName is set and config is valid', () => {
    useAppStore.getState().updateSettings({
      bufferPreset: 'custom',
      customJitterConfig: staticConfig,
    });
    const { result } = renderHook(() => useCustomPresetEditor());
    act(() => result.current.setPresetName('My Preset'));
    expect(result.current.canSave).toBe(true);
  });

  it('canSave is false when config matches original saved preset and name is unchanged', () => {
    const savedConfig: JitterConfig = { ...staticConfig };
    useAppStore.getState().updateSettings({
      bufferPreset: 'saved-0',
      customJitterConfig: savedConfig,
      savedPresets: [{ name: 'Existing', config: savedConfig }],
    });
    const { result } = renderHook(() => useCustomPresetEditor());
    expect(result.current.canSave).toBe(false);
  });

  it('handleSave adds preset to savedPresets', () => {
    useAppStore.getState().updateSettings({
      bufferPreset: 'custom',
      customJitterConfig: staticConfig,
    });
    const { result } = renderHook(() => useCustomPresetEditor());
    act(() => result.current.setPresetName('New Preset'));
    act(() => result.current.handleSave());
    const settings = useAppStore.getState().settings;
    expect(settings.savedPresets.length).toBe(1);
    expect(settings.savedPresets[0].name).toBe('New Preset');
  });

  it('handleSave retains presetName after saving to show editing state', () => {
    useAppStore.getState().updateSettings({
      bufferPreset: 'custom',
      customJitterConfig: staticConfig,
    });
    const { result } = renderHook(() => useCustomPresetEditor());
    act(() => result.current.setPresetName('Test'));
    act(() => result.current.handleSave());
    expect(result.current.presetName).toBe('Test');
  });

  it('handleReset returns default static config when editing unsaved preset', () => {
    useAppStore.getState().updateSettings({
      bufferPreset: 'custom',
      customJitterConfig: {
        minDepthMs: 999,
        comfortCapMs: 999,
        peakDecayHalflifeMs: 999,
        resumeThresholdPct: 0.99,
        staticTargetMs: 999,
      },
    });
    const { result } = renderHook(() => useCustomPresetEditor());
    act(() => result.current.handleReset());
    const config = useAppStore.getState().settings.customJitterConfig;
    expect(config.staticTargetMs).toBe(0); // Default static value
  });

  it('handleReset returns saved config when editing a saved preset', () => {
    const savedConfig: JitterConfig = {
      minDepthMs: 42,
      comfortCapMs: 200,
      peakDecayHalflifeMs: 1000,
      resumeThresholdPct: 0.5,
      staticTargetMs: 75,
    };
    useAppStore.getState().updateSettings({
      bufferPreset: 'saved-0',
      customJitterConfig: savedConfig,
      savedPresets: [{ name: 'My Saved', config: savedConfig }],
    });
    const { result } = renderHook(() => useCustomPresetEditor());
    // Modify config
    act(() => result.current.updateField({ staticTargetMs: 999 }));
    act(() => result.current.handleReset());
    const config = useAppStore.getState().settings.customJitterConfig;
    // Reset should return original saved config
    expect(config.staticTargetMs).toBe(75);
  });

  it('delete flow: requestDelete opens dialog, cancelDelete closes it', () => {
    useAppStore.getState().updateSettings({
      bufferPreset: 'custom',
      customJitterConfig: staticConfig,
    });
    const { result } = renderHook(() => useCustomPresetEditor());
    expect(result.current.isDeleteDialogOpen).toBe(false);
    act(() => result.current.requestDelete());
    expect(result.current.isDeleteDialogOpen).toBe(true);
    act(() => result.current.cancelDelete());
    expect(result.current.isDeleteDialogOpen).toBe(false);
  });

  it('confirmDelete removes preset and sets config to default static', () => {
    const savedConfig: JitterConfig = { ...staticConfig, staticTargetMs: 99 };
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
    expect(settings.customJitterConfig.staticTargetMs).toBe(0); // Default
  });

  it('isValid is false when staticTargetMs is invalid', () => {
    useAppStore.getState().updateSettings({
      bufferPreset: 'custom',
      customJitterConfig: {
        minDepthMs: 25,
        comfortCapMs: 1000,
        peakDecayHalflifeMs: 0,
        resumeThresholdPct: 0.25,
        staticTargetMs: -1,
      },
    });
    const { result } = renderHook(() => useCustomPresetEditor());
    expect(result.current.isValid).toBe(false);
    expect(result.current.canSave).toBe(false);
  });

  it('isEditingSaved is true when bufferPreset is saved-X', () => {
    const savedConfig: JitterConfig = { ...staticConfig, staticTargetMs: 77 };
    useAppStore.getState().updateSettings({
      bufferPreset: 'saved-0',
      customJitterConfig: savedConfig,
      savedPresets: [{ name: 'Saved', config: savedConfig }],
    });
    const { result } = renderHook(() => useCustomPresetEditor());
    expect(result.current.isEditingSaved).toBe(true);
  });
});
