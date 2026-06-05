import React, { useState, useEffect } from 'react';
import { useCustomPresetEditor } from '../../hooks/use-custom-preset-editor';
import { SegmentedControl } from '../shared/SegmentedControl';
import { ConfirmDialog } from '../shared/ConfirmDialog';

function NumberInput({
  value,
  onChange,
  className,
  ...props
}: Omit<React.InputHTMLAttributes<HTMLInputElement>, 'value' | 'onChange'> & {
  value: number | null | undefined;
  onChange: (val: number | null) => void;
}) {
  const [local, setLocal] = useState(value == null ? '' : value.toString());

  // Sync local state when external value changes
  useEffect(() => {
    if (value == null) {
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setLocal('');
    } else {
      // Don't overwrite if the local text evaluates to the same number (e.g. '' == 0 or '02' == 2)
      // This prevents the annoying '0' from popping back in when the user deletes the text.
      const parsedLocal = local === '' ? 0 : Number(local);
      if (parsedLocal !== value) {
        setLocal(value.toString());
      }
    }
  }, [value, local]);

  return (
    <input
      type="number"
      value={local}
      onChange={(e) => {
        const val = e.target.value;
        setLocal(val);
        if (val === '') {
          onChange(null);
        } else {
          const parsed = Number(val);
          if (!isNaN(parsed)) {
            onChange(parsed);
          }
        }
      }}
      className={className}
      {...props}
    />
  );
}

type CustomJitterConfigProps = {
  renderHelpButton: (key: string) => React.ReactNode;
};

export function CustomJitterConfig({ renderHelpButton }: CustomJitterConfigProps) {
  const editor = useCustomPresetEditor();

  if (!editor.isCustom) return null;

  const { config } = editor;

  const FIELD_LABELS: Record<string, string> = {
    staticTargetMs: 'Buffer Depth',
    minDepthMs: 'Min Depth',
    comfortCapMs: 'Comfort Cap',
    peakDecayHalflifeMs: 'Peak Decay Half-life',
    resumeThresholdPct: 'Resume Threshold',
  };

  return (
    <div className="mt-3 flex flex-col gap-3 rounded-lg border border-border bg-background p-4 animate-[fade-in_200ms_ease-out]">
      <div className="flex items-center justify-between">
        <span className="flex items-center text-[0.9rem] text-foreground">Preset Name</span>
        <input
          type="text"
          value={editor.presetName}
          onChange={(e) => editor.setPresetName(e.target.value)}
          placeholder={
            editor.isEditingSaved
              ? editor.config && editor.savedMatchIndex >= 0
                ? undefined
                : 'My Preset'
              : 'My Preset'
          }
          maxLength={30}
          className="w-[140px] rounded-[4px] border border-border bg-background px-2 py-1 text-left text-foreground outline-none focus:border-primary focus:ring-1 focus:ring-primary"
        />
      </div>

      <div className="flex items-center justify-between">
        <span className="flex items-center text-[0.9rem] text-foreground">
          Buffer Mode
          {renderHelpButton('buffer-mode')}
        </span>
        <SegmentedControl
          name="buffer-mode"
          size="mini"
          value={editor.bufferMode}
          onChange={editor.setBufferMode}
          options={[
            { value: 'static', label: 'Static' },
            { value: 'adaptive', label: 'Adaptive' },
          ]}
        />
      </div>

      {editor.bufferMode === 'static' && (
        <div className="flex items-center justify-between">
          <span className="flex items-center text-[0.9rem] text-foreground">
            Buffer Depth
            {renderHelpButton('static-depth')}
          </span>
          <div className="flex items-center justify-end">
            <NumberInput
              value={config.staticTargetMs}
              onChange={(val) => editor.updateField({ staticTargetMs: val ?? null })}
              className="mr-1.5 w-[60px] rounded-[4px] border border-border bg-background px-2 py-1 text-right text-foreground outline-none focus:border-primary focus:ring-1 focus:ring-primary"
            />
            <span className="text-foreground w-4 text-right">ms</span>
          </div>
        </div>
      )}

      {editor.bufferMode === 'adaptive' && (
        <div className="flex flex-col gap-3">
          <div className="flex items-center justify-between">
            <span className="flex items-center text-[0.9rem] text-foreground">
              Min Depth
              {renderHelpButton('min-depth')}
            </span>
            <div className="flex items-center justify-end">
              <NumberInput
                value={config.minDepthMs}
                onChange={(val) => editor.updateField({ minDepthMs: val ?? NaN })}
                className="mr-1.5 w-[60px] rounded-[4px] border border-border bg-background px-2 py-1 text-right text-foreground outline-none focus:border-primary focus:ring-1 focus:ring-primary"
              />
              <span className="text-foreground w-4 text-right">ms</span>
            </div>
          </div>
          <div className="flex items-center justify-between">
            <span className="flex items-center text-[0.9rem] text-foreground">
              Comfort Cap
              {renderHelpButton('comfort-cap')}
            </span>
            <div className="flex items-center justify-end">
              <NumberInput
                value={config.comfortCapMs}
                onChange={(val) => editor.updateField({ comfortCapMs: val ?? NaN })}
                className="mr-1.5 w-[60px] rounded-[4px] border border-border bg-background px-2 py-1 text-right text-foreground outline-none focus:border-primary focus:ring-1 focus:ring-primary"
              />
              <span className="text-foreground w-4 text-right">ms</span>
            </div>
          </div>
          <div className="flex items-center justify-between">
            <span className="flex items-center text-[0.9rem] text-foreground">
              Peak Decay Half-life
              {renderHelpButton('bounce')}
            </span>
            <div className="flex items-center justify-end">
              <NumberInput
                step="0.1"
                value={config.peakDecayHalflifeMs}
                onChange={(val) => editor.updateField({ peakDecayHalflifeMs: val ?? NaN })}
                className="mr-1.5 w-[60px] rounded-[4px] border border-border bg-background px-2 py-1 text-right text-foreground outline-none focus:border-primary focus:ring-1 focus:ring-primary"
              />
              <span className="text-foreground w-4 text-right">ms</span>
            </div>
          </div>
          <div className="flex items-center justify-between">
            <span className="flex items-center text-[0.9rem] text-foreground">
              Resume Threshold
              {renderHelpButton('resume')}
            </span>
            <div className="flex items-center justify-end">
              <NumberInput
                step="0.01"
                value={config.resumeThresholdPct}
                onChange={(val) => editor.updateField({ resumeThresholdPct: val ?? NaN })}
                className="mr-1.5 w-[60px] rounded-[4px] border border-border bg-background px-2 py-1 text-right text-foreground outline-none focus:border-primary focus:ring-1 focus:ring-primary"
              />
              <span className="text-foreground w-4 text-right">%</span>
            </div>
          </div>
        </div>
      )}

      {!editor.isValid && editor.validationErrors.length > 0 && (
        <div className="mt-1 rounded-[6px] border border-destructive/20 bg-destructive/10 p-2 text-[0.8rem] text-destructive">
          <ul className="list-inside list-disc">
            {editor.validationErrors.map((err, i) => (
              <li key={i}>
                <strong>{FIELD_LABELS[err.field] || err.field}:</strong> {err.message}
              </li>
            ))}
          </ul>
        </div>
      )}

      <div className="mt-1 flex gap-2">
        <button
          type="button"
          className="flex-1 rounded-[6px] border border-border bg-muted p-[0.6rem] text-[0.9rem] font-semibold text-foreground transition-opacity hover:opacity-90 active:opacity-80"
          onClick={editor.handleReset}
        >
          Reset
        </button>
        <button
          type="button"
          className="flex-[2] rounded-[6px] bg-primary p-[0.6rem] text-[0.9rem] font-semibold text-primary-foreground transition-opacity hover:opacity-90 active:opacity-80 disabled:cursor-not-allowed disabled:opacity-40"
          onClick={editor.handleSave}
          disabled={!editor.canSave}
        >
          Save Preset
        </button>
      </div>

      {editor.isEditingSaved && (
        <button
          type="button"
          className="mt-1 w-full rounded-[6px] border border-destructive bg-destructive/10 p-[0.6rem] text-[0.9rem] font-semibold text-destructive transition-colors hover:bg-destructive hover:text-destructive-foreground active:opacity-80"
          onClick={editor.requestDelete}
        >
          Delete Preset
        </button>
      )}

      <ConfirmDialog
        open={editor.isDeleteDialogOpen}
        message="Are you sure you want to delete this saved preset? This action cannot be undone."
        confirmLabel="Delete"
        cancelLabel="Cancel"
        onConfirm={editor.confirmDelete}
        onCancel={editor.cancelDelete}
      />
    </div>
  );
}
