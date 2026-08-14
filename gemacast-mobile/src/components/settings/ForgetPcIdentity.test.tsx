import { beforeEach, describe, expect, it } from 'bun:test';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { invokeCalls, makeDeviceInfo, makeDiscoveredSender, setupInvokeMock } from '../../__tests__/setup';
import { useAppStore } from '../../stores/app-store';
import { ForgetPcIdentity } from './ForgetPcIdentity';

beforeEach(() => {
  cleanup();
  setupInvokeMock();
  useAppStore.getState().init(makeDeviceInfo());
});

describe('ForgetPcIdentity', () => {
  it('forgets only the selected discovered PC', async () => {
    useAppStore.getState().setDiscoveredSenders([
      makeDiscoveredSender({ deviceId: 'pc-1', deviceName: 'Desktop PC' }),
      makeDiscoveredSender({ deviceId: 'pc-2', deviceName: 'Laptop' }),
    ]);
    render(<ForgetPcIdentity />);

    fireEvent.click(screen.getByLabelText('Forget Laptop'));
    fireEvent.click(screen.getByRole('button', { name: 'Forget', hidden: true }));

    await waitFor(() => expect(invokeCalls).toHaveLength(1));
    expect(invokeCalls[0]).toEqual({ cmd: 'forget_pc_identity', args: { pcId: 'pc-2' } });
  });
});
