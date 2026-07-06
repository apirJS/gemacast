import { describe, it, expect, beforeEach } from 'bun:test';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { ThemeToggle } from './ThemeToggle';
import { useAppStore } from '../../stores/app-store';

beforeEach(() => {
  cleanup();
  document.documentElement.className = '';
  useAppStore.getState().updateSettings({ theme: 'dark' });
});

describe('ThemeToggle', () => {
  it('renders a toggle button', () => {
    render(<ThemeToggle />);
    expect(screen.getByRole('button', { name: 'Toggle Theme' })).toBeTruthy();
  });

  it('updates store and document class when toggled', () => {
    render(<ThemeToggle />);
    const button = screen.getByRole('button');

    fireEvent.click(button);

    expect(useAppStore.getState().settings.theme).toBe('light');
    expect(document.documentElement.classList.contains('light')).toBe(true);
    expect(document.documentElement.classList.contains('dark')).toBe(false);
  });

  it('toggles back to dark', () => {
    useAppStore.getState().updateSettings({ theme: 'light' });
    render(<ThemeToggle />);
    const button = screen.getByRole('button');

    fireEvent.click(button);

    expect(useAppStore.getState().settings.theme).toBe('dark');
    expect(document.documentElement.classList.contains('dark')).toBe(true);
    expect(document.documentElement.classList.contains('light')).toBe(false);
  });
});
