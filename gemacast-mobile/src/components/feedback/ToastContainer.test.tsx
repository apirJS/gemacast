import { describe, it, expect, beforeEach } from 'bun:test';
import { render, screen, cleanup } from '@testing-library/react';
import { useToastStore } from '../../stores/toast-store';
import { ToastContainer } from './ToastContainer';

beforeEach(() => {
  cleanup();
  useToastStore.setState({ toasts: [] });
});

describe('ToastContainer', () => {
  it('renders nothing when no toasts', () => {
    const { container } = render(<ToastContainer />);
    expect(container.innerHTML).toBe('');
  });

  it('renders a toast message', () => {
    useToastStore.getState().show('info', 'Hello world');
    render(<ToastContainer />);
    expect(screen.getByText('Hello world')).toBeTruthy();
  });

  it('renders multiple toasts', () => {
    useToastStore.getState().show('success', 'First');
    useToastStore.getState().show('warning', 'Second');
    render(<ToastContainer />);
    expect(screen.getByText('First')).toBeTruthy();
    expect(screen.getByText('Second')).toBeTruthy();
  });

  it('renders toast with alert role', () => {
    useToastStore.getState().show('error', 'Bad thing');
    render(<ToastContainer />);
    expect(screen.getByRole('alert')).toBeTruthy();
  });
});
