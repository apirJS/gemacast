import * as z from 'zod';
import { GemaCastError } from './error';

export type Ok<T> = {
  readonly ok: true;
  readonly value: T;
};

export type Err<E> = {
  readonly ok: false;
  readonly error: E;
};

export type Result<T, E = Error> = Ok<T> | Err<E>;

export function ok<T>(value: T): Ok<T> {
  return { ok: true, value };
}
export function err<E>(error: E): Err<E> {
  return { ok: false, error };
}

export const deviceInfoSchema = z.object({
  deviceId: z.uuid(),
  deviceName: z.string(),
  ip: z.string(),
});

export type DeviceInfo = z.infer<typeof deviceInfoSchema>;

export enum Status {
  Idle = 'idle',
  Listening = 'listening',
  Connected = 'connected',
  Playing = 'playing',
}

export type AppState = {
  deviceInfo: DeviceInfo;
  status: Status;
  senderIp: string | null;
  error: GemaCastError | null;
  volume: number;
};

export type StateSubscriber = (state: AppState) => void;
