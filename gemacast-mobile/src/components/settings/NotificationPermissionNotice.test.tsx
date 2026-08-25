import { beforeEach, describe, expect, it } from 'bun:test';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { invokeCalls, makeDeviceInfo, setupInvokeMock } from '../../__tests__/setup';
import { useAppStore } from '../../stores/app-store';
import { NotificationPermissionNotice } from './NotificationPermissionNotice';

beforeEach(() => {
  cleanup();
  setupInvokeMock();
  useAppStore.getState().init(makeDeviceInfo());
});

describe('NotificationPermissionNotice', () => {
  it('says nothing when the app may post notifications', () => {
    useAppStore.getState().setNotificationPermission('granted');
    const { container } = render(<NotificationPermissionNotice />);
    expect(container.innerHTML).toBe('');
  });

  // `notRequired` is also what the backend returns when the state could not be
  // read, so a notice here would accuse a working install of being broken.
  it('says nothing when the permission does not apply or could not be read', () => {
    useAppStore.getState().setNotificationPermission('notRequired');
    const { container } = render(<NotificationPermissionNotice />);
    expect(container.innerHTML).toBe('');
  });

  it('explains what a refusal actually costs, not that streaming is broken', () => {
    useAppStore.getState().setNotificationPermission('denied');
    render(<NotificationPermissionNotice />);
    expect(screen.getByText('Notifications are off')).toBeTruthy();
    expect(screen.getByText(/Streaming still works/)).toBeTruthy();
  });

  // The two states differ only in what the user can do next, so that sentence is
  // the whole reason the port reports them separately.
  it('tells a denied user the app will ask again', () => {
    useAppStore.getState().setNotificationPermission('denied');
    render(<NotificationPermissionNotice />);
    expect(screen.getByText(/asks again the next time you open it/)).toBeTruthy();
  });

  it('tells a blocked user that only system settings can undo it', () => {
    useAppStore.getState().setNotificationPermission('blocked');
    render(<NotificationPermissionNotice />);
    expect(screen.getByText(/will not ask again/)).toBeTruthy();
    expect(screen.queryByText(/asks again the next time you open it/)).toBeNull();
  });

  it('opens the system notification settings on request', () => {
    useAppStore.getState().setNotificationPermission('blocked');
    render(<NotificationPermissionNotice />);
    fireEvent.click(screen.getByText('Open settings'));
    expect(invokeCalls.map((call) => call.cmd)).toContain('open_notification_settings');
  });

  // Fixing this happens outside the app, so a notice that only cleared on a
  // restart would sit there telling the user to do what they just did.
  it('clears itself once the user comes back with notifications turned on', async () => {
    setupInvokeMock({ get_notification_permission: 'granted' });
    useAppStore.getState().setNotificationPermission('blocked');
    const { container } = render(<NotificationPermissionNotice />);
    expect(screen.getByText('Notifications are off')).toBeTruthy();

    fireEvent(document, new Event('visibilitychange'));

    await waitFor(() => expect(container.innerHTML).toBe(''));
    expect(useAppStore.getState().notificationPermission).toBe('granted');
  });

  // Nothing to re-read while the state is fine, and this listener would otherwise
  // fire an IPC call on every tab switch for every user.
  it('does not poll the backend while notifications are working', () => {
    useAppStore.getState().setNotificationPermission('granted');
    render(<NotificationPermissionNotice />);
    fireEvent(document, new Event('visibilitychange'));
    expect(invokeCalls.map((call) => call.cmd)).not.toContain('get_notification_permission');
  });
});
