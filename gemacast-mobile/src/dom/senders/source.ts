import { AudioSource, ProcessInfo } from '../../types';

export function sourceLabel(source: AudioSource): string {
  if (source.type === 'desktop') return 'Desktop Audio';
  return source.name;
}

/** Monitor icon — desktop system audio */
const desktopSvg = `<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="2" y="3" width="20" height="14" rx="2" ry="2"/><line x1="8" y1="21" x2="16" y2="21"/><line x1="12" y1="17" x2="12" y2="21"/></svg>`;

/** Speaker with sound waves — process actively producing audio */
const speakerActiveSvg = `<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5"/><path d="M15.54 8.46a5 5 0 0 1 0 7.07"/><path d="M19.07 4.93a10 10 0 0 1 0 14.14"/></svg>`;

/** Window/app icon — process with no active audio session */
const processSilentSvg = `<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="18" height="18" rx="2" ry="2"/><line x1="3" y1="9" x2="21" y2="9"/><line x1="9" y1="3" x2="9" y2="9"/></svg>`;

export function sourceIcon(source: AudioSource): string {
  if (source.type === 'desktop') return desktopSvg;
  return source.hasAudioSession ? speakerActiveSvg : processSilentSvg;
}

export function sourcesEqual(a: AudioSource, b: AudioSource): boolean {
  if (a.type !== b.type) return false;
  if (a.type === 'desktop') return true;
  return a.type === 'process' && b.type === 'process' && a.pid === b.pid;
}

/**
 * Builds the combined source list: Desktop first, then process entries derived
 * from the process list merged with audio sources.
 */
export function buildSourceOptions(
  audioSources: AudioSource[],
  processList: ProcessInfo[],
): AudioSource[] {
  const sources: AudioSource[] = [{ type: 'desktop' }];

  // Build a lookup of audio session status from the process list
  const audioSessionByPid = new Map<number, boolean>();
  for (const proc of processList) {
    audioSessionByPid.set(proc.pid, proc.hasAudioSession);
  }

  // Existing process audio sources (currently playing audio)
  const existingPids = new Set<number>();
  for (const src of audioSources) {
    if (src.type === 'process') {
      existingPids.add(src.pid);
      // Inherit hasAudioSession from the process list if available
      const hasAudio = audioSessionByPid.get(src.pid) ?? false;
      sources.push({ ...src, hasAudioSession: hasAudio });
    }
  }

  // Add all processes from the full process list that aren't already present
  for (const proc of processList) {
    if (!existingPids.has(proc.pid)) {
      sources.push({
        type: 'process',
        pid: proc.pid,
        name: proc.name,
        hasAudioSession: proc.hasAudioSession,
      });
    }
  }

  // Sort: Desktop first (already at index 0), then audio-active processes,
  // then inactive — alphabetically within each group
  const desktop = sources[0];
  const processSources = sources.slice(1);
  processSources.sort((a, b) => {
    if (a.type !== 'process' || b.type !== 'process') return 0;
    const aAudio = a.hasAudioSession ? 1 : 0;
    const bAudio = b.hasAudioSession ? 1 : 0;
    if (bAudio !== aAudio) return bAudio - aAudio;
    return a.name.toLowerCase().localeCompare(b.name.toLowerCase());
  });

  return [desktop, ...processSources];
}
