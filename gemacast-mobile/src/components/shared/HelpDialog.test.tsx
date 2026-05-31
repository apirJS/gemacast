import { describe, it, expect, beforeEach, mock } from 'bun:test';
import { render, screen, cleanup, fireEvent, renderHook } from '@testing-library/react';
import { HelpDialog, useHelpDialog } from './HelpDialog';

// Mock the help content
mock.module('../../core/help-content', () => ({
  HELP_CONTENT: {
    'test-key': {
      title: 'Test Title',
      body: 'Test Body'
    }
  }
}));

beforeEach(() => {
  cleanup();
  HTMLDialogElement.prototype.showModal = mock();
  HTMLDialogElement.prototype.close = mock();
});

describe('HelpDialog Component', () => {
  it('renders nothing when activeKey is null', () => {
    const { container } = render(
      <HelpDialog activeKey={null} onClose={mock()} dialogRef={{ current: null }} />
    );
    expect(container.textContent).toBe('');
  });

  it('renders content when activeKey is valid', () => {
    render(
      <HelpDialog activeKey="test-key" onClose={mock()} dialogRef={{ current: null }} />
    );
    expect(screen.getByText('Test Title')).toBeTruthy();
    expect(screen.getByText('Test Body')).toBeTruthy();
  });

  it('calls onClose when close button is clicked', () => {
    const onClose = mock();
    render(
      <HelpDialog activeKey="test-key" onClose={onClose} dialogRef={{ current: null }} />
    );
    fireEvent.click(screen.getByLabelText('Close help'));
    expect(onClose).toHaveBeenCalled();
  });
});

describe('useHelpDialog Hook', () => {
  it('initializes with null activeKey', () => {
    const { result } = renderHook(() => useHelpDialog());
    expect(result.current.activeKey).toBeNull();
  });

  it('renderHelpButton returns a button that opens dialog', () => {
    const { result } = renderHook(() => useHelpDialog());
    
    render(<>{result.current.renderHelpButton('test-key')}</>);
    const btn = screen.getByLabelText('Help');
    
    // Assign a mock dialog element to the ref to test showModal
    const mockDialog = document.createElement('dialog');
    mockDialog.showModal = mock();
    (result.current.dialogRef as any).current = mockDialog;

    fireEvent.click(btn);
    
    expect(mockDialog.showModal).toHaveBeenCalled();
    // Since we clicked it, the state updates to 'test-key', but renderHook doesn't rerender automatically in this setup without act.
    // We can just verify the modal was called.
  });
});
