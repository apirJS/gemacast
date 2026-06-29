import { useRef } from 'react';
import { CircleCheck, XCircle, TriangleAlert, Info, X } from 'lucide-react';
import type { Toast as ToastType } from '../../stores/toast-store';
import { useToastStore } from '../../stores/toast-store';

const ICON_MAP: Record<ToastType['type'], React.ReactNode> = {
  success: <CircleCheck className="h-5 w-5 text-status-ok" />,
  error: <XCircle className="h-5 w-5 text-status-lost" />,
  warning: <TriangleAlert className="h-5 w-5 text-status-warn" />,
  info: <Info className="h-5 w-5 text-primary" />,
};

export function Toast({ toast }: { toast: ToastType }) {
  const dismiss = useToastStore((s) => s.dismiss);
  const dialogRef = useRef<HTMLDialogElement>(null);

  const isError = toast.type === 'error';

  return (
    <div
      className={`
        pointer-events-auto flex items-center gap-3
        rounded-[var(--radius-default)] border
        px-4 py-3 shadow-[0_4px_6px_-1px_rgb(0_0_0/0.1),0_2px_4px_-2px_rgb(0_0_0/0.1)]
        min-w-[250px] max-w-[350px]
        ${
          toast.closing
            ? 'animate-[toast-slide-out_200ms_ease-in_forwards]'
            : 'animate-[toast-slide-in_300ms_cubic-bezier(0.16,1,0.3,1)_forwards]'
        }
        ${
          isError
            ? 'border-status-lost-border bg-status-lost-bg'
            : 'border-border bg-card text-card-foreground'
        }
      `}
      role="alert"
    >
      <span className="flex shrink-0 items-center justify-center">{ICON_MAP[toast.type]}</span>

      <div className="flex flex-1 flex-col gap-1">
        <span className="text-[0.875rem] font-medium leading-5">{toast.message}</span>
        {isError && toast.fullLog && (
          <button
            type="button"
            className="text-left text-xs font-semibold text-status-lost underline underline-offset-2 hover:opacity-80"
            onClick={() => dialogRef.current?.showModal()}
          >
            Click for details
          </button>
        )}
      </div>

      <button
        type="button"
        className="shrink-0 -mr-2 flex items-center justify-center rounded-full p-1 text-muted-foreground hover:bg-muted hover:text-foreground"
        onClick={() => dismiss(toast.id)}
        aria-label="Close toast"
      >
        <X className="h-4 w-4" />
      </button>

      {isError && toast.fullLog && (
        <dialog
          ref={dialogRef}
          className="fixed inset-0 z-[10001] m-auto w-[min(90vw,600px)] rounded-[var(--radius-lg)] border border-border bg-popover p-5 text-popover-foreground shadow-xl backdrop:bg-black/50"
          onClick={(e) => {
            if (e.target === dialogRef.current) dialogRef.current.close();
          }}
        >
          <h3 className="mb-3 text-base font-semibold">Error Details</h3>
          <div className="mb-4 min-h-[100px] max-h-[50vh] overflow-x-auto overflow-y-auto rounded-[var(--radius-default)] bg-secondary p-3 text-xs whitespace-pre font-mono">
            {toast.fullLog}
          </div>
          <button
            type="button"
            className="btn btn-secondary w-full"
            onClick={() => dialogRef.current?.close()}
          >
            Close
          </button>
        </dialog>
      )}
    </div>
  );
}
