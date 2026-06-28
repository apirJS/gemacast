import { create } from 'zustand';

export type UpdateStatus =
  | 'idle'
  | 'checking'
  | 'available'
  | 'downloading'
  | 'ready'
  | 'installing'
  | 'error'
  | 'up-to-date';

export type UpdateState = {
  status: UpdateStatus;
  version: string | null;
  downloadUrl: string | null;
  sha256: string | null;
  apkPath: string | null;
  percent: number;
  errorMessage: string | null;
};

type UpdateActions = {
  setChecking: () => void;
  setAvailable: (version: string, downloadUrl: string, sha256: string | null) => void;
  setUpToDate: () => void;
  setDownloading: (percent: number) => void;
  setReady: (version: string, apkPath: string) => void;
  setInstalling: () => void;
  setError: (message: string) => void;
  /** Reset to idle so the user can retry the entire flow. */
  reset: () => void;
  /**
   * Called when the app returns to the foreground after an install attempt.
   * If we were in 'installing' state, transition back to 'ready' so the
   * user can retry.
   */
  handleAppResume: () => void;
};

export type UpdateStore = UpdateState & UpdateActions;

const INITIAL_STATE: UpdateState = {
  status: 'idle',
  version: null,
  downloadUrl: null,
  sha256: null,
  apkPath: null,
  percent: 0,
  errorMessage: null,
};

export const useUpdateStore = create<UpdateStore>((set, get) => ({
  ...INITIAL_STATE,

  setChecking: () => set({ ...INITIAL_STATE, status: 'checking' }),

  setAvailable: (version, downloadUrl, sha256) =>
    set({
      status: 'available',
      version,
      downloadUrl,
      sha256,
      apkPath: null,
      percent: 0,
      errorMessage: null,
    }),

  setUpToDate: () => set({ ...INITIAL_STATE, status: 'up-to-date' }),

  setDownloading: (percent) => set({ status: 'downloading', percent, errorMessage: null }),

  setReady: (version, apkPath) =>
    set({ status: 'ready', version, apkPath, percent: 100, errorMessage: null }),

  setInstalling: () => set({ status: 'installing', errorMessage: null }),

  setError: (message) => set({ status: 'error', errorMessage: message }),

  reset: () => set(INITIAL_STATE),

  handleAppResume: () => {
    const { status } = get();
    // If we were in 'installing' and the user returned to the app,
    // the system installer was dismissed — go back to 'ready'.
    if (status === 'installing') {
      const { version, apkPath } = get();
      if (version && apkPath) {
        set({ status: 'ready' });
      } else {
        set(INITIAL_STATE);
      }
    }
  },
}));
