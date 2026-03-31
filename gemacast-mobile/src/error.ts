export enum ErrorCode {
  NETWORK_FAILED_TO_START_DISCOVERY = 'NETWORK_FAILED_TO_START_DISCOVERY',
  NETWORK_FAILED_TO_STOP_DISCOVERY = 'NETWORK_FAILED_TO_STOP_DISCOVERY',
  NETWORK_DISCOVERY_ERROR = 'NETWORK_DISCOVERY_ERROR',
  AUDIO_PLAYBACK_ERROR = 'AUDIO_PLAYBACK_ERROR',
  AUDIO_FAILED_TO_START_PLAYBACK = 'AUDIO_FAILED_TO_START_PLAYBACK',
  AUDIO_FAILED_TO_STOP_PLAYBACK = 'AUDIO_FAILED_TO_STOP_PLAYBACK',
  UNKNOWN_ERROR = 'UNKNOWN_ERROR',
}

export type ErrorOptions = {
  code: ErrorCode;
  message?: string;
  cause?: unknown;
};

export const ERROR_MESSAGES: Record<ErrorCode, string> = {
  [ErrorCode.NETWORK_FAILED_TO_START_DISCOVERY]:
    'failed to start udp discovery',
  [ErrorCode.NETWORK_FAILED_TO_STOP_DISCOVERY]: 'failed to stop udp discovery',
  [ErrorCode.NETWORK_DISCOVERY_ERROR]: 'an error occurred during background discovery',
  [ErrorCode.AUDIO_PLAYBACK_ERROR]: 'an error occurred during audio playback',
  [ErrorCode.AUDIO_FAILED_TO_START_PLAYBACK]: 'failed to start audio playback',
  [ErrorCode.AUDIO_FAILED_TO_STOP_PLAYBACK]: 'failed to stop audio playback',
  [ErrorCode.UNKNOWN_ERROR]: 'unknown error occured',
};

export class GemaCastError extends Error {
  public readonly code: ErrorCode;
  public readonly userMessage: string;

  constructor(options: ErrorOptions) {
    super(options.message ?? ERROR_MESSAGES[options.code], {
      cause: options.cause,
    });

    this.name = 'GemaCastError';
    this.code = options.code;
    this.userMessage = ERROR_MESSAGES[this.code];
  }

  public static failedToStartDiscovery(error: unknown) {
    return new GemaCastError({
      code: ErrorCode.NETWORK_FAILED_TO_START_DISCOVERY,
      message: ERROR_MESSAGES[ErrorCode.NETWORK_FAILED_TO_START_DISCOVERY],
      cause: error,
    });
  }

  public static failedToStopDiscovery(error: unknown) {
    return new GemaCastError({
      code: ErrorCode.NETWORK_FAILED_TO_STOP_DISCOVERY,
      message: ERROR_MESSAGES[ErrorCode.NETWORK_FAILED_TO_STOP_DISCOVERY],
      cause: error,
    });
  }

  public static discoveryError(error: unknown) {
    return new GemaCastError({
      code: ErrorCode.NETWORK_DISCOVERY_ERROR,
      message: ERROR_MESSAGES[ErrorCode.NETWORK_DISCOVERY_ERROR],
      cause: error,
    });
  }

  public static playbackError(error: unknown) {
    return new GemaCastError({
      code: ErrorCode.AUDIO_PLAYBACK_ERROR,
      message: ERROR_MESSAGES[ErrorCode.AUDIO_PLAYBACK_ERROR],
      cause: error,
    });
  }

  public static failedToStartPlayback(error: unknown) {
    return new GemaCastError({
      code: ErrorCode.AUDIO_FAILED_TO_START_PLAYBACK,
      message: ERROR_MESSAGES[ErrorCode.AUDIO_FAILED_TO_START_PLAYBACK],
      cause: error,
    });
  }

  public static failedToStopPlayback(error: unknown) {
    return new GemaCastError({
      code: ErrorCode.AUDIO_FAILED_TO_STOP_PLAYBACK,
      message: ERROR_MESSAGES[ErrorCode.AUDIO_FAILED_TO_STOP_PLAYBACK],
      cause: error,
    });
  }

  public static from(error: unknown, fallbackCode = ErrorCode.UNKNOWN_ERROR) {
    if (error instanceof GemaCastError) {
      return error;
    }

    if (error instanceof Error) {
      return new GemaCastError({
        code: fallbackCode,
        message: error.message,
        cause: error,
      });
    }

    return new GemaCastError({
      code: fallbackCode,
      message: String(error),
    });
  }
}
