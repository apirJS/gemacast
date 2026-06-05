import { describe, it, expect, beforeEach } from 'bun:test';
import { render, screen, fireEvent, cleanup } from '@testing-library/react';
import { useAppStore } from '../../stores/app-store';
import { ManualConnect } from './ManualConnect';

beforeEach(() => {
  cleanup();
  useAppStore.getState().init({
    deviceId: 'test',
    deviceName: 'Test',
    ip: '127.0.0.1',
  });
});

describe('ManualConnect', () => {
  it('renders input and connect button', () => {
    render(<ManualConnect />);
    expect(screen.getByPlaceholderText('192.xx.xx.xx')).toBeTruthy();
    expect(screen.getByText('Connect')).toBeTruthy();
  });

  it('disables connect button when input is empty', () => {
    render(<ManualConnect />);
    const btn = screen.getByText('Connect');
    expect(btn.hasAttribute('disabled')).toBe(true);
  });

  it('enables connect button when input has text', () => {
    render(<ManualConnect />);
    const input = screen.getByPlaceholderText('192.xx.xx.xx');
    fireEvent.change(input, { target: { value: '10.0.0.1' } });
    const btn = screen.getByText('Connect');
    expect(btn.hasAttribute('disabled')).toBe(false);
  });

  it('disables input when loading', () => {
    useAppStore.getState().setLoading(true);
    render(<ManualConnect />);
    const input = screen.getByPlaceholderText('192.xx.xx.xx');
    expect(input.hasAttribute('disabled')).toBe(true);
  });
});
