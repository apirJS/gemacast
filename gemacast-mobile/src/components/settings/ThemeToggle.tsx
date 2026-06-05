import { useSettings } from '../../hooks/use-settings';

export function ThemeToggle() {
  const { settings, update } = useSettings();
  const isDark = settings.theme === 'dark';

  const toggle = () => {
    const next = isDark ? 'light' : 'dark';
    update({ theme: next });
    document.documentElement.classList.toggle('dark', next === 'dark');
    document.documentElement.classList.toggle('light', next === 'light');
  };

  return (
    <button
      type="button"
      className="flex h-8 w-8 items-center justify-center rounded-full text-lg transition-colors hover:bg-secondary"
      onClick={toggle}
      aria-label="Toggle Theme"
    >
      {isDark ? '☾' : '☼'}
    </button>
  );
}
