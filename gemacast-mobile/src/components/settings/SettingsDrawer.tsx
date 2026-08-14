import { X } from 'lucide-react';
import { ThemeToggle } from './ThemeToggle';
import { BufferPresetSelect } from './BufferPresetSelect';
import { CustomJitterConfig } from './CustomJitterConfig';
import { BitrateSelect } from './BitrateSelect';
import { GainSlider } from './GainSlider';
import { ExclusiveToggle } from './ExclusiveToggle';
import { KeepScreenOnToggle } from './KeepScreenOnToggle';
import { ModeSelector } from './ModeSelector';
import { UpdateBanner } from './UpdateBanner';
import { ForgetPcIdentity } from './ForgetPcIdentity';
import { HelpDialog, useHelpDialog } from '../shared/HelpDialog';
import { useAppStore } from '../../stores/app-store';
import { useDrawer } from '../../hooks/use-drawer';
import packageJson from '../../../package.json';

function SectionLabel({
  children,
  helpButton,
}: {
  children: React.ReactNode;
  helpButton?: React.ReactNode;
}) {
  return (
    <div className="mb-2 flex items-center gap-2 text-[0.8rem] font-semibold tracking-[0.04em] text-muted-foreground">
      {children}
      {helpButton}
    </div>
  );
}

function SectionDivider() {
  return <div className="border-t border-border" />;
}

export function SettingsDrawer() {
  const { open, closing, dialogRef, handleOpen, handleClose } = useDrawer('settings');
  const help = useHelpDialog();
  const exclusiveSupported = useAppStore((s) => s.exclusiveSupported);

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
          fixed inset-y-0 left-0 z-50 m-0 h-screen max-h-none w-screen max-w-[100vw]
          border-none border-r border-border bg-background p-0 text-foreground
          shadow-[4px_0_24px_rgba(0,0,0,0.2)]
          backdrop:bg-black/30 backdrop:backdrop-blur-xs
          ${closing ? 'backdrop:animate-[backdrop-fade-out_150ms_ease-in_forwards]' : ''}
          ${closing ? 'animate-[slide-out-left_350ms_cubic-bezier(0.32,0.72,0,1)_forwards]' : open ? 'animate-[slide-from-left_350ms_cubic-bezier(0.32,0.72,0,1)]' : ''}
        `}
        style={{
          paddingBottom: 'calc(1rem + env(safe-area-inset-bottom, 0px))',
        }}
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

          <div className="flex-1 space-y-5 overflow-y-auto px-5 py-5">
            <UpdateBanner />

            {/* Buffer Preset */}
            <div>
              <SectionLabel helpButton={help.renderHelpButton('buffer-preset')}>
                Buffer Preset
              </SectionLabel>
              <BufferPresetSelect />
              <CustomJitterConfig renderHelpButton={help.renderHelpButton} />
            </div>

            <SectionDivider />

            {/* Audio Bitrate Quality */}
            <div>
              <SectionLabel helpButton={help.renderHelpButton('audio-bitrate')}>
                Audio Bitrate Quality
              </SectionLabel>
              <BitrateSelect />
            </div>

            <SectionDivider />

            {/* Audio Gain */}
            <div>
              <SectionLabel helpButton={help.renderHelpButton('audio-gain')}>
                Audio Gain
              </SectionLabel>
              <GainSlider />
            </div>

            <SectionDivider />

            {/* Toggles */}
            <div className="space-y-4">
              <div className="flex items-center justify-between">
                <div>
                  <SectionLabel helpButton={help.renderHelpButton('exclusive-mode')}>
                    Exclusive Mode
                  </SectionLabel>
                  {!exclusiveSupported && (
                    <p className="-mt-1 text-[0.7rem] text-muted-foreground/70">
                      Not supported on this device
                    </p>
                  )}
                </div>
                <ExclusiveToggle />
              </div>

              <div className="flex items-center justify-between">
                <SectionLabel helpButton={help.renderHelpButton('keep-screen-on')}>
                  Keep Screen On
                </SectionLabel>
                <KeepScreenOnToggle />
              </div>
            </div>

            <SectionDivider />

            {/* Connection Mode */}
            <div>
              <SectionLabel helpButton={help.renderHelpButton('connection-mode')}>
                Mode
              </SectionLabel>
              <ModeSelector />
            </div>

            <SectionDivider />

            <div>
              <SectionLabel>Paired PCs</SectionLabel>
              <ForgetPcIdentity />
            </div>

            {/* Footer */}
            <div className="mt-4 border-t border-border pt-6 text-center">
              <p className="text-xs text-muted-foreground">
                USB Tethering or 5 GHZ Wi-Fi is recommended for lowest latency. Use{' '}
                <em>Buffer Presets</em> above to trade off between latency and stability.
              </p>
              <a
                className="mt-3 block text-[0.9rem] text-primary hover:underline"
                href="https://github.com/apirJS/gemacast"
                target="_blank"
                rel="noopener noreferrer"
              >
                GitHub — apirJS/gemacast
              </a>
              <p className="mt-4 text-[0.8rem] text-muted-foreground">
                v{packageJson.version} · 2026 Echa Apriliyanto
              </p>
            </div>
          </div>
        </div>
      </dialog>

      <HelpDialog activeKey={help.activeKey} onClose={help.closeHelp} dialogRef={help.dialogRef} />
    </>
  );
}
