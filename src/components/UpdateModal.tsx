import React, { useState, useEffect } from 'react';
import { UpdateInfo, UpdateProgressPayload } from '../types';
import {
  X,
  Sparkles,
  Download,
  CheckCircle2,
  AlertCircle,
  ExternalLink,
  Loader2,
  ArrowRight,
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useI18n } from '../i18n';
import { formatBytes } from '../utils';

interface UpdateModalProps {
  isOpen: boolean;
  onClose: () => void;
  updateInfo: UpdateInfo | null;
}

export const UpdateModal: React.FC<UpdateModalProps> = ({
  isOpen,
  onClose,
  updateInfo,
}) => {
  const { t, language } = useI18n();
  const [isDownloading, setIsDownloading] = useState(false);
  const [isReady, setIsReady] = useState(false);
  const [progress, setProgress] = useState<UpdateProgressPayload | null>(null);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);

  useEffect(() => {
    if (!isOpen) {
      setIsDownloading(false);
      setIsReady(false);
      setProgress(null);
      setErrorMessage(null);
      return;
    }

    let unlisten: (() => void) | null = null;
    listen<UpdateProgressPayload>('update://progress', (event) => {
      const payload = event.payload;
      if (!payload) return;
      setProgress(payload);

      if (payload.status === 'downloading') {
        setIsDownloading(true);
      } else if (payload.status === 'ready') {
        setIsDownloading(false);
        setIsReady(true);
      } else if (payload.status === 'error') {
        setIsDownloading(false);
        setErrorMessage(payload.error_message || t('update.error', { error: 'Unknown' }));
      }
    }).then((unsub) => {
      unlisten = unsub;
    }).catch(console.error);

    return () => {
      if (unlisten) unlisten();
    };
  }, [isOpen, t]);

  useEffect(() => {
    if (!isOpen) return;
    const onKeyDown = (e: KeyboardEvent) => {
      // Disallow escape while downloading
      if (e.key === 'Escape' && !isDownloading) {
        onClose();
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [isOpen, isDownloading, onClose]);

  if (!isOpen || !updateInfo) return null;

  const handleStartUpdate = async () => {
    const url = updateInfo.download_url;
    if (!url) {
      // If no direct binary attached, open GitHub release page in browser
      try {
        await invoke('open_browser_extensions_page'); // or shell open
      } catch {
        window.open('https://github.com/Th3Gold3nRainbow/OctoHeft/releases/latest', '_blank');
      }
      return;
    }

    setIsDownloading(true);
    setErrorMessage(null);
    setProgress({
      downloaded_bytes: 0,
      total_bytes: null,
      percent: 0,
      status: 'downloading',
      error_message: null,
    });

    try {
      await invoke('install_app_update', { downloadUrl: url });
    } catch (err) {
      setIsDownloading(false);
      setErrorMessage(String(err));
    }
  };

  const handleManualLaunch = async () => {
    try {
      await invoke('launch_app_update');
    } catch (err) {
      setErrorMessage(String(err));
    }
  };

  const percentClamped = Math.min(100, Math.max(0, Math.round(progress?.percent || 0)));

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/40 dark:bg-black/60 backdrop-blur-sm p-4 animate-in fade-in duration-150"
      role="dialog"
      aria-modal="true"
      aria-labelledby="update-modal-title"
    >
      <div className="card-surface w-full max-w-lg rounded-2xl border border-slate-200/90 dark:border-slate-700 bg-white/95 dark:bg-slate-900/95 backdrop-blur-xl shadow-2xl shadow-slate-900/15 dark:shadow-black/50 flex flex-col max-h-[85vh] overflow-hidden">
        {/* Header */}
        <div className="octoheft-subcard flex items-center justify-between border-b border-slate-200 dark:border-slate-700 px-6 py-4 bg-slate-50/90 dark:bg-slate-900/90">
          <div className="flex items-center gap-2.5">
            <div className="flex h-9 w-9 items-center justify-center rounded-xl bg-gradient-to-br from-sky-500 to-cyan-500 text-white shadow-md shadow-sky-500/20">
              <Sparkles className="h-4 w-4" />
            </div>
            <div>
              <h2 id="update-modal-title" className="text-base font-semibold text-slate-900 dark:text-slate-100 flex items-center gap-2">
                <span>{t('update.modalTitle')}</span>
                <span className="text-xs px-2 py-0.5 rounded-full font-mono font-bold bg-cyan-100 dark:bg-cyan-950/80 text-cyan-800 dark:text-cyan-300 border border-cyan-300 dark:border-cyan-500/40">
                  v{updateInfo.latest_version}
                </span>
              </h2>
              <p className="text-[11px] text-slate-500 dark:text-slate-400">
                {t('update.modalSubtitle')}
              </p>
            </div>
          </div>
          <button
            type="button"
            disabled={isDownloading}
            onClick={onClose}
            aria-label={t('common.close')}
            className={`rounded-lg p-1.5 transition-colors ${
              isDownloading
                ? 'opacity-30 cursor-not-allowed text-slate-400'
                : 'text-slate-500 dark:text-slate-400 hover:bg-slate-200/70 dark:hover:bg-slate-700/70 hover:text-slate-900 dark:hover:text-white'
            }`}
          >
            <X className="h-5 w-5" />
          </button>
        </div>

        {/* Modal Body */}
        <div className="p-6 flex-1 overflow-y-auto space-y-4 text-xs">
          {/* Version Comparison Card */}
          <div className="grid grid-cols-2 gap-3">
            <div className="octoheft-subcard p-3 rounded-xl bg-slate-50 dark:bg-slate-800/80 border border-slate-200 dark:border-slate-700">
              <span className="text-[11px] text-slate-500 dark:text-slate-400 block mb-1">
                {t('update.currentVersion')}
              </span>
              <span className="font-mono text-sm font-semibold text-slate-700 dark:text-slate-300">
                v{updateInfo.current_version}
              </span>
            </div>

            <div className="octoheft-subcard p-3 rounded-xl bg-sky-50/70 dark:bg-sky-950/50 border border-sky-300 dark:border-cyan-500/40">
              <span className="text-[11px] text-sky-700 dark:text-cyan-300 font-medium block mb-1">
                {t('update.latestVersion')}
              </span>
              <div className="flex items-center gap-1.5 font-mono text-sm font-bold text-sky-800 dark:text-cyan-200">
                <span>v{updateInfo.latest_version}</span>
                <span className="h-2 w-2 rounded-full bg-cyan-400 animate-ping inline-block" />
              </div>
            </div>
          </div>

          {/* Release Notes */}
          {updateInfo.release_notes && (
            <div className="space-y-1.5">
              <label className="font-semibold text-slate-800 dark:text-slate-200 block">
                {t('update.releaseNotes')}
              </label>
              <div className="octoheft-subcard max-h-36 overflow-y-auto rounded-xl p-3 bg-slate-50 dark:bg-slate-800/80 border border-slate-200 dark:border-slate-700 font-sans text-slate-700 dark:text-slate-300 leading-relaxed whitespace-pre-wrap select-text">
                {updateInfo.release_notes}
              </div>
            </div>
          )}

          {/* Download Progress State */}
          {(isDownloading || isReady) && (
            <div className="space-y-2 pt-2">
              <div className="flex items-center justify-between font-medium">
                <span className="text-slate-700 dark:text-slate-300 flex items-center gap-1.5">
                  {isDownloading ? (
                    <>
                      <Loader2 className="h-3.5 w-3.5 animate-spin text-sky-600 dark:text-cyan-400" />
                      <span>{t('update.downloading', { percent: percentClamped })}</span>
                    </>
                  ) : (
                    <>
                      <CheckCircle2 className="h-3.5 w-3.5 text-emerald-500 dark:text-emerald-400" />
                      <span className="text-emerald-600 dark:text-emerald-400 font-semibold">
                        {t('update.launching')}
                      </span>
                    </>
                  )}
                </span>
                <span className="font-mono text-slate-600 dark:text-slate-400">
                  {progress?.total_bytes
                    ? `${formatBytes(progress.downloaded_bytes, 1, language)} / ${formatBytes(progress.total_bytes, 1, language)}`
                    : `${percentClamped}%`}
                </span>
              </div>

              {/* Progress Bar Track */}
              <div className="w-full h-2 bg-slate-200 dark:bg-slate-700 rounded-full overflow-hidden">
                <div
                  className="h-full bg-gradient-to-r from-sky-500 to-cyan-400 transition-all duration-200"
                  style={{ width: `${percentClamped}%` }}
                />
              </div>
            </div>
          )}

          {/* Ready Confirmation Banner */}
          {isReady && (
            <div className="p-3.5 rounded-xl bg-emerald-50 dark:bg-emerald-950/70 border border-emerald-300 dark:border-emerald-500/40 text-emerald-800 dark:text-emerald-200 flex items-start gap-2.5 animate-in fade-in duration-200">
              <CheckCircle2 className="h-4 w-4 mt-0.5 text-emerald-600 dark:text-emerald-400 shrink-0" />
              <div className="space-y-1">
                <div className="font-semibold text-xs">{t('update.readyPrompt')}</div>
                <button
                  type="button"
                  onClick={handleManualLaunch}
                  className="inline-flex items-center gap-1 text-[11px] underline font-medium hover:text-emerald-900 dark:hover:text-emerald-100"
                >
                  <span>{t('update.launching')}</span>
                  <ArrowRight className="h-3 w-3" />
                </button>
              </div>
            </div>
          )}

          {/* Error Banner */}
          {errorMessage && (
            <div className="p-3 rounded-xl bg-rose-50 dark:bg-rose-950/70 border border-rose-200 dark:border-rose-500/40 text-rose-700 dark:text-rose-200 text-xs flex items-center justify-between gap-2">
              <div className="flex items-center gap-2">
                <AlertCircle className="h-4 w-4 shrink-0" />
                <span>{errorMessage}</span>
              </div>
              <button
                type="button"
                onClick={handleStartUpdate}
                className="px-2 py-1 rounded bg-rose-600 text-white font-medium hover:bg-rose-700 transition-colors"
              >
                {t('update.retry')}
              </button>
            </div>
          )}
        </div>

        {/* Modal Footer */}
        <div className="octoheft-subcard border-t border-slate-200 dark:border-slate-700 px-6 py-3.5 bg-slate-50/90 dark:bg-slate-900/90 flex items-center justify-between gap-3">
          <a
            href="https://github.com/Th3Gold3nRainbow/OctoHeft/releases/latest"
            target="_blank"
            rel="noreferrer"
            className="text-slate-600 dark:text-slate-400 hover:text-slate-900 dark:hover:text-white flex items-center gap-1.5 transition-colors text-xs"
          >
            <ExternalLink className="h-3.5 w-3.5" />
            <span>{t('update.viewOnGitHub')}</span>
          </a>

          <div className="flex items-center gap-2">
            {!isReady && (
              <button
                type="button"
                disabled={isDownloading}
                onClick={onClose}
                className="px-3.5 py-2 rounded-lg font-medium text-slate-700 dark:text-slate-300 hover:bg-slate-200/70 dark:hover:bg-slate-800 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
              >
                {t('update.cancel')}
              </button>
            )}

            {!isReady && (
              <button
                type="button"
                disabled={isDownloading}
                onClick={handleStartUpdate}
                className="px-4 py-2 rounded-lg bg-sky-600 hover:bg-sky-500 dark:bg-cyan-500 dark:hover:bg-cyan-400 text-white font-semibold flex items-center gap-1.5 shadow-md shadow-sky-600/25 dark:shadow-cyan-500/25 transition-all disabled:opacity-50 disabled:cursor-not-allowed"
              >
                {isDownloading ? (
                  <>
                    <Loader2 className="h-3.5 w-3.5 animate-spin" />
                    <span>{t('update.downloading', { percent: percentClamped })}</span>
                  </>
                ) : (
                  <>
                    <Download className="h-3.5 w-3.5" />
                    <span>{t('update.downloadAndInstall', { version: `v${updateInfo.latest_version}` })}</span>
                  </>
                )}
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
};
