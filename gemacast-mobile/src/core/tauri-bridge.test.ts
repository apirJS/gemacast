import { describe, it, expect, beforeEach } from 'bun:test';
import { tauriBridge } from './tauri-bridge';
import { ConnectionMode } from './types';
import { invokeCalls, setupInvokeMock } from '../__tests__/setup';

beforeEach(() => {
  // Clear the invoke calls before each test
  invokeCalls.length = 0;
  setupInvokeMock();
});

describe('tauriBridge', () => {
  it('connectToSender parses raw bitrate preset to null', async () => {
    await tauriBridge.connectToSender({
      ip: '10.0.0.1',
      deviceId: 'dev1',
      deviceName: 'Device',
      mode: ConnectionMode.Wifi,
      exclusiveMode: true,
      jitterConfig: { minDepthMs: 5, comfortCapMs: 100, peakDecayHalflifeMs: 250, resumeThresholdPct: 0.95 },
      bitratePreset: 'raw',
      customBitrateKbps: 0,
      transport: null,
    });
    
    expect(invokeCalls).toHaveLength(1);
    expect(invokeCalls[0].cmd).toBe('connect_to_sender');
    expect((invokeCalls[0].args as Record<string, unknown>).bitrate).toBeNull();
  });

  it('connectToSender parses custom bitrate preset', async () => {
    await tauriBridge.connectToSender({
      ip: '10.0.0.1',
      deviceId: 'dev1',
      deviceName: 'Device',
      mode: ConnectionMode.Wifi,
      exclusiveMode: true,
      jitterConfig: { minDepthMs: 5, comfortCapMs: 100, peakDecayHalflifeMs: 250, resumeThresholdPct: 0.95 },
      bitratePreset: 'custom',
      customBitrateKbps: 128,
      transport: null,
    });
    
    expect(invokeCalls[0].cmd).toBe('connect_to_sender');
    expect((invokeCalls[0].args as Record<string, unknown>).bitrate).toBe(128000); // 128 kbps -> 128000 bps
  });

  it('disconnectFromSender passes correct args', async () => {
    await tauriBridge.disconnectFromSender({ ip: '10.0.0.1', deviceId: 'dev1' });
    expect(invokeCalls[0].cmd).toBe('disconnect_from_sender');
    expect((invokeCalls[0].args as Record<string, unknown>).deviceId).toBe('dev1');
  });

  it('getNetworkState invokes correct command', async () => {
    await tauriBridge.getNetworkState();
    expect(invokeCalls[0].cmd).toBe('get_network_state');
  });

  it('startListeningForSenders passes correct args', async () => {
    await tauriBridge.startListeningForSenders({ deviceId: 'dev1', mode: ConnectionMode.Wifi });
    expect(invokeCalls[0].cmd).toBe('start_listening_for_senders');
    expect((invokeCalls[0].args as Record<string, unknown>).deviceId).toBe('dev1');
    expect((invokeCalls[0].args as Record<string, unknown>).mode).toBe(ConnectionMode.Wifi);
  });
  
  it('changeAudioSource passes correct args', async () => {
    await tauriBridge.changeAudioSource({ ip: '10.0.0.1', deviceId: 'dev1', source: { type: 'desktop' } });
    expect(invokeCalls[0].cmd).toBe('change_audio_source');
    expect(((invokeCalls[0].args as Record<string, unknown>).source as Record<string, unknown>).type).toBe('desktop');
  });

  it('getNetworkIdentifier invokes correct command', async () => {
    await tauriBridge.getNetworkIdentifier();
    expect(invokeCalls[0].cmd).toBe('get_network_identifier');
  });

  it('updateJitterConfig passes correct args', async () => {
    const jitterConfig = { minDepthMs: 5, comfortCapMs: 100, peakDecayHalflifeMs: 250, resumeThresholdPct: 0.95 };
    await tauriBridge.updateJitterConfig({ jitterConfig });
    expect(invokeCalls[0].cmd).toBe('update_jitter_config');
    expect((invokeCalls[0].args as Record<string, unknown>).jitterConfig).toEqual(jitterConfig);
  });

  it('changeAudioBitrate passes correct args', async () => {
    await tauriBridge.changeAudioBitrate({ ip: '127.0.0.1', deviceId: 'test-device', bitrate: 128000 });
    expect(invokeCalls[0].cmd).toBe('change_audio_bitrate');
    expect((invokeCalls[0].args as Record<string, unknown>).ip).toBe('127.0.0.1');
    expect((invokeCalls[0].args as Record<string, unknown>).deviceId).toBe('test-device');
    expect((invokeCalls[0].args as Record<string, unknown>).bitrate).toBe(128000);
  });
});
