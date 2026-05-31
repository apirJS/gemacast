import { describe, it, expect, beforeEach } from 'bun:test';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { ExclusiveToggle } from './ExclusiveToggle';
import { useAppStore } from '../../stores/app-store';

beforeEach(() => {
  cleanup();
});

describe('ExclusiveToggle', () => {
  it('renders checked based on settings', () => {
    useAppStore.getState().updateSettings({ exclusiveMode: true });
    render(<ExclusiveToggle />);
    const toggle = screen.getByRole('checkbox');
    expect(toggle.hasAttribute('checked')).toBe(true);
  });

  it('calls update when toggled', () => {
    useAppStore.getState().updateSettings({ exclusiveMode: true });
    render(<ExclusiveToggle />);
    const toggle = screen.getByRole('checkbox');
    fireEvent.click(toggle);
    
    expect(useAppStore.getState().settings.exclusiveMode).toBe(false);
  });
});
