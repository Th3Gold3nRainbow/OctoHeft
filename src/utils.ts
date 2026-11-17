export function formatBytes(bytes: number, decimals = 2, language?: string): string {
  const isFrench = language?.toLowerCase().startsWith('fr');
  const sizes = isFrench ? ['o', 'Ko', 'Mo', 'Go', 'To'] : ['B', 'KB', 'MB', 'GB', 'TB'];
  if (!bytes || bytes <= 0 || !isFinite(bytes)) return `0 ${sizes[0]}`;
  const k = 1024;
  const dm = decimals < 0 ? 0 : decimals;
  const rawIdx = Math.floor(Math.log(bytes) / Math.log(k));
  const i = Math.max(0, Math.min(sizes.length - 1, rawIdx));
  return `${parseFloat((bytes / Math.pow(k, i)).toFixed(dm))} ${sizes[i]}`;
}

export function formatSpeed(bytesPerSec: number, language?: string): string {
  const isFrench = language?.toLowerCase().startsWith('fr');
  const units = isFrench ? ['o/s', 'Ko/s', 'Mo/s', 'Go/s', 'To/s'] : ['B/s', 'KB/s', 'MB/s', 'GB/s', 'TB/s'];
  if (!bytesPerSec || bytesPerSec <= 0 || !isFinite(bytesPerSec)) {
    return `0.0 ${units[0]}`;
  }
  const k = 1024;
  let i = Math.max(0, Math.min(units.length - 1, Math.floor(Math.log(bytesPerSec) / Math.log(k))));
  let scaled = bytesPerSec / Math.pow(k, i);
  if (Number(scaled.toFixed(1)) >= k && i < units.length - 1) {
    i += 1;
    scaled = bytesPerSec / Math.pow(k, i);
  }
  return `${scaled.toFixed(1)} ${units[i]}`;
}

export function formatEta(seconds: number | null): string {
  if (seconds === null || seconds < 0 || !isFinite(seconds)) return '--:--';
  if (seconds > 86400) return '> 1 day';
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = Math.floor(seconds % 60);

  if (h > 0) {
    return `${h}h ${m}m ${s}s`;
  }
  if (m > 0) {
    return `${m}m ${s}s`;
  }
  return `${s}s`;
}

export function playCompletionChime(): void {
  try {
    const AudioCtx =
      window.AudioContext ||
      (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
    if (!AudioCtx) return;

    const ctx = new AudioCtx();
    if (ctx.state === 'suspended') {
      ctx.resume().catch(() => {});
    }

    const now = ctx.currentTime;
    const playTone = (freq: number, startOffset: number, duration: number, peakGain: number) => {
      const osc = ctx.createOscillator();
      const gain = ctx.createGain();
      osc.type = 'triangle';
      osc.frequency.setValueAtTime(freq, now + startOffset);
      gain.gain.setValueAtTime(0.0001, now + startOffset);
      gain.gain.exponentialRampToValueAtTime(peakGain, now + startOffset + 0.02);
      gain.gain.exponentialRampToValueAtTime(0.0001, now + startOffset + duration);
      osc.connect(gain);
      gain.connect(ctx.destination);
      osc.start(now + startOffset);
      osc.stop(now + startOffset + duration + 0.03);
    };

    // Clean two-note Fluent ascending chime (D5 -> A5)
    playTone(587.33, 0, 0.22, 0.14);
    playTone(880.0, 0.12, 0.34, 0.12);

    setTimeout(() => {
      ctx.close().catch(() => {});
    }, 800);
  } catch {
    // Ignore audio API errors on restricted headless runtimes
  }
}
