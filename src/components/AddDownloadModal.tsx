import React, { useEffect, useState } from 'react';
import { AddDownloadRequest } from '../types';
import {
  X,
  Download,
  Sliders,
  Globe,
  FolderArchive,
  Film,
  Folder,
  ClipboardCheck,
} from 'lucide-react';
import { useI18n } from '../i18n';

interface AddDownloadModalProps {
  isOpen: boolean;
  initialUrl?: string;
  initialFileName?: string;
  defaultDownloadDir?: string;
  defaultConnections?: number;
  capturedFromClipboard?: boolean;
  isMediaUrl?: boolean;
  onClose: () => void;
  onSubmit: (req: AddDownloadRequest, startImmediately: boolean) => void;
  onInspectZip?: (url: string) => void;
  onStartMediaStream?: (url: string) => void;
  onOpenInOctoMedia?: (url: string) => void;
}

export const AddDownloadModal: React.FC<AddDownloadModalProps> = ({
  isOpen,
  initialUrl,
  initialFileName,
  defaultDownloadDir,
  defaultConnections = 8,
  capturedFromClipboard = false,
  isMediaUrl = false,
  onClose,
  onSubmit,
  onInspectZip,
  onStartMediaStream,
  onOpenInOctoMedia,
}) => {
  const { t } = useI18n();
  const [url, setUrl] = useState('');
  const [fileName, setFileName] = useState('');
  const [connections, setConnections] = useState(defaultConnections);
  const [startImmediately, setStartImmediately] = useState(true);

  useEffect(() => {
    if (isOpen) {
      if (initialUrl !== undefined) {
        setUrl(initialUrl);
      }
      if (initialFileName !== undefined) {
        setFileName(initialFileName);
      }
      setConnections(defaultConnections);
    } else {
      setUrl('');
      setFileName('');
    }
  }, [isOpen, initialUrl, initialFileName, defaultConnections]);

  useEffect(() => {
    if (!isOpen) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [isOpen, onClose]);

  if (!isOpen) return null;

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!url.trim()) return;

    onSubmit(
      {
        url: url.trim(),
        file_name: fileName.trim() || undefined,
        connections,
      },
      startImmediately
    );

    setUrl('');
    setFileName('');
    onClose();
  };

  const trimmedLowerUrl = url.trim().toLowerCase();
  const isDirectStreamManifest =
    trimmedLowerUrl.includes('.m3u8') || trimmedLowerUrl.includes('.mpd');
  const showsOctoMediaOffer =
    Boolean(onOpenInOctoMedia) &&
    !isDirectStreamManifest &&
    (isMediaUrl ||
      /(?:youtube\.com|youtu\.be|vimeo\.com|twitch\.tv|dailymotion\.com|tiktok\.com)/i.test(
        url.trim()
      ));

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/35 dark:bg-black/60 backdrop-blur-sm p-4"
      role="dialog"
      aria-modal="true"
      aria-labelledby="add-download-modal-title"
    >
      <div className="card-surface w-full max-w-lg rounded-2xl border border-slate-200/90 dark:border-slate-700 bg-white/95 dark:bg-slate-900/95 backdrop-blur-xl shadow-2xl shadow-slate-900/15 dark:shadow-black/50 flex flex-col overflow-hidden animate-in fade-in zoom-in-95 duration-150">
        <div className="octoheft-subcard flex items-center justify-between border-b border-slate-200 dark:border-slate-700 px-6 py-4 bg-slate-50/90 dark:bg-slate-900/90">
          <div className="flex items-center gap-2.5 min-w-0">
            <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-xl bg-sky-50 dark:bg-sky-950/80 text-sky-700 dark:text-cyan-300 border border-sky-200 dark:border-cyan-500/40">
              <Download className="h-4 w-4" />
            </div>
            <div className="flex flex-wrap items-center gap-2 min-w-0">
              <h2 id="add-download-modal-title" className="text-base font-semibold text-slate-900 dark:text-slate-100">
                {t('addModal.title')}
              </h2>
              {capturedFromClipboard && (
                <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-[10px] font-semibold bg-cyan-50 dark:bg-cyan-950/80 text-cyan-800 dark:text-cyan-300 border border-cyan-200 dark:border-cyan-500/40">
                  <ClipboardCheck className="h-3 w-3 shrink-0" aria-hidden="true" />
                  <span>{t('addModal.clipboardCapturedBadge')}</span>
                </span>
              )}
            </div>
          </div>
          <button
            type="button"
            onClick={onClose}
            aria-label={t('common.close')}
            className="rounded-lg p-1.5 text-slate-500 dark:text-slate-400 hover:bg-slate-200/70 dark:hover:bg-slate-700/70 hover:text-slate-900 dark:hover:text-white transition-colors"
          >
            <X className="h-5 w-5" />
          </button>
        </div>

        <form onSubmit={handleSubmit} className="p-6 space-y-4">
          <div>
            <label
              htmlFor="add-download-url"
              className="block text-xs font-semibold text-slate-800 dark:text-slate-200 mb-1.5 flex items-center gap-1.5"
            >
              <Globe className="h-3.5 w-3.5 text-sky-600 dark:text-cyan-400" /> {t('addModal.urlLabel')}
            </label>
            <input
              id="add-download-url"
              type="url"
              required
              autoCapitalize="none"
              spellCheck={false}
              dir="ltr"
              placeholder={t('addModal.urlPlaceholder')}
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              className="w-full rounded-lg bg-white dark:bg-slate-800 border border-slate-300 dark:border-slate-700 px-3.5 py-2 text-sm text-slate-900 dark:text-slate-100 placeholder-slate-400 focus:border-sky-600 dark:focus:border-cyan-400 focus:outline-none focus:ring-2 focus:ring-sky-500/20 dark:focus:ring-cyan-400/25"
              autoFocus
            />

            {url.toLowerCase().includes('.zip') && onInspectZip && (
              <div className="mt-2 p-2.5 rounded-xl bg-amber-50 dark:bg-amber-950/70 border border-amber-200 dark:border-amber-500/40 flex items-center justify-between gap-2 text-xs text-amber-900 dark:text-amber-200">
                <div className="flex items-center gap-2">
                  <FolderArchive className="h-4 w-4 shrink-0 text-amber-600 dark:text-amber-400" />
                  <span>{t('addModal.zipDetected')}</span>
                </div>
                <button
                  type="button"
                  onClick={() => onInspectZip(url.trim())}
                  className="px-2.5 py-1 rounded-lg bg-white dark:bg-slate-800 hover:bg-amber-100 dark:hover:bg-amber-900/60 text-amber-900 dark:text-amber-200 border border-amber-300 dark:border-amber-500/40 font-semibold transition-colors shrink-0"
                >
                  {t('addModal.previewZip')}
                </button>
              </div>
            )}

            {isDirectStreamManifest && onStartMediaStream && (
              <div className="mt-2 p-2.5 rounded-xl bg-indigo-50 dark:bg-indigo-950/70 border border-indigo-200 dark:border-indigo-500/40 flex items-center justify-between gap-2 text-xs text-indigo-900 dark:text-indigo-200">
                <div className="flex items-center gap-2">
                  <Film className="h-4 w-4 shrink-0 text-indigo-600 dark:text-indigo-400" />
                  <span>{t('addModal.mediaDetected')}</span>
                </div>
                <button
                  type="button"
                  onClick={() => {
                    onStartMediaStream(url.trim());
                    setUrl('');
                    setFileName('');
                    onClose();
                  }}
                  className="px-2.5 py-1 rounded-lg bg-white dark:bg-slate-800 hover:bg-indigo-100 dark:hover:bg-indigo-900/60 text-indigo-900 dark:text-indigo-200 border border-indigo-300 dark:border-indigo-500/40 font-semibold transition-colors shrink-0"
                >
                  {t('addModal.nativeRemux')}
                </button>
              </div>
            )}

            {showsOctoMediaOffer && onOpenInOctoMedia && (
              <div className="mt-2 p-2.5 rounded-xl bg-sky-50 dark:bg-sky-950/70 border border-sky-200 dark:border-cyan-500/40 flex items-center justify-between gap-2 text-xs text-sky-900 dark:text-cyan-200">
                <div className="flex items-center gap-2 min-w-0">
                  <Film className="h-4 w-4 shrink-0 text-sky-600 dark:text-cyan-400" />
                  <span className="truncate">{t('addModal.octoMediaPageDetected')}</span>
                </div>
                <button
                  type="button"
                  onClick={() => {
                    const target = url.trim();
                    setUrl('');
                    setFileName('');
                    onOpenInOctoMedia(target);
                  }}
                  className="px-2.5 py-1 rounded-lg bg-white dark:bg-slate-800 hover:bg-sky-100 dark:hover:bg-cyan-950/80 text-sky-800 dark:text-cyan-200 border border-sky-300 dark:border-cyan-500/40 font-semibold transition-colors shrink-0"
                >
                  {t('addModal.openOctoMedia')}
                </button>
              </div>
            )}
          </div>

          {defaultDownloadDir && defaultDownloadDir.trim() && (
            <div className="octoheft-subcard flex items-center gap-2 rounded-lg bg-slate-50/90 dark:bg-slate-800/80 border border-slate-200/80 dark:border-slate-700 px-3 py-2 text-[11px] text-slate-600 dark:text-slate-300">
              <Folder className="h-3.5 w-3.5 shrink-0 text-amber-600 dark:text-amber-400" aria-hidden="true" />
              <span className="font-semibold text-slate-700 dark:text-slate-200 shrink-0">
                {t('addModal.saveDirLabel')}:
              </span>
              <code className="font-mono text-slate-700 dark:text-slate-200 truncate" dir="ltr" title={defaultDownloadDir}>
                {defaultDownloadDir}
              </code>
            </div>
          )}

          <div>
            <label htmlFor="add-download-filename" className="block text-xs font-semibold text-slate-800 dark:text-slate-200 mb-1.5">
              {t('addModal.fileNameLabel')}
            </label>
            <input
              id="add-download-filename"
              type="text"
              spellCheck={false}
              placeholder={t('addModal.fileNamePlaceholder')}
              value={fileName}
              onChange={(e) => setFileName(e.target.value)}
              className="w-full rounded-lg bg-white dark:bg-slate-800 border border-slate-300 dark:border-slate-700 px-3.5 py-2 text-sm text-slate-900 dark:text-slate-100 placeholder-slate-400 focus:border-sky-600 dark:focus:border-cyan-400 focus:outline-none focus:ring-2 focus:ring-sky-500/20 dark:focus:ring-cyan-400/25"
            />
          </div>

          <div>
            <div className="flex items-center justify-between mb-1.5">
              <label htmlFor="add-download-connections" className="text-xs font-semibold text-slate-800 dark:text-slate-200 flex items-center gap-1.5">
                <Sliders className="h-3.5 w-3.5 text-sky-600 dark:text-cyan-400" /> {t('addModal.connectionsLabel')}
              </label>
              <span className="accent-neon text-xs font-mono font-bold text-sky-700 dark:text-cyan-300">
                {t('addModal.threadsCount', { count: connections })}
              </span>
            </div>
            <input
              id="add-download-connections"
              type="range"
              min="1"
              max="32"
              step="1"
              value={connections}
              onChange={(e) => setConnections(parseInt(e.target.value, 10))}
              className="w-full h-1.5 bg-slate-200 dark:bg-slate-700 rounded-lg appearance-none cursor-pointer accent-sky-600 dark:accent-cyan-400"
            />
            <div className="flex justify-between text-[10px] text-slate-500 dark:text-slate-400 mt-1">
              <span>{t('addModal.singleScale')}</span>
              <span>{t('addModal.optimalScale')}</span>
              <span>{t('addModal.turboScale')}</span>
              <span>{t('addModal.maxScale')}</span>
            </div>
          </div>

          <div className="pt-3 flex items-center justify-between border-t border-slate-200 dark:border-slate-700">
            <div className="flex items-center gap-2 text-xs text-slate-700 dark:text-slate-300">
              <input
                id="add-download-immediate"
                type="checkbox"
                checked={startImmediately}
                onChange={(e) => setStartImmediately(e.target.checked)}
                className="h-4 w-4 rounded border-slate-300 dark:border-slate-600 text-sky-600 accent-sky-600 dark:accent-cyan-400 focus:ring-sky-500 cursor-pointer"
              />
              <label htmlFor="add-download-immediate" className="cursor-pointer select-none">
                {t('addModal.startImmediately')}
              </label>
            </div>

            <div className="flex gap-2">
              <button
                type="button"
                onClick={onClose}
                className="px-4 py-2 rounded-lg text-xs font-medium text-slate-600 dark:text-slate-300 hover:text-slate-900 dark:hover:text-white hover:bg-slate-100 dark:hover:bg-slate-800 transition-colors"
              >
                {t('common.cancel')}
              </button>
              <button
                type="submit"
                className="px-5 py-2 rounded-lg text-xs font-semibold bg-sky-700 text-white hover:bg-sky-800 dark:hover:bg-sky-600 border border-transparent dark:border-cyan-400/30 transition-colors shadow-sm shadow-sky-700/20"
              >
                {t('addModal.submit')}
              </button>
            </div>
          </div>
        </form>
      </div>
    </div>
  );
};
