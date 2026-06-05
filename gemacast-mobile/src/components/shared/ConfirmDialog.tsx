import { useEffect, useRef } from 'react';

type ConfirmDialogProps = {
  open: boolean;
  message: string;
  confirmLabel?: string;
  cancelLabel?: string;
  onConfirm: () => void;
  onCancel: () => void;
};

export function ConfirmDialog({
  open,
  message,
  confirmLabel = 'Delete',
  cancelLabel = 'Cancel',
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    if (open && dialogRef.current && !dialogRef.current.open) {
      dialogRef.current.showModal();
    } else if (!open && dialogRef.current?.open) {
      dialogRef.current.close();
    }
  }, [open]);
  return (
    <dialog
      ref={dialogRef}
      className={`
        fixed inset-0 z-50 m-auto w-[min(90vw,320px)] rounded-[var(--radius-lg)]
        border border-border bg-popover p-5 text-popover-foreground shadow-xl
        backdrop:bg-black/50
      `}
      onClose={(e) => {
        e.stopPropagation();
        onCancel();
      }}
    >
      <p className="mb-4 text-sm">{message}</p>
      <div className="flex gap-2">
        <button type="button" className="btn btn-secondary flex-1" onClick={onCancel}>
          {cancelLabel}
        </button>
        <button type="button" className="btn btn-destructive flex-1" onClick={onConfirm}>
          {confirmLabel}
        </button>
      </div>
    </dialog>
  );
}
