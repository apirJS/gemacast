import { useState, useRef, useEffect } from 'react';

const DRAWER_ANIMATION_MS = 350;

export function useDrawer(hashId: string) {
  const [open, setOpen] = useState(false);
  const [closing, setClosing] = useState(false);
  const dialogRef = useRef<HTMLDialogElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout>>(null);

  // Stabilized by the React Compiler; exhaustive-deps (line 51) isn't
  // compiler-aware and flags this as a changing dep. Re-subscribing the
  // popstate listener would be harmless regardless (cleanup runs each time).
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const startClose = () => {
    if (timer.current) return;
    setClosing(true);
    timer.current = setTimeout(() => {
      timer.current = null;
      setOpen(false);
      setClosing(false);
      dialogRef.current?.close();
    }, DRAWER_ANIMATION_MS);
  };

  const handleOpen = () => {
    if (timer.current) {
      clearTimeout(timer.current);
      timer.current = null;
      setClosing(false);
    }
    if (!dialogRef.current?.open) {
      dialogRef.current?.showModal();
    }
    setOpen(true);
    window.history.pushState({ drawer: hashId }, '', `#${hashId}`);
  };

  const handleClose = () => {
    if (window.location.hash === `#${hashId}`) {
      window.history.back();
    } else {
      startClose();
    }
  };

  useEffect(() => {
    const handlePopState = () => {
      if (open && window.location.hash !== `#${hashId}`) {
        startClose();
      }
    };
    window.addEventListener('popstate', handlePopState);
    return () => window.removeEventListener('popstate', handlePopState);
  }, [open, hashId, startClose]);

  useEffect(() => {
    return () => {
      if (timer.current) clearTimeout(timer.current);
    };
  }, []);

  return { open, closing, dialogRef, handleOpen, handleClose };
}
