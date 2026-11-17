import React, { useState, useEffect } from 'react';
import { QueueConfig } from '../types';
import { X, Layers, Sparkles, CheckCircle2, AlertCircle } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { useI18n } from '../i18n';

interface BatchDownloadModalProps {
  isOpen: boolean;
  onClose: () => void;
  queues: QueueConfig[];
  onSubmit: (urls: string[], queueId: string | null, startImmediately: boolean) => void;
}

export const BatchDownloadModal: React.FC<BatchDownloadModalProps> = ({
  isOpen,
  onClose,
  queues,
  onSubmit,
}) => {
  const { t } = useI18n();
  const [pattern, setPattern] = useState('https://example.com/files/track_[001-010].mp3');
  const [expandedUrls, setExpandedUrls] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [selectedQueue, setSelectedQueue] = useState<string>('main_queue');
  const [startImmediately, setStartImmediately] = useState(false);

  useEffect(() => {
    if (!isOpen) return;

    if (!pattern.includes('[') || !pattern.includes(']')) {
      setExpandedUrls(pattern.trim() ? [pattern.trim()] : []);
      setError(null);
      return;
    }

    const timer = setTimeout(async () => {
      try {
        const res = await invoke<string[]>('expand_batch_pattern', { pattern: pattern.trim() });
        setExpandedUrls(res);
        setError(null);
      } catch (err) {
        setError(String(err));
        setExpandedUrls([]);
      }
    }, 150);

    return () => clearTimeout(timer);
  }, [pattern, isOpen]);

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
    if (expandedUrls.length === 0) return;

    onSubmit(expandedUrls, selectedQueue === 'none' ? null : selectedQueue, startImmediately);
    onClose();
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/35 dark:bg-black/60 backdrop-blur-sm p-4 animate-in fade-in duration-150"
      role="dialog"
      aria-modal="true"
      aria-labelledby="batch-modal-title"
    >
      <div className="card-surface w-full max-w-xl rounded-2xl border border-slate-200/90 dark:border-slate-700 bg-white/95 dark:bg-slate-900/95 backdrop-blur-xl shadow-2xl shadow-slate-900/15 dark:shadow-black/50 flex flex-col max-h-[85vh] overflow-hidden">
        {/* Header */}
        <div className="octoheft-subcard flex items-center justify-between border-b border-slate-200 dark:border-slate-700 px-6 py-4 bg-slate-50/90 dark:bg-slate-900/90">
          <div className="flex items-center gap-2.5">
            <div className="flex h-8 w-8 items-center justify-center rounded-xl bg-sky-50 dark:bg-sky-950/80 text-sky-700 dark:text-cyan-300 border border-sky-200 dark:border-cyan-500/40">
              <Layers className="h-4 w-4" />
            </div>
            <div>
              <h2 id="batch-modal-title" className="text-base font-semibold text-slate-900 dark:text-slate-100">
                {t('batchModal.title')}
              </h2>
              <p className="text-[11px] text-slate-500 dark:text-slate-400">{t('batchModal.subtitle')}</p>
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

        {/* Form Body */}
        <form onSubmit={handleSubmit} className="p-6 flex-1 overflow-y-auto space-y-4">
          <div>
            <label
              htmlFor="batch-pattern-input"
              className="block text-xs font-semibold text-slate-800 dark:text-slate-200 mb-1.5 flex items-center gap-1.5"
            >
              <Sparkles className="h-3.5 w-3.5 text-sky-600 dark:text-cyan-400" /> {t('batchModal.patternLabel')}
            </label>
            <input
              id="batch-pattern-input"
              type="text"
              required
              dir="ltr"
              autoCapitalize="none"
              spellCheck={false}
              value={pattern}
              onChange={(e) => setPattern(e.target.value)}
              placeholder={t('batchModal.patternPlaceholder')}
              className="w-full rounded-lg bg-white dark:bg-slate-800 border border-slate-300 dark:border-slate-700 px-3.5 py-2 text-sm text-slate-900 dark:text-slate-100 font-mono placeholder-slate-400 focus:border-sky-600 dark:focus:border-cyan-400 focus:outline-none focus:ring-2 focus:ring-sky-500/20 dark:focus:ring-cyan-400/25"
              autoFocus
            />
            <p className="text-[11px] text-slate-500 dark:text-slate-400 mt-1">
              {t('batchModal.tipPrefix')}{' '}
              <span className="font-mono text-slate-700 dark:text-cyan-300 font-medium" dir="ltr">[001-020]</span>
              {t('batchModal.tipMiddle')}
              <span className="font-mono text-slate-700 dark:text-cyan-300 font-medium" dir="ltr">[a-f]</span>
              {t('batchModal.tipSuffix')}
            </p>
          </div>

          {error && (
            <div className="p-3 rounded-xl bg-rose-50 dark:bg-rose-950/70 border border-rose-200 dark:border-rose-500/40 text-rose-700 dark:text-rose-200 text-xs flex items-center gap-2">
              <AlertCircle className="h-4 w-4 shrink-0" />
              <span>{error}</span>
            </div>
          )}

          {/* Expanded Preview */}
          <div>
            <div className="flex items-center justify-between text-xs mb-1.5">
              <span className="font-semibold text-slate-800 dark:text-slate-200">
                {t('batchModal.previewTitle', { count: expandedUrls.length })}
              </span>
              {expandedUrls.length > 0 && (
                <span className="text-emerald-700 dark:text-emerald-300 flex items-center gap-1 font-mono text-[11px] font-semibold">
                  <CheckCircle2 className="h-3.5 w-3.5" /> {t('batchModal.validPattern')}
                </span>
              )}
            </div>
            <div className="octoheft-subcard h-36 rounded-xl bg-slate-50 dark:bg-slate-800/80 border border-slate-200 dark:border-slate-700 p-2.5 overflow-y-auto font-mono text-xs text-slate-700 dark:text-slate-200 space-y-1">
              {expandedUrls.length > 0 ? (
                expandedUrls.map((url, i) => (
                  <div key={i} className="truncate text-slate-700 dark:text-slate-200 hover:text-slate-950 dark:hover:text-white" dir="ltr">
                    <span className="text-slate-400 dark:text-slate-500 me-2">#{i + 1}</span>
                    {url}
                  </div>
                ))
              ) : (
                <div className="h-full flex items-center justify-center text-slate-400 dark:text-slate-500">
                  {t('batchModal.noUrls')}
                </div>
              )}
            </div>
          </div>

          {/* Queue & Start Options */}
          <div className="grid grid-cols-2 gap-3 pt-2">
            <div>
              <label htmlFor="batch-queue-select" className="block text-xs font-semibold text-slate-800 dark:text-slate-200 mb-1">
                {t('batchModal.assignQueue')}
              </label>
              <select
                id="batch-queue-select"
                value={selectedQueue}
                onChange={(e) => setSelectedQueue(e.target.value)}
                className="w-full rounded-lg bg-white dark:bg-slate-800 border border-slate-300 dark:border-slate-700 px-3 py-1.5 text-xs text-slate-900 dark:text-slate-100 focus:border-sky-600 dark:focus:border-cyan-400 focus:outline-none focus:ring-2 focus:ring-sky-500/20"
              >
                <option value="none">{t('batchModal.directNoQueue')}</option>
                {queues.map((q) => (
                  <option key={q.id} value={q.id}>
                    {q.name}
                  </option>
                ))}
              </select>
            </div>

            <div className="flex flex-col justify-end">
              <div className="flex items-center gap-2 text-xs text-slate-700 dark:text-slate-300 h-9">
                <input
                  id="batch-start-immediate"
                  type="checkbox"
                  checked={startImmediately}
                  onChange={(e) => setStartImmediately(e.target.checked)}
                  className="h-4 w-4 rounded border-slate-300 dark:border-slate-600 text-sky-600 accent-sky-600 dark:accent-cyan-400 focus:ring-sky-500 cursor-pointer"
                />
                <label htmlFor="batch-start-immediate" className="cursor-pointer select-none">
                  {t('batchModal.startImmediately')}
                </label>
              </div>
            </div>
          </div>

          {/* Actions */}
          <div className="pt-3 flex items-center justify-end gap-2 border-t border-slate-200 dark:border-slate-700">
            <button
              type="button"
              onClick={onClose}
              className="px-4 py-2 rounded-lg text-xs font-medium text-slate-600 dark:text-slate-300 hover:text-slate-900 dark:hover:text-white hover:bg-slate-100 dark:hover:bg-slate-800 transition-colors"
            >
              {t('common.cancel')}
            </button>
            <button
              type="submit"
              disabled={expandedUrls.length === 0}
              className="px-5 py-2 rounded-lg text-xs font-semibold bg-sky-700 text-white hover:bg-sky-800 dark:hover:bg-sky-600 border border-transparent dark:border-cyan-400/30 disabled:opacity-50 disabled:cursor-not-allowed transition-colors shadow-sm shadow-sky-700/20"
            >
              {t('batchModal.submit', { count: expandedUrls.length })}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
};
