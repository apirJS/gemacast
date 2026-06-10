import { useRef, useCallback, useState } from 'react';
import { X } from 'lucide-react';
import { HELP_CONTENT } from '../../core/help-content';

export function HelpDialog({
  activeKey,
  onClose,
  dialogRef,
}: {
  activeKey: string | null;
  onClose: () => void;
  dialogRef: React.RefObject<HTMLDialogElement | null>;
}) {
  const content = activeKey ? HELP_CONTENT[activeKey] : null;

  return (
    <dialog
      ref={dialogRef}
      className="fixed inset-0 z-50 m-auto max-h-[70vh] w-[min(90vw,360px)] overflow-y-auto rounded-[var(--radius-lg)] border border-border bg-popover p-5 text-popover-foreground shadow-xl backdrop:bg-black/50"
      onClose={onClose}
    >
      {content && (
        <>
          <div className="mb-2 flex items-center justify-between">
            <h2 className="text-base font-semibold">{content.title}</h2>
            <button
              type="button"
              className="text-muted-foreground hover:text-foreground transition-colors"
              onClick={onClose}
              aria-label="Close help"
            >
              <X className="h-4 w-4" />
            </button>
          </div>
          <p className="text-sm text-muted-foreground whitespace-pre-line">{content.body}</p>
        </>
      )}
    </dialog>
  );
}

// eslint-disable-next-line react-refresh/only-export-components
export function useHelpDialog() {
  const [activeHelp, setActiveHelp] = useState<string | null>(null);
  const helpDialogRef = useRef<HTMLDialogElement>(null);

  const openHelp = useCallback((key: string) => {
    setActiveHelp(key);
    helpDialogRef.current?.showModal();
  }, []);

  const closeHelp = useCallback(() => {
    helpDialogRef.current?.close();
    setActiveHelp(null);
  }, []);

  const renderHelpButton = useCallback(
    (helpKey: string) => (
      <button
        type="button"
        className="ml-1 inline-flex h-5 w-5 items-center justify-center rounded-full border border-border bg-background text-[0.7rem] font-bold text-muted-foreground hover:border-primary hover:bg-primary hover:text-white transition-colors"
        onClick={(e) => {
          e.stopPropagation();
          openHelp(helpKey);
        }}
        aria-label="Help"
      >
        ?
      </button>
    ),
    [openHelp],
  );

  return {
    activeKey: activeHelp,
    dialogRef: helpDialogRef,
    closeHelp,
    renderHelpButton,
  };
}
