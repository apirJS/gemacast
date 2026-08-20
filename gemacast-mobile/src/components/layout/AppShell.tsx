import { DeviceInfo } from '../device/DeviceInfo';
import { SenderList } from '../senders/SenderList';
import { ManualConnect } from '../senders/ManualConnect';
import { ConnectionReadout } from '../latency/ConnectionReadout';
import { ToastContainer } from '../feedback/ToastContainer';
import { SettingsDrawer } from '../settings/SettingsDrawer';
import { useWakeLock } from '../../hooks/use-wake-lock';
import { useAppStore } from '../../stores/app-store';

export function AppShell() {
  const keepScreenOn = useAppStore((s) => s.settings.keepScreenOn);
  useWakeLock(keepScreenOn);

  return (
    <>
      <ToastContainer />
      <SettingsDrawer />

      <main
        className="mx-auto flex h-dvh max-w-lg flex-col gap-6 px-6 overflow-hidden"
        style={{
          paddingTop: 'calc(4rem + env(safe-area-inset-top, 0px))',
          paddingBottom: 'calc(2rem + env(safe-area-inset-bottom, 0px))',
        }}
      >
        <DeviceInfo />

        {/* Hidden unless there is a live session to report on. */}
        <ConnectionReadout />

        <ManualConnect />
        <SenderList />
      </main>
    </>
  );
}
