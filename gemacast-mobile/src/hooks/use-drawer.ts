import { useState, useRef, useEffect, useCallback } from 'react';

/**
 * Hook to manage a modal drawer that integrates with browser history
 * (back button closes the drawer on mobile).
 */
export function useDrawer(hashId: string) {
  const [open, setOpen] = useState(false);
  const dialogRef = useRef<HTMLDialogElement>(null);

  const handleOpen = useCallback(() => {
    setOpen(true);
    dialogRef.current?.showModal();
    window.history.pushState({ drawer: hashId }, '', `#${hashId}`);
  }, [hashId]);

  const handleClose = useCallback(() => {
    if (window.location.hash === `#${hashId}`) {
      window.history.back();
    } else {
      setOpen(false);
      dialogRef.current?.close();
    }
  }, [hashId]);

  useEffect(() => {
    const handlePopState = () => {
      if (open && window.location.hash !== `#${hashId}`) {
        setOpen(false);
        dialogRef.current?.close();
      }
    };
    window.addEventListener('popstate', handlePopState);
    return () => window.removeEventListener('popstate', handlePopState);
  }, [open, hashId]);

  return { open, dialogRef, handleOpen, handleClose };
}
