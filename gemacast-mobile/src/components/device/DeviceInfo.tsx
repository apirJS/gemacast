import { useAppStore } from '../../stores/app-store';

export function DeviceInfo() {
  const deviceName = useAppStore((s) => s.deviceInfo.deviceName);
  const ip = useAppStore((s) => s.deviceInfo.ip);

  return (
    <section className="flex flex-col items-center gap-1 px-1 text-center">
      <span className="text-2xl font-medium text-foreground truncate max-w-full">
        {deviceName}
      </span>
      <span className="text-sm font-medium text-muted-foreground shrink-0">
        IP: {ip}
      </span>
    </section>
  );
}
