import { useState, useRef, useEffect, useCallback } from 'react';

const DRAWER_ANIMATION_MS = 350;

export function useDrawer(hashId: string) {
  const [open, setOpen] = useState(false);
  const [closing, setClosing] = useState(false);
  const dialogRef = useRef<HTMLDialogElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout>>(null);

  const startClose = useCallback(() => {
    if (timer.current) return;
    setClosing(true);
    timer.current = setTimeout(() => {
      timer.current = null;
      setOpen(false);
      setClosing(false);
      dialogRef.current?.close();
    }, DRAWER_ANIMATION_MS);
  }, []);

  const handleOpen = useCallback(() => {
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
  }, [hashId]);

  const handleClose = useCallback(() => {
    if (window.location.hash === `#${hashId}`) {
      window.history.back();
    } else {
      startClose();
    }
  }, [hashId, startClose]);

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
    return () => { if (timer.current) clearTimeout(timer.current); };
  }, []);

  return { open, closing, dialogRef, handleOpen, handleClose };
}
