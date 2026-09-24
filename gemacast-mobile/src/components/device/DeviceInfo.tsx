import { useAppStore } from '../../stores/app-store';
import { DeviceInfoView } from './DeviceInfoView';

export function DeviceInfo() {
  const deviceName = useAppStore((s) => s.deviceInfo.deviceName);
  const ip = useAppStore((s) => s.deviceInfo.ip);

  return <DeviceInfoView deviceName={deviceName} ip={ip} />;
}
