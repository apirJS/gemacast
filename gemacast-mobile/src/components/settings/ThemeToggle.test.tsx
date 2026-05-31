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
  it('renders correctly based on theme', () => {
    render(<ThemeToggle />);
    expect(screen.getByText('☾')).toBeTruthy();
  });

  it('renders sun icon for light theme', () => {
    useAppStore.getState().updateSettings({ theme: 'light' });
    render(<ThemeToggle />);
    expect(screen.getByText('☼')).toBeTruthy();
  });

  it('updates store and document class when toggled', () => {
    render(<ThemeToggle />);
    const button = screen.getByRole('button');
    
    fireEvent.click(button);
    
    expect(useAppStore.getState().settings.theme).toBe('light');
    expect(document.documentElement.classList.contains('light')).toBe(true);
    expect(document.documentElement.classList.contains('dark')).toBe(false);
  });
});
