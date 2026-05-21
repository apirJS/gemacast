import { App } from '../../App';
import { AppState } from '../../types';
import { toastManager } from '../toast';
import { GemaCastError } from '../../error';

export function setupErrorSection(app: App) {
  let lastError: GemaCastError | null = null;

  app.stateHandler.subscribe((state: AppState) => {
    if (!state.error) {
      toastManager.clearError();
      lastError = null;
      return;
    }

    if (state.error === lastError) {
      return;
    }
    lastError = state.error;

    let detail = 'No additional details available.';
    if (state.error.cause instanceof Error) {
      detail = state.error.cause.message;
    } else if (typeof state.error.cause === 'string') {
      detail = state.error.cause;
    } else if (state.error.cause != null) {
      detail = String(state.error.cause);
    }

    toastManager.showError(state.error.userMessage, detail);
  });
}
