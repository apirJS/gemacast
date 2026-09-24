import { useEffect, useState } from 'react';
import { AppShell } from './components/layout/AppShell';
import { applicationController } from './controllers';

function LoadingScreen() {
  return (
    <div className="flex min-h-dvh items-center justify-center">
      <div className="h-8 w-8 animate-spin rounded-full border-2 border-primary border-t-transparent" />
    </div>
  );
}

export function App() {
  const [ready, setReady] = useState(false);

  useEffect(() => {
    let stop: () => void = () => undefined;
    let active = true;
    void applicationController.initialize().then(() => {
      if (!active) return;
      stop = applicationController.start();
      setReady(true);
    });
    return () => {
      active = false;
      stop();
    };
  }, []);

  return ready ? <AppShell /> : <LoadingScreen />;
}
