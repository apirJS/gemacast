import { App } from './App';
import { setupLatencyStats } from './dom/latency';
import { setupDeviceAndStatus } from './dom/device';
import { setupSenderList } from './dom/senders';
import { setupErrorSection } from './dom/error';
import { setupManualConnect } from './dom/manual-connect';
import { listenForTauriEvents } from './dom/listeners';
import { initSettingsDrawer } from './dom/settings';
import { setupNavigationHandler } from './dom/navigation';

window.addEventListener('DOMContentLoaded', async () => {
  const app = await App.create();

  setupDeviceAndStatus(app);
  setupLatencyStats(app);
  setupSenderList(app);
  setupManualConnect(app);
  setupErrorSection(app);
  listenForTauriEvents(app);
  initSettingsDrawer(app);
  setupNavigationHandler();

  app.discovery.startListening(app.stateHandler.getState().settings.mode);
});
