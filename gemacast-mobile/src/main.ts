import { App } from './App';
import { setupLatencyStats } from './dom/latency';
import { setupDeviceAndStatus } from './dom/device';
import { setupVolumeControls } from './dom/volume';
import { setupSenderList } from './dom/senders';
import { setupErrorSection } from './dom/error';
import { listenForTauriEvents } from './dom/listeners';

window.addEventListener('DOMContentLoaded', async () => {
  const app = await App.create();

  setupDeviceAndStatus(app);
  setupLatencyStats(app);
  setupVolumeControls(app);
  setupSenderList(app);
  setupErrorSection(app);
  listenForTauriEvents(app);

  app.discovery.startListening();
});
