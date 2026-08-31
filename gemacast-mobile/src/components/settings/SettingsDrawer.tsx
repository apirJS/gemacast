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
  const { open, handleOpen, handleClose } = useDrawer('settings');
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

      <div
        role="dialog"
        aria-label="Settings"
        inert={!open}
        className={`
          fixed top-0 left-0 z-50 flex h-dvh w-screen flex-col overflow-hidden
          border-r border-border bg-background text-foreground
          shadow-[4px_0_24px_rgba(0,0,0,0.2)]
          transition-transform duration-300 ease-out
          ${open ? 'translate-x-0' : '-translate-x-full'}
        `}
        style={{
          paddingBottom: 'calc(1rem + env(safe-area-inset-bottom, 0px))',
        }}
      >
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
            <SectionLabel helpButton={help.renderHelpButton('audio-gain')}>Audio Gain</SectionLabel>
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
            <SectionLabel helpButton={help.renderHelpButton('connection-mode')}>Mode</SectionLabel>
            <ModeSelector />
          </div>

          <SectionDivider />

          <div>
            <SectionLabel>Paired PCs</SectionLabel>
            <ForgetPcIdentity />
          </div>

          {/* Footer */}
          <div className="flex flex-col gap-y-1 mt-4 border-t border-border pt-6 text-center justify-center">
            <p className="text-xs text-muted-foreground">
              USB Tethering or 5 GHz Wi-Fi is recommended for lowest latency. Use{' '}
              <em>Buffer Presets</em> above to trade off between delay and stability.
            </p>
            <a
              className="mt-3 flex flex-row justify-center items-center text-[0.9rem] text-primary hover:underline gap-x-2"
              href="https://github.com/apirJS/gemacast"
              target="_blank"
              rel="noopener noreferrer"
            >
              <svg
                width={30}
                height={30}
                fill="#000000"
                viewBox="0 -0.5 25 25"
                xmlns="http://www.w3.org/2000/svg"
              >
                <g id="SVGRepo_bgCarrier" stroke-width="0"></g>
                <g id="SVGRepo_tracerCarrier" stroke-linecap="round" stroke-linejoin="round"></g>
                <g id="SVGRepo_iconCarrier">
                  <path d="m12.301 0h.093c2.242 0 4.34.613 6.137 1.68l-.055-.031c1.871 1.094 3.386 2.609 4.449 4.422l.031.058c1.04 1.769 1.654 3.896 1.654 6.166 0 5.406-3.483 10-8.327 11.658l-.087.026c-.063.02-.135.031-.209.031-.162 0-.312-.054-.433-.144l.002.001c-.128-.115-.208-.281-.208-.466 0-.005 0-.01 0-.014v.001q0-.048.008-1.226t.008-2.154c.007-.075.011-.161.011-.249 0-.792-.323-1.508-.844-2.025.618-.061 1.176-.163 1.718-.305l-.076.017c.573-.16 1.073-.373 1.537-.642l-.031.017c.508-.28.938-.636 1.292-1.058l.006-.007c.372-.476.663-1.036.84-1.645l.009-.035c.209-.683.329-1.468.329-2.281 0-.045 0-.091-.001-.136v.007c0-.022.001-.047.001-.072 0-1.248-.482-2.383-1.269-3.23l.003.003c.168-.44.265-.948.265-1.479 0-.649-.145-1.263-.404-1.814l.011.026c-.115-.022-.246-.035-.381-.035-.334 0-.649.078-.929.216l.012-.005c-.568.21-1.054.448-1.512.726l.038-.022-.609.384c-.922-.264-1.981-.416-3.075-.416s-2.153.152-3.157.436l.081-.02q-.256-.176-.681-.433c-.373-.214-.814-.421-1.272-.595l-.066-.022c-.293-.154-.64-.244-1.009-.244-.124 0-.246.01-.364.03l.013-.002c-.248.524-.393 1.139-.393 1.788 0 .531.097 1.04.275 1.509l-.01-.029c-.785.844-1.266 1.979-1.266 3.227 0 .025 0 .051.001.076v-.004c-.001.039-.001.084-.001.13 0 .809.12 1.591.344 2.327l-.015-.057c.189.643.476 1.202.85 1.693l-.009-.013c.354.435.782.793 1.267 1.062l.022.011c.432.252.933.465 1.46.614l.046.011c.466.125 1.024.227 1.595.284l.046.004c-.431.428-.718 1-.784 1.638l-.001.012c-.207.101-.448.183-.699.236l-.021.004c-.256.051-.549.08-.85.08-.022 0-.044 0-.066 0h.003c-.394-.008-.756-.136-1.055-.348l.006.004c-.371-.259-.671-.595-.881-.986l-.007-.015c-.198-.336-.459-.614-.768-.827l-.009-.006c-.225-.169-.49-.301-.776-.38l-.016-.004-.32-.048c-.023-.002-.05-.003-.077-.003-.14 0-.273.028-.394.077l.007-.003q-.128.072-.08.184c.039.086.087.16.145.225l-.001-.001c.061.072.13.135.205.19l.003.002.112.08c.283.148.516.354.693.603l.004.006c.191.237.359.505.494.792l.01.024.16.368c.135.402.38.738.7.981l.005.004c.3.234.662.402 1.057.478l.016.002c.33.064.714.104 1.106.112h.007c.045.002.097.002.15.002.261 0 .517-.021.767-.062l-.027.004.368-.064q0 .609.008 1.418t.008.873v.014c0 .185-.08.351-.208.466h-.001c-.119.089-.268.143-.431.143-.075 0-.147-.011-.214-.032l.005.001c-4.929-1.689-8.409-6.283-8.409-11.69 0-2.268.612-4.393 1.681-6.219l-.032.058c1.094-1.871 2.609-3.386 4.422-4.449l.058-.031c1.739-1.034 3.835-1.645 6.073-1.645h.098-.005zm-7.64 17.666q.048-.112-.112-.192-.16-.048-.208.032-.048.112.112.192.144.096.208-.032zm.497.545q.112-.08-.032-.256-.16-.144-.256-.048-.112.08.032.256.159.157.256.047zm.48.72q.144-.112 0-.304-.128-.208-.272-.096-.144.08 0 .288t.272.112zm.672.673q.128-.128-.064-.304-.192-.192-.32-.048-.144.128.064.304.192.192.32.044zm.913.4q.048-.176-.208-.256-.24-.064-.304.112t.208.24q.24.097.304-.096zm1.009.08q0-.208-.272-.176-.256 0-.256.176 0 .208.272.176.256.001.256-.175zm.929-.16q-.032-.176-.288-.144-.256.048-.224.24t.288.128.225-.224z"></path>
                </g>
              </svg>
              <span> apirJS/gemacast</span>
            </a>

            <p className="mt-4 text-[0.8rem] text-muted-foreground">
              v{packageJson.version} · 2026 Echa Apriliyanto
            </p>
          </div>
        </div>
      </div>

      <HelpDialog activeKey={help.activeKey} onClose={help.closeHelp} dialogRef={help.dialogRef} />
    </>
  );
}
