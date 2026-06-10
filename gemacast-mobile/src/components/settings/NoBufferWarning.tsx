export function NoBufferWarningDialog({
  dialogRef,
  dontShowAgain,
  setDontShowAgain,
  handleOk,
}: {
  dialogRef: React.RefObject<HTMLDialogElement | null>;
  dontShowAgain: boolean;
  setDontShowAgain: (v: boolean) => void;
  handleOk: () => void;
}) {
  return (
    <dialog
      ref={dialogRef}
      className="fixed inset-0 z-50 m-auto w-[min(90vw,360px)] rounded-[var(--radius-lg)] border border-border bg-popover p-5 text-popover-foreground shadow-xl backdrop:bg-black/50"
    >
      <h2 className="mb-2 text-base font-semibold text-foreground">No Buffer Mode</h2>
      <p className="mb-4 text-sm text-muted-foreground">
        This mode disables all audio buffering. Audio will play the instant it arrives, with zero
        safety net.
      </p>
      <p className="mb-4 text-sm text-muted-foreground">
        On unstable Wi-Fi connections, you may experience crackling, pops, or brief audio drops.
        This mode is best suited for wired connections (USB/ADB) or very stable 5 GHz Wi-Fi.
      </p>

      <label className="mb-4 flex items-center gap-2 cursor-pointer select-none">
        <input
          type="checkbox"
          checked={dontShowAgain}
          onChange={(e) => setDontShowAgain(e.target.checked)}
          className="h-4 w-4 accent-primary rounded"
        />
        <span className="text-sm text-muted-foreground">Don&apos;t show this again</span>
      </label>

      <div className="flex justify-end">
        <button
          type="button"
          className="rounded-[var(--radius-default)] bg-primary px-5 py-2 text-sm font-semibold text-primary-foreground transition-opacity hover:opacity-90 active:opacity-80"
          onClick={handleOk}
        >
          Ok
        </button>
      </div>
    </dialog>
  );
}
