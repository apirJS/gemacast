import { App } from '../../App';
import { initDrawer } from './drawer';
import { initHelpModal } from './help';
import { initThemeToggle } from './theme';
import { initModes } from './modes';
import { initBufferSettings } from './buffer';
import { initBitrateSettings } from './bitrate';

export function initSettingsDrawer(app: App) {
  initDrawer();
  initHelpModal();
  initThemeToggle(app);
  initModes(app);
  initBufferSettings(app);
  initBitrateSettings(app);
}
