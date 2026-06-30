import { DeviceInfo } from '../device/DeviceInfo';
import { StatusChip } from '../device/StatusChip';
import { SenderList } from '../senders/SenderList';
import { ManualConnect } from '../senders/ManualConnect';
import { LatencyStats } from '../latency/LatencyStats';
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
        className="mx-auto flex h-[100dvh] max-w-lg flex-col gap-6 px-6 overflow-hidden"
        style={{
          paddingTop: 'calc(4rem + env(safe-area-inset-top, 0px))',
          paddingBottom: 'calc(2rem + env(safe-area-inset-bottom, 0px))',
        }}
      >
        <DeviceInfo />
        <ManualConnect />
        <SenderList />

        <section className="mt-auto flex flex-col items-center gap-2 pt-4">
          <StatusChip />
          <LatencyStats />
        </section>
      </main>
    </>
  );
}
