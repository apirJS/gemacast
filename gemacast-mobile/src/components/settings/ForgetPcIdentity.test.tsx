import { beforeEach, describe, expect, it } from 'bun:test';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { invokeCalls, makeDeviceInfo, makeDiscoveredSender, setupInvokeMock } from '../../__tests__/setup';
import { Status } from '../../core/types';
import { useAppStore } from '../../stores/app-store';
import { ForgetPcIdentity } from './ForgetPcIdentity';

beforeEach(() => {
  cleanup();
  setupInvokeMock();
  useAppStore.getState().init(makeDeviceInfo());
});

describe('ForgetPcIdentity', () => {
  it('shows only PCs present in the native trust store', async () => {
    setupInvokeMock({ get_paired_pc_ids: ['pc-2', 'missing-name'] });
    useAppStore.getState().setDiscoveredSenders([
      makeDiscoveredSender({ deviceId: 'pc-1', deviceName: 'Desktop PC' }),
      makeDiscoveredSender({ deviceId: 'pc-2', deviceName: 'Laptop' }),
    ]);
    render(<ForgetPcIdentity />);

    expect(await screen.findByLabelText('Forget Laptop')).toBeTruthy();
    expect(screen.getByLabelText('Forget missing-name')).toBeTruthy();
    expect(screen.queryByLabelText('Forget Desktop PC')).toBeNull();
  });

  it('wraps long PC names inside a vertically bounded list', async () => {
    const longName = `Living room ${'streaming-pc-'.repeat(20)}`;
    setupInvokeMock({ get_paired_pc_ids: ['pc-long'] });
    useAppStore
      .getState()
      .setDiscoveredSenders([makeDiscoveredSender({ deviceId: 'pc-long', deviceName: longName })]);
    render(<ForgetPcIdentity />);

    const name = await screen.findByText(longName);
    expect(name.className).toContain('[overflow-wrap:anywhere]');
    expect(name.className).not.toContain('truncate');

    const list = screen.getByRole('list', { name: 'Paired PCs' });
    expect(list.className).toContain('max-h-40');
    expect(list.className).toContain('overflow-y-auto');
  });

  it('refreshes the native trust store after a PC connects', async () => {
    let pairedPcIds: string[] = [];
    setupInvokeMock({ get_paired_pc_ids: () => pairedPcIds });
    const sender = makeDiscoveredSender({ deviceId: 'pc-2', deviceName: 'Laptop' });
    useAppStore.getState().setDiscoveredSenders([sender]);
    render(<ForgetPcIdentity />);

    expect(await screen.findByText('No paired PCs')).toBeTruthy();
    pairedPcIds = ['pc-2'];
    useAppStore.getState().setConnectedSender(sender);

    expect(await screen.findByLabelText('Forget Laptop')).toBeTruthy();
  });

  it('removes only the selected PC after native removal succeeds', async () => {
    setupInvokeMock({ get_paired_pc_ids: ['pc-1', 'pc-2'] });
    useAppStore.getState().setDiscoveredSenders([
      makeDiscoveredSender({ deviceId: 'pc-1', deviceName: 'Desktop PC' }),
      makeDiscoveredSender({ deviceId: 'pc-2', deviceName: 'Laptop' }),
    ]);
    render(<ForgetPcIdentity />);

    await screen.findByLabelText('Forget Laptop');
    fireEvent.click(screen.getByLabelText('Forget Laptop'));
    fireEvent.click(screen.getByRole('button', { name: 'Forget', hidden: true }));

    await waitFor(() =>
      expect(invokeCalls).toContainEqual({ cmd: 'forget_pc_identity', args: { pcId: 'pc-2' } }),
    );
    await waitFor(() => expect(screen.queryByLabelText('Forget Laptop')).toBeNull());
    expect(screen.getByLabelText('Forget Desktop PC')).toBeTruthy();
    expect(invokeCalls).toContainEqual({ cmd: 'forget_pc_identity', args: { pcId: 'pc-2' } });
    expect(invokeCalls.some((call) => call.cmd === 'disconnect_from_sender')).toBe(false);
  });

  it('disconnects the stream before forgetting the connected PC', async () => {
    setupInvokeMock({ get_paired_pc_ids: ['pc-2'] });
    const sender = makeDiscoveredSender({ deviceId: 'pc-2', deviceName: 'Laptop' });
    useAppStore.getState().patch({
      connectedSender: sender,
      lastConnectedSender: sender,
      discoveredSenders: [sender],
      status: Status.Playing,
    });
    render(<ForgetPcIdentity />);

    await screen.findByLabelText('Forget Laptop');
    fireEvent.click(screen.getByLabelText('Forget Laptop'));
    expect(screen.getByText('Disconnect from and forget Laptop?')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Forget', hidden: true }));

    await waitFor(() =>
      expect(invokeCalls).toContainEqual({ cmd: 'forget_pc_identity', args: { pcId: 'pc-2' } }),
    );
    const disconnectIndex = invokeCalls.findIndex((call) => call.cmd === 'disconnect_from_sender');
    const forgetIndex = invokeCalls.findIndex((call) => call.cmd === 'forget_pc_identity');
    expect(disconnectIndex).toBeGreaterThan(-1);
    expect(disconnectIndex).toBeLessThan(forgetIndex);
    expect(useAppStore.getState().connectedSender).toBeNull();
    expect(useAppStore.getState().lastConnectedSender).toBeNull();
    expect(useAppStore.getState().status).toBe(Status.Listening);
    await waitFor(() => expect(screen.queryByLabelText('Forget Laptop')).toBeNull());
  });

  it('keeps the PC when native removal fails', async () => {
    setupInvokeMock({
      get_paired_pc_ids: ['pc-2'],
      forget_pc_identity: () => {
        throw new Error('remove failed');
      },
    });
    useAppStore
      .getState()
      .setDiscoveredSenders([makeDiscoveredSender({ deviceId: 'pc-2', deviceName: 'Laptop' })]);
    render(<ForgetPcIdentity />);

    await screen.findByLabelText('Forget Laptop');
    fireEvent.click(screen.getByLabelText('Forget Laptop'));
    fireEvent.click(screen.getByRole('button', { name: 'Forget', hidden: true }));

    await waitFor(() =>
      expect(invokeCalls).toContainEqual({
        cmd: 'forget_pc_identity',
        args: { pcId: 'pc-2' },
      }),
    );
    expect(screen.getByLabelText('Forget Laptop')).toBeTruthy();
  });
});
