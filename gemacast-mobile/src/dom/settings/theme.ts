import { AppState } from '../../types';
import type { App } from '../../App';

export function initThemeToggle(app: App) {
  const themeBtn = document.getElementById(
    'theme-toggle-btn',
  ) as HTMLButtonElement;

  themeBtn.addEventListener('click', () => {
    const curr = app.stateHandler.getState().settings;
    app.stateHandler.setState({
      settings: {
        ...curr,
        theme: curr.theme === 'dark' ? 'light' : 'dark',
      },
    });
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
