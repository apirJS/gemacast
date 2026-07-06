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

function expandAndRender() {
  render(<ManualConnect />);
  // Expand the collapsible to reveal the input + connect button
  fireEvent.click(screen.getByText('Connect by Address'));
}

describe('ManualConnect', () => {
  it('renders collapsed header', () => {
    render(<ManualConnect />);
    expect(screen.getByText('Connect by Address')).toBeTruthy();
  });

  it('renders input and connect button when expanded', () => {
    expandAndRender();
    expect(screen.getByPlaceholderText('192.xx.xx.xx')).toBeTruthy();
    expect(screen.getByText('Connect')).toBeTruthy();
  });

  it('disables connect button when input is empty', () => {
    expandAndRender();
    const buttons = screen
      .getAllByRole('button')
      .filter((b) => b.textContent?.trim() === 'Connect');
    expect(buttons[0].hasAttribute('disabled')).toBe(true);
  });

  it('enables connect button when input has text', () => {
    expandAndRender();
    const input = screen.getByPlaceholderText('192.xx.xx.xx');
    fireEvent.change(input, { target: { value: '10.0.0.1' } });
    const buttons = screen
      .getAllByRole('button')
      .filter((b) => b.textContent?.trim() === 'Connect');
    expect(buttons[0].hasAttribute('disabled')).toBe(false);
  });

  it('disables input when loading a manual connection', () => {
    useAppStore.getState().patch({ isLoading: true, connectingSenderId: 'manual-10.0.0.1' });
    expandAndRender();
    const input = screen.getByPlaceholderText('192.xx.xx.xx');
    expect(input.hasAttribute('disabled')).toBe(true);
  });
});
