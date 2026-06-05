import { describe, it, expect, beforeEach } from 'bun:test';
import { useToastStore } from './toast-store';

beforeEach(() => {
  useToastStore.setState({ toasts: [] });
});

describe('toast-store', () => {
  it('show adds a toast', () => {
    useToastStore.getState().show('info', 'Hello');
    expect(useToastStore.getState().toasts).toHaveLength(1);
    expect(useToastStore.getState().toasts[0].message).toBe('Hello');
    expect(useToastStore.getState().toasts[0].type).toBe('info');
  });

  it('show generates unique IDs', () => {
    useToastStore.getState().show('info', 'First');
    useToastStore.getState().show('success', 'Second');
    const [a, b] = useToastStore.getState().toasts;
    expect(a.id).not.toBe(b.id);
  });

  it('dismiss removes a specific toast by ID asynchronously', async () => {
    useToastStore.getState().show('info', 'Keep');
    useToastStore.getState().show('warning', 'Remove');
    const removeId = useToastStore.getState().toasts[1].id;
    
    useToastStore.getState().dismiss(removeId);
    
    // It should be marked as closing immediately
    expect(useToastStore.getState().toasts[1].closing).toBe(true);
    
    // Wait for the 200ms removal timeout
    await new Promise(resolve => setTimeout(resolve, 250));
    
    const remaining = useToastStore.getState().toasts;
    expect(remaining).toHaveLength(1);
    expect(remaining[0].message).toBe('Keep');
  });

  it('dismiss with non-existent ID is a no-op', () => {
    useToastStore.getState().show('info', 'Stay');
    useToastStore.getState().dismiss('no-such-id');
    expect(useToastStore.getState().toasts).toHaveLength(1);
  });

  it('multiple toasts can coexist', () => {
    useToastStore.getState().show('info', 'A');
    useToastStore.getState().show('success', 'B');
    useToastStore.getState().show('warning', 'C');
    expect(useToastStore.getState().toasts).toHaveLength(3);
  });

  it('error toasts deduplicate and remove prior error toasts', () => {
    useToastStore.getState().show('error', 'Error 1');
    useToastStore.getState().show('info', 'Some info');
    useToastStore.getState().show('error', 'Error 2');
    
    const toasts = useToastStore.getState().toasts;
    expect(toasts).toHaveLength(2);
    expect(toasts[0].type).toBe('info');
    expect(toasts[1].type).toBe('error');
    expect(toasts[1].message).toBe('Error 2');
  });
});
