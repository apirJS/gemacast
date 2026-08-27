import { useCallback, useEffect, useRef, useState } from 'react';

//  Must match the slide-out animation in index.css.
const DRAWER_ANIMATION_MS = 350;

/**
 * Open/close state for a full-screen modal `<dialog>` drawer, wired to the
 * hardware back button through a single history entry.
 */
export function useDrawer(hashId: string) {
  const [open, setOpen] = useState(false);
  const [closing, setClosing] = useState(false);
  const dialogRef = useRef<HTMLDialogElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Same shape as ConfirmDialog. Calling showModal/close from the handlers
  // instead is how the element and the state drift apart.
  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    if (open && !dialog.open) dialog.showModal();
    else if (!open && dialog.open) dialog.close();
  }, [open]);

  const finishClose = useCallback(() => {
    if (timer.current) {
      clearTimeout(timer.current);
      timer.current = null;
    }
    setClosing(false);
    setOpen(false);
  }, []);

  /** Play the slide-out, then drop the dialog. */
  const startClose = useCallback(() => {
    if (timer.current) return;
    setClosing(true);
    timer.current = setTimeout(finishClose, DRAWER_ANIMATION_MS);
  }, [finishClose]);

  const handleOpen = useCallback(() => {
    if (timer.current) {
      clearTimeout(timer.current);
      timer.current = null;
    }
    setClosing(false);
    setOpen(true);
    // One entry per open. A duplicate entry for the same hash takes two back
    // presses to leave, which reads as a dead close button.
    if (window.location.hash !== `#${hashId}`) {
      window.history.pushState({ drawer: hashId }, '', `#${hashId}`);
    }
  }, [hashId]);

  /** Route the close through history so the X and the back button agree. */
  const handleClose = useCallback(() => {
    if (window.location.hash === `#${hashId}`) window.history.back();
    else startClose();
  }, [hashId, startClose]);

  /**
   * The dialog's own `close` event. Fires for Esc, for the browser's close
   * watcher claiming the Android back gesture, and for our own `close()`. The
   * element is already shut in all three, so the state has to follow - the sync
   * effect above would otherwise reopen it.
   */
  const handleNativeClose = useCallback(() => {
    finishClose();
    if (window.location.hash === `#${hashId}`) window.history.back();
  }, [finishClose, hashId]);

  useEffect(() => {
    const handlePopState = () => {
      // Keyed to the element rather than to `open`: a native close already shut
      // the dialog and popped its own entry, which must not restart the
      // animation.
      if (window.location.hash !== `#${hashId}` && dialogRef.current?.open) startClose();
    };
    window.addEventListener('popstate', handlePopState);
    return () => window.removeEventListener('popstate', handlePopState);
  }, [hashId, startClose]);

  useEffect(() => {
    const handleVisibility = () => {
      if (document.visibilityState === 'visible' && timer.current) finishClose();
    };
    document.addEventListener('visibilitychange', handleVisibility);
    return () => document.removeEventListener('visibilitychange', handleVisibility);
  }, [finishClose]);

  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    [],
  );

  return { open, closing, dialogRef, handleOpen, handleClose, handleNativeClose };
}
