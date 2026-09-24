import { hasLiveSession } from '../../core/types';
import { useManualConnectionController } from '../../controllers';
import { useAppStore } from '../../stores/app-store';
import { ManualConnectView } from './ManualConnectView';

export function ManualConnect() {
  const controller = useManualConnectionController();
  const status = useAppStore((state) => state.status);

  return (
    <ManualConnectView
      visible={!hasLiveSession(status)}
      ip={controller.ip}
      loading={controller.isLoading}
      disabled={controller.isDisabled}
      onIpChange={controller.setIp}
      onConnect={() => void controller.handleConnect()}
    />
  );
}
