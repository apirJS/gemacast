import { describe, it, expect, beforeEach } from 'bun:test';
import { render, screen, fireEvent, cleanup } from '@testing-library/react';
import { useAppStore } from '../../stores/app-store';
import { Status } from '../../core/types';
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
    useAppStore.getState().patch({ isLoading: true, connectingStreamerId: 'manual-10.0.0.1' });
    expandAndRender();
    const input = screen.getByPlaceholderText('192.xx.xx.xx');
    expect(input.hasAttribute('disabled')).toBe(true);
  });

  describe('visibility', () => {
    // The card is a way to reach a PC discovery missed, so it is dead weight
    // once a stream is running — but it must stay up through every state where
    // the user might still need to reach something.
    it.each([
      ['Connected', Status.Connected],
      ['Playing', Status.Playing],
      ['Paused', Status.Paused],
      ['Reconnecting', Status.Reconnecting],
    ])('renders nothing when %s', (_name, status) => {
      useAppStore.getState().setStatus(status);
      const { container } = render(<ManualConnect />);
      expect(container.innerHTML).toBe('');
    });

    it.each([
      ['Idle', Status.Idle],
      ['Listening', Status.Listening],
      ['Connecting', Status.Connecting],
    ])('stays visible when %s', (_name, status) => {
      useAppStore.getState().setStatus(status);
      render(<ManualConnect />);
      expect(screen.getByText('Connect by Address')).toBeTruthy();
    });

    it('comes back after disconnecting', () => {
      useAppStore.getState().setStatus(Status.Playing);
      const { container, rerender } = render(<ManualConnect />);
      expect(container.innerHTML).toBe('');

      useAppStore.getState().setStatus(Status.Listening);
      rerender(<ManualConnect />);
      expect(screen.getByText('Connect by Address')).toBeTruthy();
    });
  });
});
