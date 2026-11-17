import React, { useEffect, useState } from 'react';
import { BrowserIntegrationReport, BrowserStatusInfo } from '../types';
import { invoke } from '@tauri-apps/api/core';
import { useI18n } from '../i18n';
import {
  X,
  Puzzle,
  CheckCircle2,
  AlertCircle,
  ExternalLink,
  FolderOpen,
  Copy,
  Check,
  Wrench,
} from 'lucide-react';

interface BrowserIntegrationModalProps {
  isOpen: boolean;
  onClose: () => void;
}

export const BrowserIntegrationModal: React.FC<BrowserIntegrationModalProps> = ({
  isOpen,
  onClose,
}) => {
  const { t } = useI18n();
  const [report, setReport] = useState<BrowserIntegrationReport | null>(null);
  const [selectedBrowserId, setSelectedBrowserId] = useState<string>('chrome');
  const [dontShowAgain, setDontShowAgain] = useState(true);
  const [copiedPath, setCopiedPath] = useState(false);
  const [statusMessage, setStatusMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const loadStatus = async () => {
    try {
      const res = await invoke<BrowserIntegrationReport>('get_browser_integration_status');
      setReport(res);
      const firstInstalled =
        res.browsers.find((b) => b.installed && b.id !== 'firefox') ||
        res.browsers.find((b) => b.installed);
      if (firstInstalled) {
        setSelectedBrowserId(firstInstalled.id);
      }
    } catch (err) {
      console.error('Failed to query browser integration status:', err);
    }
  };

  useEffect(() => {
    if (!isOpen) return;
    setCopiedPath(false);
    setStatusMessage(null);
    loadStatus();
  }, [isOpen]);

  useEffect(() => {
    if (!isOpen) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [isOpen, onClose]);

  if (!isOpen) return null;

  const handleCopyPath = async () => {
    if (!report) return;
    try {
      await invoke('copy_text_to_clipboard', { text: report.extension_dir });
      setCopiedPath(true);
      setTimeout(() => setCopiedPath(false), 2500);
    } catch {
      try {
        await navigator.clipboard.writeText(report.extension_dir);
        setCopiedPath(true);
        setTimeout(() => setCopiedPath(false), 2500);
      } catch (err) {
        alert(t('browserModal.copyError', { error: String(err) }));
      }
    }
  };

  const handleActivateExtension = async (targetBrowserId?: string) => {
    setBusy(true);
    setStatusMessage(null);
    try {
      const repaired = await invoke<BrowserIntegrationReport>('repair_browser_integration');
      setReport(repaired);

      await invoke('copy_text_to_clipboard', { text: repaired.extension_dir }).catch(() => {});
      setCopiedPath(true);

      const id = targetBrowserId || selectedBrowserId;
      await invoke('open_browser_extensions_page', { browserId: id });
      setStatusMessage(t('browserModal.statusActivated'));
    } catch (err) {
      setStatusMessage(t('browserModal.infoStatus', { error: String(err) }));
    } finally {
      setBusy(false);
    }
  };

  const handleOpenFolder = async () => {
    try {
      await invoke('open_extension_folder');
    } catch (err) {
      alert(t('browserModal.openFolderError', { error: String(err) }));
    }
  };

  const handleDismiss = async () => {
    try {
      await invoke('dismiss_browser_onboarding', { completed: dontShowAgain });
    } catch (err) {
      console.error(err);
    }
    onClose();
  };

  const browserBadge = (b: BrowserStatusInfo) => {
    if (!b.installed) {
      return (
        <span className="px-2 py-0.5 rounded-full text-[10px] font-medium bg-slate-200 dark:bg-slate-800 text-slate-600 dark:text-slate-400">
          {t('browserModal.notDetected')}
        </span>
      );
    }
    if (b.registry_registered && b.manifest_exists) {
      return (
        <span className="px-2 py-0.5 rounded-full text-[10px] font-semibold bg-emerald-50 dark:bg-emerald-950/70 text-emerald-700 dark:text-emerald-300 border border-emerald-200 dark:border-emerald-500/40 flex items-center gap-1">
          <CheckCircle2 className="h-3 w-3" /> {t('browserModal.nativeConnected')}
        </span>
      );
    }
    return (
      <span className="px-2 py-0.5 rounded-full text-[10px] font-semibold bg-amber-50 dark:bg-amber-950/70 text-amber-800 dark:text-amber-300 border border-amber-200 dark:border-amber-500/40 flex items-center gap-1">
        <AlertCircle className="h-3 w-3" /> {t('browserModal.needsRepair')}
      </span>
    );
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/35 dark:bg-black/60 backdrop-blur-sm p-4 animate-in fade-in duration-150"
      role="dialog"
      aria-modal="true"
      aria-labelledby="browser-integration-modal-title"
    >
      <div className="card-surface w-full max-w-xl rounded-2xl border border-slate-200/90 dark:border-slate-700 bg-white/95 dark:bg-slate-900/95 backdrop-blur-xl shadow-2xl shadow-slate-900/15 dark:shadow-black/50 overflow-hidden flex flex-col max-h-[90vh]">
        {/* Header */}
        <div className="octoheft-subcard flex items-center justify-between border-b border-slate-200 dark:border-slate-700 px-6 py-4 bg-slate-50/90 dark:bg-slate-900/90">
          <div className="flex items-center gap-3">
            <div className="flex h-9 w-9 items-center justify-center rounded-xl bg-sky-50 dark:bg-sky-950/80 text-sky-700 dark:text-cyan-300 border border-sky-200 dark:border-cyan-500/40">
              <Puzzle className="h-5 w-5" />
            </div>
            <div>
              <h2 id="browser-integration-modal-title" className="text-base font-semibold text-slate-900 dark:text-slate-100">
                {t('browserModal.title')}
              </h2>
              <p className="text-[11px] text-slate-500 dark:text-slate-400">
                {t('browserModal.subtitle')}
              </p>
            </div>
          </div>
          <button
            type="button"
            onClick={handleDismiss}
            aria-label={t('common.close')}
            className="rounded-lg p-1.5 text-slate-500 dark:text-slate-400 hover:bg-slate-200/70 dark:hover:bg-slate-700/70 hover:text-slate-900 dark:hover:text-white transition-colors"
          >
            <X className="h-5 w-5" />
          </button>
        </div>

        {/* Content */}
        <div className="p-6 space-y-4 overflow-y-auto text-xs">
          {/* Detected Browsers Grid */}
          <div>
            <span className="block font-semibold text-slate-800 dark:text-slate-200 mb-2">
              {t('browserModal.detectedBrowsers')}
            </span>
            <div className="grid grid-cols-2 gap-2.5">
              {(report?.browsers || []).map((b) => (
                <button
                  key={b.id}
                  type="button"
                  disabled={!b.installed}
                  onClick={() => setSelectedBrowserId(b.id)}
                  className={`p-3 rounded-xl border text-start transition-colors flex flex-col gap-1.5 ${
                    selectedBrowserId === b.id && b.installed
                      ? 'border-sky-500 dark:border-cyan-400 bg-sky-50/80 dark:bg-sky-950/70 ring-1 ring-sky-500/20 dark:ring-cyan-400/30'
                      : b.installed
                      ? 'octoheft-subcard border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-800/80 hover:border-slate-300 dark:hover:border-slate-600'
                      : 'octoheft-subcard border-slate-200 dark:border-slate-800 bg-slate-50 dark:bg-slate-900 opacity-55 cursor-not-allowed'
                  }`}
                >
                  <div className="flex items-center justify-between gap-2">
                    <span className="font-semibold text-slate-900 dark:text-slate-100">{b.name}</span>
                  </div>
                  <div className="flex items-center justify-between gap-1">
                    {browserBadge(b)}
                  </div>
                </button>
              ))}
            </div>
          </div>

          {/* 2-Step Activation Instructions */}
          <div className="octoheft-subcard p-4 rounded-xl bg-slate-50 dark:bg-slate-800/80 border border-slate-200 dark:border-slate-700 space-y-3">
            <div className="font-semibold text-sky-800 dark:text-cyan-300 flex items-center gap-1.5">
              <Wrench className="h-3.5 w-3.5" />
              <span>{t('browserModal.quickInstallTitle')}</span>
            </div>

            <ol className="list-decimal list-inside space-y-1.5 text-slate-700 dark:text-slate-300 text-[11px] leading-relaxed">
              <li>
                {t('browserModal.step1Pre')}{' '}
                <strong className="text-sky-700 dark:text-cyan-300">{t('browserModal.step1Action')}</strong>{' '}
                {t('browserModal.step1Post')}{' '}
                <code className="px-1.5 py-0.5 rounded bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-700 text-sky-700 dark:text-cyan-300 font-mono" dir="ltr">
                  chrome://extensions
                </code>
                .
              </li>
              <li>
                {t('browserModal.step2Pre')}{' '}
                <strong className="text-slate-900 dark:text-slate-100">{t('browserModal.developerMode')}</strong>
                {t('browserModal.step2Mid')}{' '}
                <strong className="text-slate-900 dark:text-slate-100">{t('browserModal.loadUnpacked')}</strong>{' '}
                {t('browserModal.step2Post')}
              </li>
            </ol>

            {/* Copyable Extension Path */}
            <div className="flex items-center gap-2">
              <input
                type="text"
                readOnly
                dir="ltr"
                aria-label={t('settings.browserExtDir')}
                value={report?.extension_dir || t('browserModal.loadingPath')}
                className="flex-1 rounded-lg bg-white dark:bg-slate-900 border border-slate-300 dark:border-slate-700 px-3 py-1.5 text-[11px] font-mono text-slate-800 dark:text-slate-200 focus:outline-none"
              />
              <button
                type="button"
                onClick={handleCopyPath}
                className="octoheft-subcard px-3 py-1.5 rounded-lg bg-white dark:bg-slate-900 border border-slate-300 dark:border-slate-700 hover:border-sky-400 dark:hover:border-cyan-400 hover:bg-sky-50 dark:hover:bg-sky-950/60 text-slate-800 dark:text-slate-200 flex items-center gap-1.5 font-medium transition-colors shrink-0"
              >
                {copiedPath ? (
                  <>
                    <Check className="h-3.5 w-3.5 text-emerald-600 dark:text-emerald-400" /> {t('browserModal.copied')}
                  </>
                ) : (
                  <>
                    <Copy className="h-3.5 w-3.5 text-slate-500 dark:text-slate-400" /> {t('browserModal.copy')}
                  </>
                )}
              </button>
            </div>
          </div>

          {statusMessage && (
            <div className="p-2.5 rounded-xl bg-emerald-50 dark:bg-emerald-950/70 border border-emerald-200 dark:border-emerald-500/40 text-emerald-800 dark:text-emerald-300 text-[11px] flex items-center gap-2">
              <CheckCircle2 className="h-4 w-4 shrink-0" />
              <span>{statusMessage}</span>
            </div>
          )}

          {/* Action Buttons */}
          <div className="flex flex-wrap items-center gap-2.5 pt-1">
            <button
              type="button"
              disabled={busy}
              onClick={() => handleActivateExtension()}
              className="flex-1 px-4 py-2.5 rounded-lg bg-sky-700 hover:bg-sky-800 dark:hover:bg-sky-600 border border-transparent dark:border-cyan-400/30 disabled:opacity-50 text-white font-semibold flex items-center justify-center gap-2 shadow-sm shadow-sky-700/20 transition-colors"
            >
              <ExternalLink className="h-4 w-4" />
              {t('browserModal.activateBtn')}
            </button>
            <button
              type="button"
              onClick={handleOpenFolder}
              className="octoheft-subcard px-4 py-2.5 rounded-lg bg-white dark:bg-slate-800 border border-slate-300 dark:border-slate-700 hover:border-slate-400 dark:hover:border-slate-600 hover:bg-slate-50 dark:hover:bg-slate-700 text-slate-800 dark:text-slate-200 font-medium flex items-center gap-2 transition-colors"
            >
              <FolderOpen className="h-4 w-4 text-amber-600 dark:text-amber-400" />
              {t('browserModal.openFolderBtn')}
            </button>
          </div>
        </div>

        {/* Footer */}
        <div className="octoheft-subcard flex items-center justify-between border-t border-slate-200 dark:border-slate-700 px-6 py-3.5 bg-slate-50/90 dark:bg-slate-900/90 text-xs">
          <div className="flex items-center gap-2 text-slate-600 dark:text-slate-300">
            <input
              id="browser-dont-show"
              type="checkbox"
              checked={dontShowAgain}
              onChange={(e) => setDontShowAgain(e.target.checked)}
              className="h-4 w-4 rounded border-slate-300 dark:border-slate-600 text-sky-600 accent-sky-600 dark:accent-cyan-400 focus:ring-sky-500 cursor-pointer"
            />
            <label htmlFor="browser-dont-show" className="cursor-pointer select-none">
              {t('browserModal.dontShowAgain')}
            </label>
          </div>
          <button
            type="button"
            onClick={handleDismiss}
            className="octoheft-subcard px-4 py-1.5 rounded-lg bg-white dark:bg-slate-800 border border-slate-300 dark:border-slate-700 hover:bg-slate-100 dark:hover:bg-slate-700 text-slate-800 dark:text-slate-200 font-medium transition-colors"
          >
            {t('common.close')}
          </button>
        </div>
      </div>
    </div>
  );
};
