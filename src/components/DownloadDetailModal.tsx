import React, { useEffect, useState } from 'react';
import { AntivirusScanResult, DownloadTask, HashVerificationResult } from '../types';
import { formatBytes, formatEta, formatSpeed } from '../utils';
import { SpeedGraph } from './SpeedGraph';
import { useI18n } from '../i18n';
import {
  X,
  Play,
  Pause,
  Folder,
  ExternalLink,
  Activity,
  Cpu,
  Shield,
  CheckCircle2,
  AlertCircle,
  Copy,
  Loader2,
  Film,
  Ban,
  FolderOpen,
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';

interface DownloadDetailModalProps {
  task: DownloadTask;
  openOnFinish: boolean;
  onToggleOpenOnFinish: (checked: boolean) => void;
  autoCloseOnFinish: boolean;
  onToggleAutoCloseOnFinish: (checked: boolean) => void;
  onClose: () => void;
  onCancelTask: (id: string) => void;
  onPause: (id: string) => void;
  onResume: (id: string) => void;
  onOpenFile: (path: string) => void;
  onOpenFolder: (path: string) => void;
}

const SEGMENT_COLORS = [
  'bg-sky-600',
  'bg-cyan-500',
  'bg-indigo-500',
  'bg-blue-600',
  'bg-teal-500',
  'bg-sky-500',
];

export const DownloadDetailModal: React.FC<DownloadDetailModalProps> = ({
  task,
  openOnFinish,
  onToggleOpenOnFinish,
  autoCloseOnFinish,
  onToggleAutoCloseOnFinish,
  onClose,
  onCancelTask,
  onPause,
  onResume,
  onOpenFile,
  onOpenFolder,
}) => {
  const { t, language } = useI18n();
  const [activeTab, setActiveTab] = useState<'segments' | 'integrity' | 'media'>('segments');
  const [speedHistory, setSpeedHistory] = useState<number[]>([]);

  // Integrity & Antivirus State
  const [expectedHash, setExpectedHash] = useState('');
  const [hashResult, setHashResult] = useState<HashVerificationResult | null>(null);
  const [hashing, setHashing] = useState(false);
  const [scanResult, setScanResult] = useState<AntivirusScanResult | null>(null);
  const [scanning, setScanning] = useState(false);
  const [copiedHash, setCopiedHash] = useState<string | null>(null);

  useEffect(() => {
    setSpeedHistory((prev) => {
      const next = [...prev, task.speed];
      if (next.length > 30) next.shift();
      return next;
    });
  }, [task.speed, task.downloaded_bytes]);

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        onClose();
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [onClose]);

  const segments = Array.isArray(task.segments) ? task.segments : [];
  const percentage =
    task.total_size > 0
      ? Math.min(100, Math.floor((task.downloaded_bytes / task.total_size) * 100))
      : task.status === 'completed'
      ? 100
      : 0;

  const isCompleted = task.status === 'completed';
  const isDownloading = task.status === 'downloading' || task.status === 'connecting';

  const getStatusBadge = () => {
    switch (task.status) {
      case 'downloading':
        return (
          <span className="px-2.5 py-1 rounded-full text-[11px] font-semibold bg-sky-50 dark:bg-sky-950/80 text-sky-700 dark:text-cyan-300 border border-sky-200 dark:border-cyan-500/40 shrink-0">
            {t('status.downloading')}
          </span>
        );
      case 'connecting':
        return (
          <span className="px-2.5 py-1 rounded-full text-[11px] font-semibold bg-cyan-50 dark:bg-cyan-950/80 text-cyan-700 dark:text-cyan-300 border border-cyan-200 dark:border-cyan-500/40 shrink-0">
            {t('status.connecting')}
          </span>
        );
      case 'completed':
        return (
          <span className="px-2.5 py-1 rounded-full text-[11px] font-semibold bg-emerald-50 dark:bg-emerald-950/70 text-emerald-700 dark:text-emerald-300 border border-emerald-200 dark:border-emerald-500/40 shrink-0">
            {t('status.completed')}
          </span>
        );
      case 'paused':
        return (
          <span className="px-2.5 py-1 rounded-full text-[11px] font-semibold bg-amber-50 dark:bg-amber-950/70 text-amber-800 dark:text-amber-300 border border-amber-200 dark:border-amber-500/40 shrink-0">
            {t('status.paused')}
          </span>
        );
      case 'error':
        return (
          <span
            className="px-2.5 py-1 rounded-full text-[11px] font-semibold bg-rose-50 dark:bg-rose-950/70 text-rose-700 dark:text-rose-300 border border-rose-200 dark:border-rose-500/40 shrink-0"
            title={task.error_message || ''}
          >
            {t('status.error')}
          </span>
        );
      default:
        return (
          <span className="px-2.5 py-1 rounded-full text-[11px] font-semibold bg-slate-100 dark:bg-slate-800 text-slate-600 dark:text-slate-300 border border-slate-200 dark:border-slate-700 shrink-0">
            {t('status.queued')}
          </span>
        );
    }
  };

  const handleComputeHash = async () => {
    setHashing(true);
    try {
      const res = await invoke<HashVerificationResult>('verify_file_hash', {
        path: task.save_path,
        expectedHash: expectedHash.trim() || null,
      });
      setHashResult(res);
    } catch (err) {
      alert(t('detailModal.hashError', { error: String(err) }));
    } finally {
      setHashing(false);
    }
  };

  const handleScanAntivirus = async () => {
    setScanning(true);
    try {
      const res = await invoke<AntivirusScanResult>('scan_file_antivirus', {
        path: task.save_path,
        customScanner: null,
        customArgs: null,
      });
      setScanResult(res);
    } catch (err) {
      alert(t('detailModal.scanError', { error: String(err) }));
    } finally {
      setScanning(false);
    }
  };

  const copyToClipboard = (text: string, label: string) => {
    navigator.clipboard.writeText(text);
    setCopiedHash(label);
    setTimeout(() => setCopiedHash(null), 1500);
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/35 dark:bg-black/60 backdrop-blur-sm p-4"
      role="dialog"
      aria-modal="true"
      aria-labelledby="download-detail-title"
    >
      <div className="card-surface w-full max-w-3xl rounded-2xl border border-slate-200/90 dark:border-slate-700 bg-white/95 dark:bg-slate-900/95 backdrop-blur-xl shadow-2xl shadow-slate-900/15 dark:shadow-black/50 flex flex-col max-h-[90vh] overflow-hidden">
        {/* Header */}
        <div className="octoheft-subcard flex items-center justify-between border-b border-slate-200 dark:border-slate-700 px-6 py-4 bg-slate-50/90 dark:bg-slate-900/90">
          <div className="flex items-center gap-3 overflow-hidden min-w-0">
            <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-sky-50 dark:bg-sky-950/80 text-sky-700 dark:text-cyan-300 border border-sky-200 dark:border-cyan-500/40">
              <Activity className="h-5 w-5" />
            </div>
            <div className="overflow-hidden min-w-0">
              <div className="flex items-center gap-2">
                <h2
                  id="download-detail-title"
                  className="text-base font-semibold text-slate-900 dark:text-slate-100 truncate"
                >
                  {task.file_name}
                </h2>
                {getStatusBadge()}
              </div>
              <p className="text-[11px] text-slate-500 dark:text-slate-400 truncate font-mono mt-0.5" dir="ltr">
                {task.url}
              </p>
            </div>
          </div>
          <button
            type="button"
            onClick={onClose}
            aria-label={t('common.close')}
            className="rounded-lg p-1.5 text-slate-500 dark:text-slate-400 hover:bg-slate-200/70 dark:hover:bg-slate-700/70 hover:text-slate-900 dark:hover:text-white transition-colors shrink-0 ms-2"
          >
            <X className="h-5 w-5" />
          </button>
        </div>

        {/* Tab Navigation */}
        <div className="octoheft-subcard flex border-b border-slate-200 dark:border-slate-700 bg-slate-50/60 dark:bg-slate-900/70 px-6 gap-2 text-xs">
          <button
            type="button"
            onClick={() => setActiveTab('segments')}
            className={`py-2.5 px-3 font-medium border-b-2 flex items-center gap-1.5 transition-colors ${
              activeTab === 'segments'
                ? 'border-sky-600 dark:border-cyan-400 text-sky-700 dark:text-cyan-300 font-semibold'
                : 'border-transparent text-slate-500 dark:text-slate-400 hover:text-slate-900 dark:hover:text-white'
            }`}
          >
            <Cpu className="h-3.5 w-3.5" /> {t('detailModal.tabSegments')}
          </button>
          <button
            type="button"
            onClick={() => setActiveTab('integrity')}
            className={`py-2.5 px-3 font-medium border-b-2 flex items-center gap-1.5 transition-colors ${
              activeTab === 'integrity'
                ? 'border-sky-600 dark:border-cyan-400 text-sky-700 dark:text-cyan-300 font-semibold'
                : 'border-transparent text-slate-500 dark:text-slate-400 hover:text-slate-900 dark:hover:text-white'
            }`}
          >
            <Shield className="h-3.5 w-3.5" /> {t('detailModal.tabIntegrity')}
          </button>
          <button
            type="button"
            onClick={() => setActiveTab('media')}
            className={`py-2.5 px-3 font-medium border-b-2 flex items-center gap-1.5 transition-colors ${
              activeTab === 'media'
                ? 'border-indigo-600 dark:border-indigo-400 text-indigo-700 dark:text-indigo-300 font-semibold'
                : 'border-transparent text-slate-500 dark:text-slate-400 hover:text-slate-900 dark:hover:text-white'
            }`}
          >
            <Film className="h-3.5 w-3.5" /> {t('detailModal.tabMedia')}
            {task.media_info?.video_resolution && (
              <span className="px-1.5 py-0.2 rounded text-[10px] font-mono bg-indigo-50 dark:bg-indigo-950/70 text-indigo-700 dark:text-indigo-300 border border-indigo-200 dark:border-indigo-500/40">
                {task.media_info.video_resolution}
              </span>
            )}
          </button>
        </div>

        {/* Tab 1: IDM-Style Progress Dialog, Segments & Live Metrics */}
        {activeTab === 'segments' && (
          <div className="flex-1 overflow-y-auto p-6 space-y-5">
            {/* Save Path Row */}
            <div className="octoheft-subcard flex items-center justify-between gap-3 px-3.5 py-2.5 rounded-xl bg-slate-50 dark:bg-slate-800/80 border border-slate-200 dark:border-slate-700 text-xs">
              <div className="flex items-center gap-2 min-w-0">
                <FolderOpen className="h-4 w-4 text-amber-600 dark:text-amber-400 shrink-0" />
                <span className="text-slate-500 dark:text-slate-400 font-medium shrink-0">
                  {t('detailModal.saveLocation')}:
                </span>
                <span className="font-mono text-[11px] text-slate-800 dark:text-slate-200 truncate select-all" dir="ltr">
                  {task.save_path}
                </span>
              </div>
              {task.media_info && (
                <span className="px-2 py-0.5 rounded-full text-[10px] font-bold bg-indigo-50 dark:bg-indigo-950/70 text-indigo-700 dark:text-indigo-300 border border-indigo-200 dark:border-indigo-500/40 shrink-0 font-mono">
                  {task.media_info.video_resolution || task.media_info.protocol} •{' '}
                  {(task.media_info.mux_phase || 'downloading').toUpperCase()}{' '}
                  {Math.round(task.media_info.mux_percent || 0)}%
                </span>
              )}
            </div>

            {task.status === 'error' && task.error_message && (
              <div className="p-3 rounded-xl bg-rose-50 dark:bg-rose-950/70 border border-rose-200 dark:border-rose-500/40 text-rose-700 dark:text-rose-200 text-xs flex items-center gap-2">
                <AlertCircle className="h-4 w-4 shrink-0" />
                <span className="break-words">{task.error_message}</span>
              </div>
            )}

            {/* Signature IDM Segment Chunk Bar */}
            <div className="octoheft-subcard rounded-xl border border-slate-200 dark:border-slate-700 bg-slate-50/80 dark:bg-slate-800/80 p-4">
              <div className="flex justify-between items-center text-xs text-slate-700 dark:text-slate-200 mb-2 font-mono">
                <span className="accent-neon font-semibold text-sky-800 dark:text-cyan-300">
                  {t('detailModal.overallProgress', { percent: percentage })}
                </span>
                <span>
                  {formatBytes(task.downloaded_bytes)} / {formatBytes(task.total_size)}
                </span>
              </div>
              <div className="h-4 w-full bg-slate-200/90 dark:bg-slate-700 rounded-lg overflow-hidden flex gap-[2px] p-[2px]">
                {segments.length > 0 ? (
                  segments.map((seg, idx) => {
                    const segSpan = seg.end_byte - seg.start_byte + 1;
                    const total = task.total_size > 0 ? task.total_size : 1;
                    const segWidthPct = Math.max(4, (segSpan / total) * 100);
                    const segDonePct =
                      segSpan > 0
                        ? Math.min(100, Math.max(0, (seg.downloaded / segSpan) * 100))
                        : isCompleted
                        ? 100
                        : 0;
                    const barColor =
                      seg.status === 'completed' || isCompleted
                        ? 'bg-emerald-500'
                        : SEGMENT_COLORS[idx % SEGMENT_COLORS.length];

                    return (
                      <div
                        key={seg.id}
                        style={{ width: `${segWidthPct}%` }}
                        className="h-full bg-white dark:bg-slate-900 rounded-sm overflow-hidden relative shadow-2xs"
                        title={t('detailModal.partTooltip', {
                          part: seg.id + 1,
                          percent: segDonePct.toFixed(1),
                        })}
                      >
                        <div
                          style={{ width: `${segDonePct}%` }}
                          className={`h-full transition-all duration-150 ${barColor}`}
                        />
                      </div>
                    );
                  })
                ) : (
                  <div
                    style={{ width: `${percentage}%` }}
                    className={`h-full rounded-sm transition-all ${
                      isCompleted ? 'bg-emerald-500' : 'bg-sky-600 dark:bg-cyan-400'
                    }`}
                  />
                )}
              </div>
            </div>

            {/* IDM Two-Column Metrics Grid & Live Speed Graph */}
            <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
              <div className="grid grid-cols-2 gap-2.5 text-xs">
                <div className="octoheft-subcard p-3 rounded-xl bg-slate-50 dark:bg-slate-800/80 border border-slate-200/90 dark:border-slate-700">
                  <span className="text-[11px] text-slate-500 dark:text-slate-400 block">{t('detailModal.fileSize')}</span>
                  <span className="text-sm font-bold font-mono text-slate-900 dark:text-slate-100 mt-0.5 block">
                    {formatBytes(task.total_size)}
                  </span>
                </div>

                <div className="octoheft-subcard p-3 rounded-xl bg-slate-50 dark:bg-slate-800/80 border border-slate-200/90 dark:border-slate-700">
                  <span className="text-[11px] text-slate-500 dark:text-slate-400 block">{t('detailModal.downloaded')}</span>
                  <span className="text-sm font-bold font-mono text-slate-900 dark:text-slate-100 mt-0.5 block">
                    {formatBytes(task.downloaded_bytes)}{' '}
                    <span className="text-sky-700 dark:text-cyan-300 text-xs">({percentage}%)</span>
                  </span>
                </div>

                <div className="octoheft-subcard p-3 rounded-xl bg-slate-50 dark:bg-slate-800/80 border border-slate-200/90 dark:border-slate-700">
                  <span className="text-[11px] text-slate-500 dark:text-slate-400 block">
                    {t('detailModal.currentSpeed')}
                  </span>
                  <span className="accent-neon inline-block min-w-[84px] text-sm font-bold font-mono tabular-nums text-sky-700 dark:text-cyan-300 mt-0.5">
                    {formatSpeed(task.speed, language)}
                  </span>
                </div>

                <div className="octoheft-subcard p-3 rounded-xl bg-slate-50 dark:bg-slate-800/80 border border-slate-200/90 dark:border-slate-700">
                  <span className="text-[11px] text-slate-500 dark:text-slate-400 block">
                    {t('detailModal.timeRemaining')}
                  </span>
                  <span className="text-sm font-bold font-mono text-slate-900 dark:text-slate-100 mt-0.5 block">
                    {isCompleted ? '0s' : formatEta(task.eta_seconds)}
                  </span>
                </div>

                <div className="octoheft-subcard p-3 rounded-xl bg-slate-50 dark:bg-slate-800/80 border border-slate-200/90 dark:border-slate-700">
                  <span className="text-[11px] text-slate-500 dark:text-slate-400 block">
                    {t('detailModal.resumeCapability')}
                  </span>
                  <span
                    className={`inline-block mt-1 px-2 py-0.5 rounded-full text-[11px] font-semibold ${
                      task.supports_range
                        ? 'bg-emerald-50 dark:bg-emerald-950/70 text-emerald-700 dark:text-emerald-300 border border-emerald-200 dark:border-emerald-500/40'
                        : 'bg-amber-50 dark:bg-amber-950/70 text-amber-800 dark:text-amber-300 border border-amber-200 dark:border-amber-500/40'
                    }`}
                  >
                    {task.supports_range ? t('detailModal.resumeYes') : t('detailModal.resumeNo')}
                  </span>
                </div>

                <div className="octoheft-subcard p-3 rounded-xl bg-slate-50 dark:bg-slate-800/80 border border-slate-200/90 dark:border-slate-700">
                  <span className="text-[11px] text-slate-500 dark:text-slate-400 block">
                    {t('detailModal.parallelStreams')}
                  </span>
                  <span className="text-sm font-bold font-mono text-slate-900 dark:text-slate-100 mt-0.5 block">
                    {t('detailModal.connectionsCount', {
                      count: segments.length || task.connections,
                    })}
                  </span>
                </div>
              </div>

              <SpeedGraph data={speedHistory} />
            </div>

            {/* Parallel Segment Visualizer */}
            <div>
              <h3 className="text-[11px] font-semibold uppercase tracking-wider text-slate-500 dark:text-slate-400 mb-2.5 flex items-center justify-between">
                <span>{t('detailModal.activeWorkers')}</span>
                <span className="font-mono text-slate-600 dark:text-slate-300">
                  {t('detailModal.workersCompleted', {
                    done: segments.filter((s) => s.status === 'completed').length,
                    total: segments.length,
                  })}
                </span>
              </h3>

              <div className="space-y-2 max-h-44 overflow-y-auto pe-1">
                {segments.map((seg, idx) => {
                  const segSpan = seg.end_byte - seg.start_byte + 1;
                  const segPercent =
                    segSpan > 0
                      ? Math.min(100, Math.floor((seg.downloaded / segSpan) * 100))
                      : isCompleted
                      ? 100
                      : 0;

                  return (
                    <div
                      key={seg.id}
                      className="octoheft-subcard p-2.5 rounded-xl bg-slate-50 dark:bg-slate-800/80 border border-slate-200/80 dark:border-slate-700 flex flex-col gap-1.5"
                    >
                      <div className="flex items-center justify-between text-xs">
                        <div className="flex items-center gap-2">
                          <span className="font-mono font-semibold text-slate-800 dark:text-slate-100">
                            {t('detailModal.connLabel', { id: seg.id + 1 })}
                          </span>
                          <span className="text-[11px] font-mono text-slate-500 dark:text-slate-400" dir="ltr">
                            [{formatBytes(seg.start_byte)} - {formatBytes(seg.end_byte)}]
                          </span>
                        </div>
                        <div className="flex items-center gap-3">
                          <span className="font-mono text-slate-700 dark:text-slate-200">
                            {formatBytes(seg.downloaded)} ({segPercent}%)
                          </span>
                          <span
                            className={`text-[10px] px-2 py-0.5 rounded-full font-semibold ${
                              seg.status === 'completed'
                                ? 'bg-emerald-50 dark:bg-emerald-950/70 text-emerald-700 dark:text-emerald-300 border border-emerald-200 dark:border-emerald-500/40'
                                : seg.status === 'downloading'
                                ? 'bg-sky-50 dark:bg-sky-950/80 text-sky-700 dark:text-cyan-300 border border-sky-200 dark:border-cyan-500/40'
                                : 'bg-slate-200 dark:bg-slate-700 text-slate-600 dark:text-slate-300'
                            }`}
                          >
                            {seg.status}
                          </span>
                        </div>
                      </div>

                      <div className="h-1.5 w-full bg-slate-200 dark:bg-slate-700 rounded-full overflow-hidden">
                        <div
                          style={{ width: `${segPercent}%` }}
                          className={`h-full transition-all duration-150 ${
                            seg.status === 'completed'
                              ? 'bg-emerald-500'
                              : SEGMENT_COLORS[idx % SEGMENT_COLORS.length]
                          }`}
                        />
                      </div>
                    </div>
                  );
                })}
              </div>
            </div>

            {/* Post-completion Options */}
            <div className="flex flex-wrap items-center gap-6 pt-1 border-t border-slate-100 dark:border-slate-700/80 text-xs text-slate-700 dark:text-slate-300">
              <label
                htmlFor="detail-open-on-finish"
                className="flex items-center gap-2 cursor-pointer select-none"
              >
                <input
                  id="detail-open-on-finish"
                  type="checkbox"
                  checked={openOnFinish}
                  onChange={(e) => onToggleOpenOnFinish(e.target.checked)}
                  className="h-4 w-4 rounded border-slate-300 dark:border-slate-600 text-sky-600 accent-sky-600 dark:accent-cyan-400 focus:ring-sky-500 cursor-pointer"
                />
                <span>{t('detailModal.openWhenFinished')}</span>
              </label>

              <label
                htmlFor="detail-auto-close"
                className="flex items-center gap-2 cursor-pointer select-none"
              >
                <input
                  id="detail-auto-close"
                  type="checkbox"
                  checked={autoCloseOnFinish}
                  onChange={(e) => onToggleAutoCloseOnFinish(e.target.checked)}
                  className="h-4 w-4 rounded border-slate-300 dark:border-slate-600 text-sky-600 accent-sky-600 dark:accent-cyan-400 focus:ring-sky-500 cursor-pointer"
                />
                <span>{t('detailModal.closeWhenComplete')}</span>
              </label>
            </div>
          </div>
        )}

        {/* Tab 2: Integrity & Antivirus Scanner */}
        {activeTab === 'integrity' && (
          <div className="flex-1 overflow-y-auto p-6 space-y-5 text-xs">
            <div className="octoheft-subcard p-4 rounded-xl bg-slate-50 dark:bg-slate-800/80 border border-slate-200 dark:border-slate-700 space-y-4">
              <div className="flex items-center justify-between gap-2">
                <div>
                  <h3 className="text-sm font-semibold text-slate-900 dark:text-slate-100">
                    {t('detailModal.cryptoTitle')}
                  </h3>
                  <p className="text-slate-600 dark:text-slate-300 text-[11px]">{t('detailModal.cryptoSubtitle')}</p>
                </div>
                <button
                  type="button"
                  onClick={handleComputeHash}
                  disabled={hashing}
                  className="px-3.5 py-1.5 rounded-lg text-xs font-semibold bg-sky-700 text-white hover:bg-sky-800 dark:hover:bg-sky-600 border border-transparent dark:border-cyan-400/30 disabled:opacity-50 transition-colors flex items-center gap-1.5"
                >
                  {hashing ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : null}
                  <span>{t('detailModal.computeHashes')}</span>
                </button>
              </div>

              <div>
                <label htmlFor="expected-hash-input" className="block text-slate-700 dark:text-slate-300 font-medium mb-1">
                  {t('detailModal.expectedHashLabel')}
                </label>
                <input
                  id="expected-hash-input"
                  type="text"
                  dir="ltr"
                  spellCheck={false}
                  autoCapitalize="none"
                  placeholder={t('detailModal.expectedHashPlaceholder')}
                  value={expectedHash}
                  onChange={(e) => setExpectedHash(e.target.value)}
                  className="w-full rounded-lg bg-white dark:bg-slate-900 border border-slate-300 dark:border-slate-700 px-3 py-1.5 text-xs text-slate-900 dark:text-slate-100 font-mono focus:border-sky-600 dark:focus:border-cyan-400 focus:outline-none focus:ring-2 focus:ring-sky-500/20"
                />
              </div>

              {hashResult && (
                <div className="space-y-3 pt-2 border-t border-slate-200 dark:border-slate-700">
                  {hashResult.is_match !== undefined && (
                    <div
                      className={`p-2.5 rounded-lg border flex items-center gap-2 ${
                        hashResult.is_match
                          ? 'bg-emerald-50 dark:bg-emerald-950/70 border-emerald-200 dark:border-emerald-500/40 text-emerald-800 dark:text-emerald-300'
                          : 'bg-rose-50 dark:bg-rose-950/70 border-rose-200 dark:border-rose-500/40 text-rose-800 dark:text-rose-200'
                      }`}
                    >
                      {hashResult.is_match ? (
                        <>
                          <CheckCircle2 className="h-4 w-4 text-emerald-600 dark:text-emerald-400 shrink-0" />
                          <span className="font-semibold">{t('detailModal.hashMatch')}</span>
                        </>
                      ) : (
                        <>
                          <AlertCircle className="h-4 w-4 text-rose-600 dark:text-rose-400 shrink-0" />
                          <span className="font-semibold">{t('detailModal.hashMismatch')}</span>
                        </>
                      )}
                    </div>
                  )}

                  <div className="space-y-2">
                    <div className="octoheft-subcard flex items-center justify-between p-2.5 rounded-lg bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-700">
                      <div className="overflow-hidden me-2">
                        <span className="text-[10px] text-slate-500 dark:text-slate-400 font-semibold uppercase block">
                          SHA-256
                        </span>
                        <span className="font-mono text-slate-800 dark:text-slate-200 truncate block select-all" dir="ltr">
                          {hashResult.computed_sha256}
                        </span>
                      </div>
                      <button
                        type="button"
                        onClick={() => copyToClipboard(hashResult.computed_sha256, 'sha256')}
                        className="text-slate-600 dark:text-slate-300 hover:text-sky-700 dark:hover:text-cyan-300 p-1 text-[10px] font-mono flex items-center gap-1 shrink-0"
                      >
                        <Copy className="h-3.5 w-3.5" />
                        <span>
                          {copiedHash === 'sha256' ? t('detailModal.copied') : t('detailModal.copy')}
                        </span>
                      </button>
                    </div>

                    <div className="octoheft-subcard flex items-center justify-between p-2.5 rounded-lg bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-700">
                      <div className="overflow-hidden me-2">
                        <span className="text-[10px] text-slate-500 dark:text-slate-400 font-semibold uppercase block">
                          MD5
                        </span>
                        <span className="font-mono text-slate-800 dark:text-slate-200 truncate block select-all" dir="ltr">
                          {hashResult.computed_md5}
                        </span>
                      </div>
                      <button
                        type="button"
                        onClick={() => copyToClipboard(hashResult.computed_md5, 'md5')}
                        className="text-slate-600 dark:text-slate-300 hover:text-sky-700 dark:hover:text-cyan-300 p-1 text-[10px] font-mono flex items-center gap-1 shrink-0"
                      >
                        <Copy className="h-3.5 w-3.5" />
                        <span>
                          {copiedHash === 'md5' ? t('detailModal.copied') : t('detailModal.copy')}
                        </span>
                      </button>
                    </div>
                  </div>
                </div>
              )}
            </div>

            <div className="octoheft-subcard p-4 rounded-xl bg-slate-50 dark:bg-slate-800/80 border border-slate-200 dark:border-slate-700 space-y-3">
              <div className="flex items-center justify-between gap-2">
                <div>
                  <h3 className="text-sm font-semibold text-slate-900 dark:text-slate-100 flex items-center gap-1.5">
                    <Shield className="h-4 w-4 text-indigo-600 dark:text-indigo-400" /> {t('detailModal.defenderTitle')}
                  </h3>
                  <p className="text-slate-600 dark:text-slate-300 text-[11px]">{t('detailModal.defenderSubtitle')}</p>
                </div>
                <button
                  type="button"
                  onClick={handleScanAntivirus}
                  disabled={scanning}
                  className="px-3.5 py-1.5 rounded-lg text-xs font-semibold bg-indigo-600 text-white hover:bg-indigo-700 disabled:opacity-50 transition-colors flex items-center gap-1.5"
                >
                  {scanning ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : null}
                  <span>{t('detailModal.scanWithDefender')}</span>
                </button>
              </div>

              {scanResult && (
                <div
                  className={`p-3 rounded-lg border flex items-start gap-2.5 ${
                    scanResult.is_clean
                      ? 'bg-emerald-50 dark:bg-emerald-950/70 border-emerald-200 dark:border-emerald-500/40 text-emerald-800 dark:text-emerald-300'
                      : 'bg-rose-50 dark:bg-rose-950/70 border-rose-200 dark:border-rose-500/40 text-rose-800 dark:text-rose-200'
                  }`}
                >
                  {scanResult.is_clean ? (
                    <CheckCircle2 className="h-4 w-4 text-emerald-600 dark:text-emerald-400 shrink-0 mt-0.5" />
                  ) : (
                    <AlertCircle className="h-4 w-4 text-rose-600 dark:text-rose-400 shrink-0 mt-0.5" />
                  )}
                  <div>
                    <span className="font-semibold block">
                      {scanResult.is_clean ? t('detailModal.scanClean') : t('detailModal.scanThreat')}
                    </span>
                    <span className="text-[11px] text-slate-600 dark:text-slate-300">{scanResult.details}</span>
                  </div>
                </div>
              )}
            </div>
          </div>
        )}

        {/* Tab 3: Media Stream & Remuxer */}
        {activeTab === 'media' && (() => {
          const isOctoMedia = task.media_info?.protocol === 'YT-DLP';
          return (
          <div className="flex-1 overflow-y-auto p-6 space-y-5 text-xs">
            <div className="octoheft-subcard rounded-xl border border-slate-200 dark:border-slate-700 bg-slate-50 dark:bg-slate-800/80 p-4 space-y-4">
              <div className="flex items-center justify-between gap-2">
                <div>
                  <h3 className="font-semibold text-slate-900 dark:text-slate-100 flex items-center gap-2">
                    <Film className="h-4 w-4 text-indigo-600 dark:text-indigo-400" />
                    <span>
                      {isOctoMedia ? t('detailModal.octoMediaTitle') : t('detailModal.mediaTitle')}
                    </span>
                  </h3>
                  <p className="text-[11px] text-slate-600 dark:text-slate-300 mt-0.5">
                    {isOctoMedia ? t('detailModal.octoMediaDesc') : t('detailModal.mediaDesc')}
                  </p>
                </div>
                {task.media_info && (
                  <span className="px-2.5 py-1 rounded-full text-[11px] font-bold bg-indigo-50 dark:bg-indigo-950/70 text-indigo-700 dark:text-indigo-300 border border-indigo-200 dark:border-indigo-500/40 shrink-0">
                    {t('detailModal.protocolStream', { protocol: task.media_info.protocol })}
                  </span>
                )}
              </div>

              {task.media_info ? (
                <div className="space-y-4 pt-2">
                  <div className="grid grid-cols-2 sm:grid-cols-4 gap-2.5">
                    <div className="octoheft-subcard p-3 rounded-xl bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-700">
                      <div className="text-[10px] text-slate-500 dark:text-slate-400 uppercase font-semibold">
                        {t('detailModal.videoResolution')}
                      </div>
                      <div className="text-sm font-bold font-mono text-sky-700 dark:text-cyan-300 mt-0.5">
                        {task.media_info.video_resolution ||
                          (task.media_info.audio_only
                            ? t('videoAddon.audioOnlyBadge')
                            : '1080p FHD')}
                      </div>
                    </div>
                    <div className="octoheft-subcard p-3 rounded-xl bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-700">
                      <div className="text-[10px] text-slate-500 dark:text-slate-400 uppercase font-semibold">
                        {t('detailModal.videoCodec')}
                      </div>
                      <div className="text-sm font-bold font-mono text-slate-900 dark:text-slate-100 mt-0.5">
                        {task.media_info.video_codec ||
                          (task.media_info.audio_only
                            ? t('videoAddon.audioOnlyBadge')
                            : 'H.264 / AVC')}
                      </div>
                    </div>
                    <div className="octoheft-subcard p-3 rounded-xl bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-700">
                      <div className="text-[10px] text-slate-500 dark:text-slate-400 uppercase font-semibold">
                        {t('detailModal.audioBitrate')}
                      </div>
                      <div className="text-sm font-bold font-mono text-cyan-700 dark:text-cyan-300 mt-0.5">
                        {task.media_info.audio_bitrate
                          ? `${task.media_info.audio_bitrate} kbps`
                          : task.media_info.audio_codec || '128 kbps AAC'}
                      </div>
                    </div>
                    <div className="octoheft-subcard p-3 rounded-xl bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-700">
                      <div className="text-[10px] text-slate-500 dark:text-slate-400 uppercase font-semibold">
                        {t('detailModal.segments')}
                      </div>
                      <div className="text-sm font-bold font-mono text-indigo-700 dark:text-indigo-300 mt-0.5">
                        {task.media_info.completed_segments} / {task.media_info.total_segments}
                      </div>
                    </div>
                  </div>

                  <div className="octoheft-subcard p-3.5 rounded-xl bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-700 space-y-2">
                    <div className="flex justify-between items-center text-xs">
                      <span className="font-semibold text-slate-800 dark:text-slate-200">
                        {t('detailModal.remuxPhase')}{' '}
                        <span className="font-mono text-indigo-700 dark:text-indigo-300 uppercase">
                          {task.media_info.mux_phase}
                        </span>
                      </span>
                      <span className="font-mono text-slate-700 dark:text-slate-200">
                        {Math.round(task.media_info.mux_percent || 0)}%
                      </span>
                    </div>

                    <div className="h-2 w-full bg-slate-200 dark:bg-slate-700 rounded-full overflow-hidden">
                      <div
                        style={{ width: `${task.media_info.mux_percent || 0}%` }}
                        className="h-full bg-gradient-to-r from-sky-600 to-indigo-600 dark:from-cyan-400 dark:to-sky-500 transition-all duration-200"
                      />
                    </div>

                    <div className="flex justify-between text-[10px] text-slate-500 dark:text-slate-400 font-mono">
                      <span>{t(isOctoMedia ? 'detailModal.octoMediaStep1' : 'detailModal.phaseStep1')}</span>
                      <span>{t(isOctoMedia ? 'detailModal.octoMediaStep2' : 'detailModal.phaseStep2')}</span>
                      <span>{t(isOctoMedia ? 'detailModal.octoMediaStep3' : 'detailModal.phaseStep3')}</span>
                      <span>{t(isOctoMedia ? 'detailModal.octoMediaStep4' : 'detailModal.phaseStep4')}</span>
                    </div>
                  </div>
                </div>
              ) : (
                <div className="octoheft-subcard p-8 text-center bg-white dark:bg-slate-900 rounded-xl border border-slate-200 dark:border-slate-700 space-y-2">
                  <Film className="h-8 w-8 text-slate-400 mx-auto" />
                  <div className="font-semibold text-slate-800 dark:text-slate-100">
                    {t('detailModal.standardBinaryTitle')}
                  </div>
                  <p className="text-[11px] text-slate-600 dark:text-slate-300 max-w-sm mx-auto">
                    {t('detailModal.standardBinaryDesc')}
                  </p>
                </div>
              )}
            </div>
          </div>
          );
        })()}

        {/* Footer Actions */}
        <div className="octoheft-subcard flex items-center justify-between border-t border-slate-200 dark:border-slate-700 px-6 py-3.5 bg-slate-50/90 dark:bg-slate-900/90">
          <div className="flex gap-2">
            <button
              type="button"
              onClick={() => onOpenFile(task.save_path)}
              disabled={!isCompleted}
              className="octoheft-subcard flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium text-slate-700 dark:text-slate-200 bg-white dark:bg-slate-800 border border-slate-200 dark:border-slate-700 hover:bg-slate-100 dark:hover:bg-slate-700 disabled:opacity-45 disabled:cursor-not-allowed transition-colors"
            >
              <ExternalLink className="h-3.5 w-3.5 text-sky-600 dark:text-cyan-400" /> {t('detailModal.openFile')}
            </button>
            <button
              type="button"
              onClick={() => onOpenFolder(task.save_path)}
              className="octoheft-subcard flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium text-slate-700 dark:text-slate-200 bg-white dark:bg-slate-800 border border-slate-200 dark:border-slate-700 hover:bg-slate-100 dark:hover:bg-slate-700 transition-colors"
            >
              <Folder className="h-3.5 w-3.5 text-amber-600 dark:text-amber-400" /> {t('detailModal.openFolder')}
            </button>
          </div>

          <div className="flex gap-2">
            {isDownloading ? (
              <button
                type="button"
                onClick={() => onPause(task.id)}
                className="flex items-center gap-1.5 px-4 py-1.5 rounded-lg text-xs font-semibold bg-amber-50 dark:bg-amber-950/70 text-amber-800 dark:text-amber-300 border border-amber-200 dark:border-amber-500/40 hover:bg-amber-100 dark:hover:bg-amber-900/70 transition-colors"
              >
                <Pause className="h-3.5 w-3.5" /> {t('detailModal.pause')}
              </button>
            ) : !isCompleted ? (
              <button
                type="button"
                onClick={() => onResume(task.id)}
                className="flex items-center gap-1.5 px-4 py-1.5 rounded-lg text-xs font-semibold bg-sky-700 text-white hover:bg-sky-800 dark:hover:bg-sky-600 border border-transparent dark:border-cyan-400/30 transition-colors"
              >
                <Play className="h-3.5 w-3.5" /> {t('detailModal.resume')}
              </button>
            ) : null}

            {!isCompleted && (
              <button
                type="button"
                onClick={() => onCancelTask(task.id)}
                className="flex items-center gap-1.5 px-3.5 py-1.5 rounded-lg text-xs font-medium text-rose-700 dark:text-rose-300 bg-rose-50 dark:bg-rose-950/70 border border-rose-200 dark:border-rose-500/40 hover:bg-rose-100 dark:hover:bg-rose-900/70 transition-colors"
              >
                <Ban className="h-3.5 w-3.5" /> {t('detailModal.cancelDownload')}
              </button>
            )}

            <button
              type="button"
              onClick={onClose}
              className="px-4 py-1.5 rounded-lg text-xs font-medium text-slate-600 dark:text-slate-300 hover:text-slate-900 dark:hover:text-white hover:bg-slate-200/60 dark:hover:bg-slate-800 transition-colors"
            >
              {t('common.close')}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
};
