import { useSettings } from '../../hooks/use-settings';
import { Sun, Moon } from 'lucide-react';

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
      className={`
        relative flex h-9 w-9 items-center justify-center rounded-full
        transition-all duration-300 ease-out
        hover:bg-accent active:scale-90
      `}
      onClick={toggle}
      aria-label="Toggle Theme"
    >
      {/* Sun icon */}
      <Sun
        className={`
          absolute h-[18px] w-[18px] text-amber-400
          transition-all duration-300 ease-[cubic-bezier(0.16,1,0.3,1)]
          ${isDark ? 'rotate-90 scale-0 opacity-0' : 'rotate-0 scale-100 opacity-100'}
        `}
        strokeWidth={2}
      />
      {/* Moon icon */}
      <Moon
        className={`
          absolute h-[18px] w-[18px] text-blue-300
          transition-all duration-300 ease-[cubic-bezier(0.16,1,0.3,1)]
          ${isDark ? 'rotate-0 scale-100 opacity-100' : '-rotate-90 scale-0 opacity-0'}
        `}
        strokeWidth={2}
      />
    </button>
  );
}
