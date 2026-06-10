import { X } from 'lucide-react';
import { ThemeToggle } from './ThemeToggle';
import { BufferPresetSelect } from './BufferPresetSelect';
import { CustomJitterConfig } from './CustomJitterConfig';
import { BitrateSelect } from './BitrateSelect';
import { GainSlider } from './GainSlider';
import { ExclusiveToggle } from './ExclusiveToggle';
import { ModeSelector } from './ModeSelector';
import { HelpDialog, useHelpDialog } from '../shared/HelpDialog';
import { useDrawer } from '../../hooks/use-drawer';

export function SettingsDrawer() {
  const { open, dialogRef, handleOpen, handleClose } = useDrawer('settings');
  const help = useHelpDialog();

  return (
    <>
      <button
        type="button"
        className="fixed left-5 z-40 flex items-center justify-center rounded-full p-2 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
        style={{ top: 'max(1.5rem, env(safe-area-inset-top, 0px))' }}
        onClick={handleOpen}
        aria-label="Open settings"
      >
        <svg
          width="20"
          height="20"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <path d="M4 6H20M4 12H20M4 18H20" />
        </svg>
      </button>

      <dialog
        ref={dialogRef}
        className={`
          fixed inset-y-0 left-0 z-50 m-0 h-[100vh] max-h-none w-[100vw] max-w-[100vw]
          border-none border-r border-border bg-background p-0 text-foreground
          shadow-[4px_0_24px_rgba(0,0,0,0.2)]
          backdrop:bg-black/30 backdrop:backdrop-blur-[4px]
          ${open ? 'animate-[slide-from-left_350ms_cubic-bezier(0.32,0.72,0,1)]' : ''}
        `}
        onClose={handleClose}
      >
        <div className="flex h-full flex-col">
          <div
            className="flex items-center justify-between border-b border-border px-5 py-3"
            style={{ paddingTop: 'max(1.5rem, env(safe-area-inset-top, 0px))' }}
          >
            <button
              type="button"
              className="text-muted-foreground transition-colors hover:text-foreground"
              onClick={handleClose}
              aria-label="Close Settings"
            >
              <X className="h-5 w-5" />
            </button>
            <ThemeToggle />
          </div>

          <div className="flex-1 space-y-6 overflow-y-auto px-5 py-5">
            <div>
              <div className="mb-2 flex items-center gap-2 text-[0.9rem] font-semibold uppercase tracking-[0.05em] text-muted-foreground">
                Buffer Preset
                {help.renderHelpButton('buffer-preset')}
              </div>
              <BufferPresetSelect />
              <CustomJitterConfig renderHelpButton={help.renderHelpButton} />
            </div>

            <div>
              <div className="mb-2 flex items-center gap-2 text-[0.9rem] font-semibold uppercase tracking-[0.05em] text-muted-foreground">
                Audio Bitrate Quality
                {help.renderHelpButton('audio-bitrate')}
              </div>
              <BitrateSelect />
            </div>

            <div>
              <div className="mb-2 flex items-center gap-2 text-[0.9rem] font-semibold uppercase tracking-[0.05em] text-muted-foreground">
                Audio Gain
                {help.renderHelpButton('audio-gain')}
              </div>
              <GainSlider />
            </div>

            <div className="flex items-center justify-between mb-2">
              <div className="flex items-center gap-2 text-[0.9rem] font-semibold uppercase tracking-[0.05em] text-muted-foreground">
                Exclusive Mode
                {help.renderHelpButton('exclusive-mode')}
              </div>
              <ExclusiveToggle />
            </div>

            <div>
              <div className="mb-2 flex items-center gap-2 text-[0.9rem] font-semibold uppercase tracking-[0.05em] text-muted-foreground">
                Mode
                {help.renderHelpButton('connection-mode')}
              </div>
              <ModeSelector />
            </div>

            <div className="mt-4 border-t border-border pt-6 text-center">
              <p className="text-[0.85rem] text-muted-foreground">
                Latency depends on your Wi-Fi quality. 5 GHz band recommended for lowest latency.
                Use <em>Buffer Presets</em> above to trade off between latency and stability.
              </p>
              <a
                className="mt-3 block text-[0.9rem] text-primary hover:underline"
                href="https://github.com/apirJS/gemacast"
                target="_blank"
                rel="noopener noreferrer"
              >
                GitHub — apirJS/gemacast
              </a>
              <p className="mt-4 text-[0.8rem] text-muted-foreground">v0.1.0 · 2026 ApirJS</p>
            </div>
          </div>
        </div>
      </dialog>

      <HelpDialog activeKey={help.activeKey} onClose={help.closeHelp} dialogRef={help.dialogRef} />
    </>
  );
}
