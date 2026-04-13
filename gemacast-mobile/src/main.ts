import { App } from './App';
import { setupLatencyStats } from './dom/latency';
import { setupDeviceAndStatus } from './dom/device';
import { setupVolumeControls } from './dom/volume';
import { setupSenderList } from './dom/senders';
import { setupErrorSection } from './dom/error';
import { listenForTauriEvents } from './dom/listeners';
import { initSettingsDrawer } from './dom/settings';

window.addEventListener('DOMContentLoaded', async () => {
  const app = await App.create();

  setupDeviceAndStatus(app);
  setupLatencyStats(app);
  setupVolumeControls(app);
  setupSenderList(app);
  setupErrorSection(app);
  listenForTauriEvents(app);
  initSettingsDrawer(app);

  app.discovery.startListening(app.stateHandler.getState().settings.mode);

  history.pushState({ main: true }, '');
  let lastBackPress = 0;

  window.addEventListener('popstate', () => {
    const drawer = document.getElementById(
      'settings-drawer',
    ) as HTMLDialogElement;
    const helpModal = document.getElementById(
      'help-modal',
    ) as HTMLDialogElement;

    if (helpModal?.open) {
      helpModal.close();
      history.pushState({ main: true }, '');
      return;
    }

    if (drawer?.open) {
      drawer.close();
      history.pushState({ main: true }, '');
      return;
    }

    const now = Date.now();
    if (now - lastBackPress < 2000) {
      return;
    }
    lastBackPress = now;
    history.pushState({ main: true }, '');

    let toast = document.getElementById('back-toast');
    if (!toast) {
      toast = document.createElement('div');
      toast.id = 'back-toast';
      toast.style.cssText =
        'position:fixed;bottom:80px;left:50%;transform:translateX(-50%);background:rgba(255,255,255,0.15);color:var(--foreground);padding:8px 20px;border-radius:20px;font-size:0.85rem;z-index:9999;backdrop-filter:blur(8px);transition:opacity 0.3s;';
      document.body.appendChild(toast);
    }
    toast.textContent = 'Press back again to exit';
    toast.style.opacity = '1';
    setTimeout(() => {
      toast!.style.opacity = '0';
    }, 1500);
  });
});
