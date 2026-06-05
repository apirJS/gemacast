import { describe, it, expect, beforeEach, mock } from 'bun:test';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { ConfirmDialog } from './ConfirmDialog';

beforeEach(() => {
  cleanup();
  HTMLDialogElement.prototype.showModal = mock();
  HTMLDialogElement.prototype.close = mock();
});

describe('ConfirmDialog', () => {
  it('calls showModal when open is true', () => {
    render(
      <ConfirmDialog open={true} message="Are you sure?" onConfirm={mock()} onCancel={mock()} />,
    );
    expect(HTMLDialogElement.prototype.showModal).toHaveBeenCalled();
  });

  it('renders message and buttons', () => {
    render(
      <ConfirmDialog open={true} message="Are you sure?" onConfirm={mock()} onCancel={mock()} />,
    );
    expect(screen.getByText('Are you sure?')).toBeTruthy();
    expect(screen.getByText('Delete')).toBeTruthy();
    expect(screen.getByText('Cancel')).toBeTruthy();
  });

  it('calls onConfirm when confirm button clicked', () => {
    const onConfirm = mock();
    render(<ConfirmDialog open={true} message="Test" onConfirm={onConfirm} onCancel={mock()} />);
    fireEvent.click(screen.getByText('Delete'));
    expect(onConfirm).toHaveBeenCalled();
  });

  it('calls onCancel when cancel button clicked', () => {
    const onCancel = mock();
    render(<ConfirmDialog open={true} message="Test" onConfirm={mock()} onCancel={onCancel} />);
    fireEvent.click(screen.getByText('Cancel'));
    expect(onCancel).toHaveBeenCalled();
  });

  it('calls close when open becomes false', () => {
    const { rerender } = render(
      <ConfirmDialog open={true} message="Test" onConfirm={mock()} onCancel={mock()} />,
    );
    // Mock open property on dialog
    Object.defineProperty(HTMLDialogElement.prototype, 'open', {
      get: () => true,
      configurable: true,
    });

    rerender(<ConfirmDialog open={false} message="Test" onConfirm={mock()} onCancel={mock()} />);
    expect(HTMLDialogElement.prototype.close).toHaveBeenCalled();
  });
});
