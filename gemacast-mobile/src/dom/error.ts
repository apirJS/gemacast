import { App } from '../App';
import { AppState } from '../types';

export function setupErrorSection(app: App) {
  const errorSectionEl = document.getElementById(
    'error-section',
  ) as HTMLDivElement | null;
  const errorMessageEl = document.getElementById(
    'error-message',
  ) as HTMLSpanElement | null;
  const errorDetailTextEl = document.getElementById(
    'error-detail-text',
  ) as HTMLParagraphElement | null;
  const errorDismissEl = document.getElementById(
    'error-dismiss-btn',
  ) as HTMLButtonElement | null;

  errorDismissEl?.addEventListener('click', () => {
    app.stateHandler.dismissError();
  });

  app.stateHandler.subscribe((state: AppState) => {
    if (!errorSectionEl) return;
    
    errorSectionEl.hidden = state.error === null;

    if (state.error && errorMessageEl) {
      errorMessageEl.textContent = state.error.userMessage;
    }
    if (state.error && errorDetailTextEl) {
      let detail = 'No additional details available.';
      if (state.error.cause instanceof Error) {
        detail = state.error.cause.message;
      } else if (typeof state.error.cause === 'string') {
        detail = state.error.cause;
      } else if (state.error.cause != null) {
        detail = String(state.error.cause);
      }
      errorDetailTextEl.textContent = detail;
    }
  });
}