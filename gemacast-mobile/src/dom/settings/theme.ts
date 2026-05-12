import { AppState } from '../../types';
import type { App } from '../../App';
import { toastManager } from '../toast';

export function initThemeToggle(app: App) {
  const themeBtn = document.getElementById(
    'theme-toggle-btn',
  ) as HTMLButtonElement;

  themeBtn.addEventListener('click', () => {
    const curr = app.stateHandler.getState().settings;
    const nextTheme = curr.theme === 'dark' ? 'light' : 'dark';
    app.stateHandler.setState({
      settings: {
        ...curr,
        theme: nextTheme,
      },
    });
    toastManager.showInfo(`Theme set to ${nextTheme}`);
  });

  app.stateHandler.subscribe((state: AppState) => {
    const s = state.settings;
    if (s.theme === 'dark') {
      document.documentElement.classList.add('dark-theme');
      document.documentElement.classList.remove('light-theme');
    } else {
      document.documentElement.classList.remove('dark-theme');
      document.documentElement.classList.add('light-theme');
    }
  });
}
