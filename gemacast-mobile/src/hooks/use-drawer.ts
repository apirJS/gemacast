import { useCallback, useEffect, useState } from 'react';

export function useDrawer(hashId: string) {
  const [open, setOpen] = useState(false);

  const handleOpen = useCallback(() => {
    setOpen(true);
    if (window.location.hash !== `#${hashId}`) {
      window.history.pushState({ drawer: hashId }, '', `#${hashId}`);
    }
  }, [hashId]);

  const handleClose = useCallback(() => {
    if (window.location.hash === `#${hashId}`) window.history.back();
    else setOpen(false);
  }, [hashId]);

  useEffect(() => {
    const syncToHash = () => setOpen(window.location.hash === `#${hashId}`);
    window.addEventListener('popstate', syncToHash);
    return () => window.removeEventListener('popstate', syncToHash);
  }, [hashId]);

  return { open, handleOpen, handleClose };
}
