import { useToastStore } from '../../stores/toast-store';
import { Toast } from './Toast';

export function ToastContainer() {
  const toasts = useToastStore((s) => s.toasts);

  if (toasts.length === 0) return null;

  return (
    <div
      className="fixed right-4 z-[10000] flex flex-col items-end gap-2 pointer-events-none"
      style={{ top: 'calc(1rem + env(safe-area-inset-top, 0px))' }}
    >
      {toasts.map((toast) => (
        <Toast key={toast.id} toast={toast} />
      ))}
    </div>
  );
}
