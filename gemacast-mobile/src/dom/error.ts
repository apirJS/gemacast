import { App } from '../App';
import { AppState } from '../types';
import { toastManager } from './toast';

export function setupErrorSection(app: App) {
  app.stateHandler.subscribe((state: AppState) => {
    if (!state.error) {
      toastManager.clearError();
      return;
    }

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