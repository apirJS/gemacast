import { describe, it, expect } from 'bun:test';
import { GemaCastError, ErrorCode, ERROR_MESSAGES } from './error';

describe('GemaCastError factory methods', () => {
  it('senderTimeout sets code NETWORK_SENDER_TIMEOUT', () => {
    const err = GemaCastError.senderTimeout();
    expect(err.code).toBe(ErrorCode.NETWORK_SENDER_TIMEOUT);
  });

  it('senderTimeout uses default message from ERROR_MESSAGES', () => {
    const err = GemaCastError.senderTimeout();
    expect(err.userMessage).toBe(ERROR_MESSAGES[ErrorCode.NETWORK_SENDER_TIMEOUT]);
  });

  it('failedToStartDiscovery preserves "already in use" message', () => {
    const err = GemaCastError.failedToStartDiscovery('Port already in use on 0.0.0.0:9000');
    expect(err.userMessage).toBe('Port already in use on 0.0.0.0:9000');
  });

  it('failedToStartDiscovery uses default for generic errors', () => {
    const err = GemaCastError.failedToStartDiscovery(new Error('something'));
    expect(err.userMessage).toBe(ERROR_MESSAGES[ErrorCode.NETWORK_FAILED_TO_START_DISCOVERY]);
  });

  it('creates failedToStopDiscovery error', () => {
    const err = GemaCastError.failedToStopDiscovery('reason');
    expect(err.code).toBe(ErrorCode.NETWORK_FAILED_TO_STOP_DISCOVERY);
    expect(err.cause).toBe('reason');
  });

  it('creates discoveryError', () => {
    const err = GemaCastError.discoveryError('reason');
    expect(err.code).toBe(ErrorCode.NETWORK_DISCOVERY_ERROR);
    expect(err.cause).toBe('reason');
  });

  it('creates playbackError', () => {
    const err = GemaCastError.playbackError('reason');
    expect(err.code).toBe(ErrorCode.AUDIO_PLAYBACK_ERROR);
    expect(err.cause).toBe('reason');
  });

  it('creates failedToStartPlayback error', () => {
    const err = GemaCastError.failedToStartPlayback('reason');
    expect(err.code).toBe(ErrorCode.AUDIO_FAILED_TO_START_PLAYBACK);
    expect(err.cause).toBe('reason');
  });

  it('creates failedToStopPlayback error', () => {
    const err = GemaCastError.failedToStopPlayback('reason');
    expect(err.code).toBe(ErrorCode.AUDIO_FAILED_TO_STOP_PLAYBACK);
    expect(err.cause).toBe('reason');
  });

  it('reconnectFailed sets correct code', () => {
    const err = GemaCastError.reconnectFailed();
    expect(err.code).toBe(ErrorCode.NETWORK_RECONNECT_FAILED);
  });
});

describe('GemaCastError.from', () => {
  it('returns same instance for GemaCastError input', () => {
    const original = GemaCastError.senderTimeout();
    const result = GemaCastError.from(original);
    expect(result).toBe(original);
  });

  it('wraps a native Error', () => {
    const native = new Error('native fail');
    const wrapped = GemaCastError.from(native);
    expect(wrapped).toBeInstanceOf(GemaCastError);
    expect(wrapped.userMessage).toBe('native fail');
    expect(wrapped.cause).toBe(native);
    expect(wrapped.code).toBe(ErrorCode.UNKNOWN_ERROR);
  });

  it('wraps a plain string', () => {
    const wrapped = GemaCastError.from('oops');
    expect(wrapped).toBeInstanceOf(GemaCastError);
    expect(wrapped.userMessage).toBe('oops');
  });

  it('uses custom fallbackCode', () => {
    const wrapped = GemaCastError.from('bad', ErrorCode.AUDIO_PLAYBACK_ERROR);
    expect(wrapped.code).toBe(ErrorCode.AUDIO_PLAYBACK_ERROR);
  });
});
