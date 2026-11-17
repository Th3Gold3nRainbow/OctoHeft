import React, { useState, useEffect } from 'react';
import { ZipPreviewResult, ZipFileEntry } from '../types';
import { formatBytes } from '../utils';
import { X, FolderArchive, File, Folder, Download, Search, CheckCircle2, AlertCircle, Loader2 } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { useI18n } from '../i18n';

interface ZipPreviewModalProps {
  isOpen: boolean;
  initialUrl?: string;
  onClose: () => void;
  onDownloadFull: (url: string) => void;
}

export const ZipPreviewModal: React.FC<ZipPreviewModalProps> = ({
  isOpen,
  initialUrl,
  onClose,
  onDownloadFull,
}) => {
  const { t } = useI18n();
  const [url, setUrl] = useState(initialUrl || '');
  const [preview, setPreview] = useState<ZipPreviewResult | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [filterQuery, setFilterQuery] = useState('');
  const [extractingEntry, setExtractingEntry] = useState<string | null>(null);
  const [extractedSuccess, setExtractedSuccess] = useState<string | null>(null);

  useEffect(() => {
    if (initialUrl) {
      setUrl(initialUrl);
      fetchPreview(initialUrl);
    }
  }, [initialUrl, isOpen]);

  useEffect(() => {
    if (!isOpen) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [isOpen, onClose]);

  const fetchPreview = async (targetUrl: string) => {
    if (!targetUrl.trim()) return;
    setLoading(true);
    setError(null);
    setPreview(null);
    setExtractedSuccess(null);

    try {
      const res = await invoke<ZipPreviewResult>('preview_remote_zip', { url: targetUrl.trim() });
      setPreview(res);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  };

  const handleExtractSingle = async (entry: ZipFileEntry) => {
    if (!preview) return;
    setExtractingEntry(entry.name);
    setExtractedSuccess(null);
    try {
      const savedPath = await invoke<string>('download_zip_entry', {
        url: preview.url,
        entry,
        savePath: null,
      });
      setExtractedSuccess(t('zipModal.extractedPrefix', { path: savedPath }));
    } catch (err) {
      alert(t('zipModal.extractionFailed', { error: String(err) }));
    } finally {
      setExtractingEntry(null);
    }
  };

  if (!isOpen) return null;

  const filteredFiles = (preview?.files || []).filter((f) =>
    f.name.toLowerCase().includes(filterQuery.toLowerCase())
  );

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/35 dark:bg-black/60 backdrop-blur-sm p-4 animate-in fade-in duration-150"
      role="dialog"
      aria-modal="true"
      aria-labelledby="zip-preview-modal-title"
    >
      <div className="card-surface w-full max-w-3xl rounded-2xl border border-slate-200/90 dark:border-slate-700 bg-white/95 dark:bg-slate-900/95 backdrop-blur-xl shadow-2xl shadow-slate-900/15 dark:shadow-black/50 flex flex-col max-h-[88vh] overflow-hidden">
        {/* Header */}
        <div className="octoheft-subcard flex items-center justify-between border-b border-slate-200 dark:border-slate-700 px-6 py-4 bg-slate-50/90 dark:bg-slate-900/90">
          <div className="flex items-center gap-2.5">
            <div className="flex h-8 w-8 items-center justify-center rounded-xl bg-amber-50 dark:bg-amber-950/80 text-amber-700 dark:text-amber-300 border border-amber-200 dark:border-amber-500/40">
              <FolderArchive className="h-4 w-4" />
            </div>
            <div>
              <h2 id="zip-preview-modal-title" className="text-base font-semibold text-slate-900 dark:text-slate-100">
                {t('zipModal.title')}
              </h2>
              <p className="text-[11px] text-slate-500 dark:text-slate-400">
                {t('zipModal.subtitle')}
              </p>
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

        {/* URL Input Bar */}
        <div className="octoheft-subcard p-4 border-b border-slate-200 dark:border-slate-700 bg-slate-50/60 dark:bg-slate-900/70 flex gap-2">
          <input
            type="url"
            dir="ltr"
            autoCapitalize="none"
            spellCheck={false}
            aria-label={t('zipModal.urlPlaceholder')}
            placeholder={t('zipModal.urlPlaceholder')}
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && fetchPreview(url)}
            className="flex-1 rounded-lg bg-white dark:bg-slate-800 border border-slate-300 dark:border-slate-700 px-3 py-1.5 text-xs text-slate-900 dark:text-slate-100 font-mono placeholder-slate-400 focus:border-sky-600 dark:focus:border-cyan-400 focus:outline-none focus:ring-2 focus:ring-sky-500/20"
          />
          <button
            type="button"
            onClick={() => fetchPreview(url)}
            disabled={loading || !url.trim()}
            className="px-4 py-1.5 rounded-lg text-xs font-semibold bg-sky-700 text-white hover:bg-sky-800 dark:hover:bg-sky-600 border border-transparent dark:border-cyan-400/30 disabled:opacity-50 transition-colors flex items-center gap-1.5 shadow-sm shadow-sky-700/20"
          >
            {loading ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : null}
            <span>{t('zipModal.inspectBtn')}</span>
          </button>
        </div>

        {/* Content View */}
        <div className="flex-1 overflow-y-auto p-6 space-y-4">
          {error && (
            <div className="p-3 rounded-xl bg-rose-50 dark:bg-rose-950/70 border border-rose-200 dark:border-rose-500/40 text-rose-700 dark:text-rose-200 text-xs flex items-center gap-2">
              <AlertCircle className="h-4 w-4 shrink-0" />
              <span>{error}</span>
            </div>
          )}

          {extractedSuccess && (
            <div className="p-3 rounded-xl bg-emerald-50 dark:bg-emerald-950/70 border border-emerald-200 dark:border-emerald-500/40 text-emerald-800 dark:text-emerald-300 text-xs flex items-center gap-2">
              <CheckCircle2 className="h-4 w-4 shrink-0 text-emerald-600 dark:text-emerald-400" />
              <span className="truncate">{extractedSuccess}</span>
            </div>
          )}

          {loading ? (
            <div className="py-16 flex flex-col items-center justify-center text-center">
              <Loader2 className="h-8 w-8 animate-spin text-sky-600 dark:text-cyan-400 mb-2" />
              <p className="text-xs text-slate-800 dark:text-slate-100 font-semibold">{t('zipModal.loadingTitle')}</p>
              <p className="text-[11px] text-slate-500 dark:text-slate-400">{t('zipModal.loadingSubtitle')}</p>
            </div>
          ) : preview ? (
            <div className="space-y-4">
              {/* Archive Stats */}
              <div className="grid grid-cols-3 gap-3 text-xs">
                <div className="octoheft-subcard p-3 rounded-xl bg-slate-50 dark:bg-slate-800/80 border border-slate-200 dark:border-slate-700">
                  <span className="text-slate-500 dark:text-slate-400 block text-[10px] font-semibold uppercase">{t('zipModal.statTotalSize')}</span>
                  <span className="font-bold font-mono text-slate-900 dark:text-slate-100 text-sm">
                    {formatBytes(preview.total_archive_size)}
                  </span>
                </div>
                <div className="octoheft-subcard p-3 rounded-xl bg-slate-50 dark:bg-slate-800/80 border border-slate-200 dark:border-slate-700">
                  <span className="text-slate-500 dark:text-slate-400 block text-[10px] font-semibold uppercase">{t('zipModal.statEnclosed')}</span>
                  <span className="font-bold font-mono text-emerald-700 dark:text-emerald-300 text-sm">
                    {t('zipModal.entriesCount', { count: preview.entries_count })}
                  </span>
                </div>
                <div className="octoheft-subcard p-3 rounded-xl bg-slate-50 dark:bg-slate-800/80 border border-slate-200 dark:border-slate-700 flex items-center justify-between gap-2">
                  <div>
                    <span className="text-slate-500 dark:text-slate-400 block text-[10px] font-semibold uppercase">{t('zipModal.statTransferred')}</span>
                    <span className="accent-neon font-bold font-mono text-sky-700 dark:text-cyan-300 text-sm">&lt; 64 KB</span>
                  </div>
                  <span className="text-[10px] px-2 py-0.5 rounded-full bg-emerald-50 dark:bg-emerald-950/70 text-emerald-700 dark:text-emerald-300 border border-emerald-200 dark:border-emerald-500/40 font-semibold">
                    {t('zipModal.fractionOfFile')}
                  </span>
                </div>
              </div>

              {/* Filter Input */}
              <div className="relative">
                <Search className="absolute start-3 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-slate-400" aria-hidden="true" />
                <input
                  type="search"
                  aria-label={t('zipModal.filterPlaceholder')}
                  placeholder={t('zipModal.filterPlaceholder')}
                  value={filterQuery}
                  onChange={(e) => setFilterQuery(e.target.value)}
                  spellCheck={false}
                  className="w-full rounded-lg bg-white dark:bg-slate-800 border border-slate-300 dark:border-slate-700 ps-8 pe-3 py-1.5 text-xs text-slate-900 dark:text-slate-100 placeholder-slate-400 focus:border-sky-600 dark:focus:border-cyan-400 focus:outline-none focus:ring-2 focus:ring-sky-500/20"
                />
              </div>

              {/* Files Table */}
              <div className="octoheft-subcard rounded-xl border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 overflow-hidden max-h-72 overflow-y-auto">
                <table className="w-full text-start text-xs">
                  <thead className="sticky top-0 bg-slate-50 dark:bg-slate-800 border-b border-slate-200 dark:border-slate-700 text-[10px] uppercase text-slate-500 dark:text-slate-300 font-semibold tracking-wider">
                    <tr>
                      <th className="py-2 px-3 text-start">{t('zipModal.colFileName')}</th>
                      <th className="py-2 px-3 text-start">{t('zipModal.colSize')}</th>
                      <th className="py-2 px-3 text-start">{t('zipModal.colMethod')}</th>
                      <th className="py-2 px-3 text-end">{t('zipModal.colAction')}</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-slate-100 dark:divide-slate-800 font-mono">
                    {filteredFiles.map((f, i) => (
                      <tr key={i} className="hover:bg-sky-50/50 dark:hover:bg-slate-800/60 transition-colors">
                        <td className="py-2 px-3 max-w-xs truncate text-slate-800 dark:text-slate-200 flex items-center gap-2">
                          {f.is_directory ? (
                            <Folder className="h-3.5 w-3.5 text-amber-600 dark:text-amber-400 shrink-0" />
                          ) : (
                            <File className="h-3.5 w-3.5 text-sky-600 dark:text-cyan-400 shrink-0" />
                          )}
                          <span className="truncate" title={f.name}>
                            {f.name}
                          </span>
                        </td>
                        <td className="py-2 px-3 whitespace-nowrap text-slate-700 dark:text-slate-300">
                          {f.is_directory ? '--' : formatBytes(f.uncompressed_size)}
                        </td>
                        <td className="py-2 px-3 whitespace-nowrap text-slate-500 dark:text-slate-400 text-[11px]">
                          {f.compression_name}
                        </td>
                        <td className="py-2 px-3 whitespace-nowrap text-end">
                          {!f.is_directory && (
                            <button
                              type="button"
                              onClick={() => handleExtractSingle(f)}
                              disabled={extractingEntry === f.name}
                              className="px-2.5 py-1 rounded-lg bg-amber-50 dark:bg-amber-950/70 text-amber-800 dark:text-amber-300 border border-amber-200 dark:border-amber-500/40 hover:bg-amber-100 dark:hover:bg-amber-900/70 disabled:opacity-50 text-[11px] font-semibold transition-colors inline-flex items-center gap-1"
                            >
                              {extractingEntry === f.name ? (
                                <Loader2 className="h-3 w-3 animate-spin" />
                              ) : (
                                <Download className="h-3 w-3" />
                              )}
                              <span>{t('zipModal.extractFile')}</span>
                            </button>
                          )}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </div>
          ) : (
            <div className="py-12 text-center text-slate-500 dark:text-slate-400 text-xs">
              {t('zipModal.emptyPrompt')}
            </div>
          )}
        </div>

        {/* Footer */}
        <div className="octoheft-subcard flex items-center justify-between border-t border-slate-200 dark:border-slate-700 px-6 py-4 bg-slate-50/90 dark:bg-slate-900/90">
          <div className="text-[11px] text-slate-500 dark:text-slate-400">
            {t('zipModal.footerHint')}
          </div>

          <div className="flex gap-2">
            <button
              type="button"
              onClick={onClose}
              className="px-4 py-2 rounded-lg text-xs font-medium text-slate-600 dark:text-slate-300 hover:text-slate-900 dark:hover:text-white hover:bg-slate-200/60 dark:hover:bg-slate-800 transition-colors"
            >
              {t('common.close')}
            </button>
            {preview && (
              <button
                type="button"
                onClick={() => {
                  onDownloadFull(preview.url);
                  onClose();
                }}
                className="px-5 py-2 rounded-lg text-xs font-semibold bg-sky-700 text-white hover:bg-sky-800 dark:hover:bg-sky-600 border border-transparent dark:border-cyan-400/30 transition-colors shadow-sm shadow-sky-700/20"
              >
                {t('zipModal.downloadFull')}
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
};
