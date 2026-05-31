import { create } from 'zustand';

export type ToastType = 'info' | 'success' | 'warning' | 'error';

export type Toast = {
  id: string;
  type: ToastType;
  message: string;
  fullLog?: string;
  closing?: boolean;
};

type ToastActions = {
  show: (type: ToastType, message: string, fullLog?: string) => void;
  dismiss: (id: string) => void;
  clearError: () => void;
};

export type ToastStore = {
  toasts: Toast[];
} & ToastActions;

let nextId = 0;

export const useToastStore = create<ToastStore>((set, get) => ({
  toasts: [],

  show: (type, message, fullLog) => {
    // Deduplicate error toasts — remove existing error before adding new one
    if (type === 'error') {
      const existing = get().toasts.filter((t) => t.type !== 'error');
      const id = `toast-${++nextId}`;
      set({ toasts: [...existing, { id, type, message, fullLog }] });
      // Error toasts do NOT auto-dismiss
      return;
    }

    const id = `toast-${++nextId}`;
    set((state) => ({
      toasts: [...state.toasts, { id, type, message }],
    }));

    setTimeout(() => {
      get().dismiss(id);
    }, 3000);
  },

  dismiss: (id) => {
    // Start closing animation
    set((state) => ({
      toasts: state.toasts.map((t) =>
        t.id === id ? { ...t, closing: true } : t,
      ),
    }));
    // Remove after animation completes
    setTimeout(() => {
      set((state) => ({
        toasts: state.toasts.filter((t) => t.id !== id),
      }));
    }, 200);
  },

  clearError: () => {
    const errorToast = get().toasts.find((t) => t.type === 'error');
    if (errorToast) {
      get().dismiss(errorToast.id);
    }
  },
}));
