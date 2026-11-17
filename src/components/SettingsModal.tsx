import React, { useState, useEffect } from 'react';
import { AppSettings, BrowserIntegrationReport } from '../types';
import {
  useI18n,
  SUPPORTED_LANGUAGES,
  LanguageCode,
} from '../i18n';
import { playCompletionChime } from '../utils';
import {
  X,
  Sliders,
  CheckCircle2,
  AlertCircle,
  RotateCcw,
  Puzzle,
  Wrench,
  FolderOpen,
  Languages,
  Volume2,
  FileType,
  Folder,
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';

interface SettingsModalProps {
  isOpen: boolean;
  onClose: () => void;
  onSettingsSaved?: (settings: AppSettings) => void;
  onOpenBrowserWizard?: () => void;
}

export const SettingsModal: React.FC<SettingsModalProps> = ({
  isOpen,
  onClose,
  onSettingsSaved,
  onOpenBrowserWizard,
}) => {
  const { t, language, setLanguage } = useI18n();
  const [activeTab, setActiveTab] = useState<'general' | 'fileTypes' | 'saveTo' | 'browser'>(
    'general'
  );
  const [draft, setDraft] = useState<AppSettings>({
    language: language || 'en',
    intercept_extensions: '',
    sound_on_complete: true,
    default_download_dir: '',
    use_category_subfolders: true,
  });
  const [browserReport, setBrowserReport] = useState<BrowserIntegrationReport | null>(null);
  const [browserActionMessage, setBrowserActionMessage] = useState<string | null>(null);
  const [savedSuccess, setSavedSuccess] = useState(false);

  const loadBrowserReport = () => {
    invoke<BrowserIntegrationReport>('get_browser_integration_status')
      .then(setBrowserReport)
      .catch(console.error);
  };

  useEffect(() => {
    if (!isOpen) return;
    setSavedSuccess(false);
    setBrowserActionMessage(null);
    invoke<AppSettings>('get_app_settings')
      .then((res) => {
        if (res) {
          setDraft(res);
        }
      })
      .catch(console.error);
    loadBrowserReport();
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

  const handleLanguageChange = async (next: LanguageCode) => {
    setDraft((prev) => ({ ...prev, language: next }));
    await setLanguage(next);
  };

  const handleRestoreDefaultExtensions = async () => {
    try {
      const defaults = await invoke<string>('get_default_intercept_extensions');
      setDraft((prev) => ({ ...prev, intercept_extensions: defaults }));
    } catch (err) {
      console.error(err);
    }
  };

  const handleBrowseFolder = async () => {
    try {
      const picked = await invoke<string | null>('pick_download_folder', {
        title: t('settings.defaultFolderLabel'),
      });
      if (picked) {
        setDraft((prev) => ({ ...prev, default_download_dir: picked }));
      }
    } catch (err) {
      alert(t('settings.pickFolderError', { error: String(err) }));
    }
  };

  const handleRepairBrowserKeys = async () => {
    try {
      const updated = await invoke<BrowserIntegrationReport>('repair_browser_integration');
      setBrowserReport(updated);
      setBrowserActionMessage(t('settings.browserRepairedMsg'));
    } catch (err) {
      alert(t('settings.browserRepairError', { error: String(err) }));
    }
  };

  const handleOpenExtensionFolder = async () => {
    try {
      await invoke('open_extension_folder');
    } catch (err) {
      alert(t('browserModal.openFolderError', { error: String(err) }));
    }
  };

  const handleSave = async (e: React.FormEvent) => {
    e.preventDefault();
    try {
      const saved = await invoke<AppSettings>('save_app_settings', {
        settings: draft,
      });
      setDraft(saved);
      if (onSettingsSaved) {
        onSettingsSaved(saved);
      }
      setSavedSuccess(true);
      setTimeout(() => {
        setSavedSuccess(false);
        onClose();
      }, 650);
    } catch (err) {
      alert(t('settings.saveError', { error: String(err) }));
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/35 dark:bg-black/60 backdrop-blur-sm p-4"
      role="dialog"
      aria-modal="true"
      aria-labelledby="settings-modal-title"
    >
      <div className="card-surface w-full max-w-2xl rounded-2xl border border-slate-200/90 dark:border-slate-700 bg-white/95 dark:bg-slate-900/95 backdrop-blur-xl shadow-2xl shadow-slate-900/15 dark:shadow-black/50 flex flex-col max-h-[88vh] overflow-hidden">
        {/* Header */}
        <div className="octoheft-subcard flex items-center justify-between border-b border-slate-200 dark:border-slate-700 px-6 py-4 bg-slate-50/90 dark:bg-slate-900/90">
          <div className="flex items-center gap-2.5">
            <div className="flex h-9 w-9 items-center justify-center rounded-xl bg-sky-50 dark:bg-sky-950/80 text-sky-700 dark:text-cyan-300 border border-sky-200 dark:border-cyan-500/40">
              <Sliders className="h-4 w-4" />
            </div>
            <div>
              <h2 id="settings-modal-title" className="text-base font-semibold text-slate-900 dark:text-slate-100">
                {t('settings.title')}
              </h2>
              <p className="text-[11px] text-slate-500 dark:text-slate-400">{t('settings.subtitle')}</p>
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

        {/* Simplified 4-Tab Navigation */}
        <div className="octoheft-subcard flex border-b border-slate-200 dark:border-slate-700 bg-slate-50/60 dark:bg-slate-900/70 px-6 gap-1 text-xs overflow-x-auto">
          <button
            type="button"
            onClick={() => setActiveTab('general')}
            className={`py-3 px-3 font-medium border-b-2 flex items-center gap-1.5 whitespace-nowrap transition-colors ${
              activeTab === 'general'
                ? 'border-sky-600 dark:border-cyan-400 text-sky-700 dark:text-cyan-300 font-semibold'
                : 'border-transparent text-slate-600 dark:text-slate-300 hover:text-slate-900 dark:hover:text-white'
            }`}
          >
            <Sliders className="h-3.5 w-3.5" /> {t('settings.tabGeneral')}
          </button>
          <button
            type="button"
            onClick={() => setActiveTab('fileTypes')}
            className={`py-3 px-3 font-medium border-b-2 flex items-center gap-1.5 whitespace-nowrap transition-colors ${
              activeTab === 'fileTypes'
                ? 'border-sky-600 dark:border-cyan-400 text-sky-700 dark:text-cyan-300 font-semibold'
                : 'border-transparent text-slate-600 dark:text-slate-300 hover:text-slate-900 dark:hover:text-white'
            }`}
          >
            <FileType className="h-3.5 w-3.5" /> {t('settings.tabFileTypes')}
          </button>
          <button
            type="button"
            onClick={() => setActiveTab('saveTo')}
            className={`py-3 px-3 font-medium border-b-2 flex items-center gap-1.5 whitespace-nowrap transition-colors ${
              activeTab === 'saveTo'
                ? 'border-sky-600 dark:border-cyan-400 text-sky-700 dark:text-cyan-300 font-semibold'
                : 'border-transparent text-slate-600 dark:text-slate-300 hover:text-slate-900 dark:hover:text-white'
            }`}
          >
            <Folder className="h-3.5 w-3.5" /> {t('settings.tabSaveTo')}
          </button>
          <button
            type="button"
            onClick={() => {
              setActiveTab('browser');
              loadBrowserReport();
            }}
            className={`py-3 px-3 font-medium border-b-2 flex items-center gap-1.5 whitespace-nowrap transition-colors ${
              activeTab === 'browser'
                ? 'border-sky-600 dark:border-cyan-400 text-sky-700 dark:text-cyan-300 font-semibold'
                : 'border-transparent text-slate-600 dark:text-slate-300 hover:text-slate-900 dark:hover:text-white'
            }`}
          >
            <Puzzle className="h-3.5 w-3.5" /> {t('settings.tabBrowser')}
          </button>
        </div>

        {/* Tab Contents */}
        <form onSubmit={handleSave} className="flex-1 overflow-y-auto p-6 space-y-5 text-xs">
          {/* Tab 1: General */}
          {activeTab === 'general' && (
            <div className="space-y-4">
              {/* Language Selector */}
              <div className="octoheft-subcard p-4 rounded-xl bg-slate-50 dark:bg-slate-800/80 border border-slate-200 dark:border-slate-700 space-y-2">
                <label
                  htmlFor="language-select"
                  className="font-semibold text-slate-800 dark:text-slate-200 flex items-center gap-2"
                >
                  <Languages className="h-4 w-4 text-sky-600 dark:text-cyan-400" />
                  <span>{t('settings.languageLabel')}</span>
                </label>
                <select
                  id="language-select"
                  value={language}
                  onChange={(e) => handleLanguageChange(e.target.value as LanguageCode)}
                  className="w-full rounded-lg bg-white dark:bg-slate-900 border border-slate-300 dark:border-slate-700 px-3 py-2 text-xs text-slate-900 dark:text-slate-100 focus:border-sky-600 dark:focus:border-cyan-400 focus:outline-none focus:ring-2 focus:ring-sky-500/20"
                >
                  {SUPPORTED_LANGUAGES.map((opt) => (
                    <option key={opt.code} value={opt.code}>
                      {opt.nativeName} ({opt.englishName})
                    </option>
                  ))}
                </select>
                <p className="text-[11px] text-slate-600 dark:text-slate-300">{t('settings.languageHelp')}</p>
              </div>

              {/* Completion Sound Setting */}
              <div className="octoheft-subcard p-4 rounded-xl bg-slate-50 dark:bg-slate-800/80 border border-slate-200 dark:border-slate-700 flex items-center justify-between gap-4">
                <label
                  htmlFor="settings-sound-complete"
                  className="flex items-start gap-3 cursor-pointer flex-1"
                >
                  <input
                    id="settings-sound-complete"
                    type="checkbox"
                    checked={draft.sound_on_complete}
                    onChange={(e) =>
                      setDraft((prev) => ({ ...prev, sound_on_complete: e.target.checked }))
                    }
                    className="mt-0.5 h-4 w-4 rounded border-slate-300 dark:border-slate-600 text-sky-600 accent-sky-600 dark:accent-cyan-400 focus:ring-sky-500 cursor-pointer shrink-0"
                  />
                  <div>
                    <div className="font-semibold text-slate-900 dark:text-slate-100">
                      {t('settings.soundOnComplete')}
                    </div>
                    <div className="text-[11px] text-slate-600 dark:text-slate-300 mt-0.5">
                      {t('settings.soundDesc')}
                    </div>
                  </div>
                </label>

                <button
                  type="button"
                  onClick={() => playCompletionChime()}
                  className="octoheft-subcard px-3 py-1.5 rounded-lg bg-white dark:bg-slate-900 border border-slate-300 dark:border-slate-700 hover:border-sky-400 dark:hover:border-cyan-400 hover:bg-sky-50 dark:hover:bg-sky-950/60 text-slate-800 dark:text-slate-200 font-medium flex items-center gap-1.5 transition-colors shrink-0"
                >
                  <Volume2 className="h-3.5 w-3.5 text-sky-600 dark:text-cyan-400" />
                  <span>{t('settings.testChime')}</span>
                </button>
              </div>
            </div>
          )}

          {/* Tab 2: File Types */}
          {activeTab === 'fileTypes' && (
            <div className="space-y-4">
              <div className="flex items-start justify-between gap-3">
                <div>
                  <h3 className="font-semibold text-slate-900 dark:text-slate-100">{t('settings.fileTypesTitle')}</h3>
                  <p className="text-[11px] text-slate-600 dark:text-slate-300 mt-0.5">{t('settings.fileTypesDesc')}</p>
                </div>
                <button
                  type="button"
                  onClick={handleRestoreDefaultExtensions}
                  className="octoheft-subcard flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-white dark:bg-slate-800 border border-slate-300 dark:border-slate-700 text-slate-700 dark:text-slate-200 hover:border-sky-400 dark:hover:border-cyan-400 hover:text-sky-700 dark:hover:text-cyan-300 font-medium transition-colors shrink-0"
                >
                  <RotateCcw className="h-3.5 w-3.5" />
                  <span>{t('settings.restoreDefaults')}</span>
                </button>
              </div>

              <div className="octoheft-subcard p-4 rounded-xl bg-slate-50 dark:bg-slate-800/80 border border-slate-200 dark:border-slate-700 space-y-2">
                <label
                  htmlFor="intercept-extensions-textarea"
                  className="block font-semibold text-slate-800 dark:text-slate-200"
                >
                  {t('settings.fileTypesLabel')}
                </label>
                <textarea
                  id="intercept-extensions-textarea"
                  rows={6}
                  dir="ltr"
                  spellCheck={false}
                  value={draft.intercept_extensions}
                  onChange={(e) =>
                    setDraft((prev) => ({ ...prev, intercept_extensions: e.target.value }))
                  }
                  className="w-full rounded-lg bg-white dark:bg-slate-900 border border-slate-300 dark:border-slate-700 px-3.5 py-2.5 text-xs font-mono text-slate-900 dark:text-slate-100 leading-relaxed focus:border-sky-600 dark:focus:border-cyan-400 focus:outline-none focus:ring-2 focus:ring-sky-500/20"
                />
                <p className="text-[11px] text-slate-500 dark:text-slate-400">{t('settings.fileTypesHint')}</p>
              </div>
            </div>
          )}

          {/* Tab 3: Save To */}
          {activeTab === 'saveTo' && (
            <div className="space-y-4">
              <div>
                <h3 className="font-semibold text-slate-900 dark:text-slate-100">{t('settings.saveToTitle')}</h3>
              </div>

              <div className="octoheft-subcard p-4 rounded-xl bg-slate-50 dark:bg-slate-800/80 border border-slate-200 dark:border-slate-700 space-y-3">
                <label
                  htmlFor="default-download-folder-input"
                  className="block font-semibold text-slate-800 dark:text-slate-200"
                >
                  {t('settings.defaultFolderLabel')}
                </label>
                <div className="flex items-center gap-2">
                  <input
                    id="default-download-folder-input"
                    type="text"
                    dir="ltr"
                    spellCheck={false}
                    value={draft.default_download_dir}
                    onChange={(e) =>
                      setDraft((prev) => ({ ...prev, default_download_dir: e.target.value }))
                    }
                    className="flex-1 rounded-lg bg-white dark:bg-slate-900 border border-slate-300 dark:border-slate-700 px-3 py-2 text-xs font-mono text-slate-900 dark:text-slate-100 focus:border-sky-600 dark:focus:border-cyan-400 focus:outline-none focus:ring-2 focus:ring-sky-500/20"
                  />
                  <button
                    type="button"
                    onClick={handleBrowseFolder}
                    className="octoheft-subcard px-3.5 py-2 rounded-lg bg-white dark:bg-slate-900 border border-slate-300 dark:border-slate-700 hover:border-sky-400 dark:hover:border-cyan-400 hover:bg-sky-50 dark:hover:bg-sky-950/60 text-slate-800 dark:text-slate-200 font-medium flex items-center gap-1.5 transition-colors shrink-0"
                  >
                    <FolderOpen className="h-3.5 w-3.5 text-amber-600 dark:text-amber-400" />
                    <span>{t('settings.browseFolder')}</span>
                  </button>
                </div>
              </div>

              <label
                htmlFor="settings-category-subfolders"
                className="octoheft-subcard p-4 rounded-xl bg-slate-50 dark:bg-slate-800/80 border border-slate-200 dark:border-slate-700 flex items-start gap-3 cursor-pointer hover:border-slate-300 dark:hover:border-slate-600 transition-colors"
              >
                <input
                  id="settings-category-subfolders"
                  type="checkbox"
                  checked={draft.use_category_subfolders}
                  onChange={(e) =>
                    setDraft((prev) => ({
                      ...prev,
                      use_category_subfolders: e.target.checked,
                    }))
                  }
                  className="mt-0.5 h-4 w-4 rounded border-slate-300 dark:border-slate-600 text-sky-600 accent-sky-600 dark:accent-cyan-400 focus:ring-sky-500 cursor-pointer shrink-0"
                />
                <div>
                  <div className="font-semibold text-slate-900 dark:text-slate-100">{t('settings.useSubfolders')}</div>
                  <div className="text-[11px] text-slate-600 dark:text-slate-300 mt-0.5">
                    {t('settings.useSubfoldersDesc')}
                  </div>
                </div>
              </label>
            </div>
          )}

          {/* Tab 4: Browser Integration */}
          {activeTab === 'browser' && (
            <div className="space-y-4">
              <div className="flex items-center justify-between gap-2">
                <div>
                  <span className="block font-semibold text-slate-900 dark:text-slate-100">
                    {t('settings.browserTitle')}
                  </span>
                  <span className="text-[11px] text-slate-600 dark:text-slate-300">
                    {t('settings.browserSubtitle')}
                  </span>
                </div>
                {onOpenBrowserWizard && (
                  <button
                    type="button"
                    onClick={onOpenBrowserWizard}
                    className="px-3 py-1.5 rounded-lg bg-sky-50 dark:bg-sky-950/80 border border-sky-200 dark:border-cyan-500/40 text-sky-800 dark:text-cyan-300 hover:bg-sky-100 dark:hover:bg-sky-900/80 text-[11px] font-semibold transition-colors shrink-0"
                  >
                    {t('settings.browserWizardBtn')}
                  </button>
                )}
              </div>

              <div className="space-y-2">
                {(browserReport?.browsers || []).map((b) => {
                  const isConnected = b.installed && b.registry_registered && b.manifest_exists;
                  return (
                    <div
                      key={b.id}
                      className="octoheft-subcard flex items-center justify-between p-3 rounded-xl bg-slate-50 dark:bg-slate-800/80 border border-slate-200 dark:border-slate-700"
                    >
                      <div className="min-w-0 pe-3">
                        <div className="font-semibold text-slate-900 dark:text-slate-100">{b.name}</div>
                        <div className="text-[10px] font-mono text-slate-500 dark:text-slate-400 truncate" dir="ltr">
                          {b.registry_key}
                        </div>
                      </div>
                      {!b.installed ? (
                        <span className="px-2.5 py-1 rounded-full text-[11px] font-medium bg-slate-200 dark:bg-slate-700 text-slate-600 dark:text-slate-300 shrink-0">
                          {t('settings.browserNotDetected')}
                        </span>
                      ) : isConnected ? (
                        <span className="px-2.5 py-1 rounded-full text-[11px] font-semibold bg-emerald-50 dark:bg-emerald-950/70 text-emerald-700 dark:text-emerald-300 border border-emerald-200 dark:border-emerald-500/40 flex items-center gap-1.5 shrink-0">
                          <CheckCircle2 className="h-3.5 w-3.5" /> {t('settings.browserConnected')}
                        </span>
                      ) : (
                        <span className="px-2.5 py-1 rounded-full text-[11px] font-semibold bg-amber-50 dark:bg-amber-950/70 text-amber-800 dark:text-amber-300 border border-amber-200 dark:border-amber-500/40 flex items-center gap-1.5 shrink-0">
                          <AlertCircle className="h-3.5 w-3.5" /> {t('settings.browserNeedsRepair')}
                        </span>
                      )}
                    </div>
                  );
                })}
              </div>

              {browserReport && (
                <div className="octoheft-subcard p-3 rounded-xl bg-slate-50 dark:bg-slate-800/80 border border-slate-200 dark:border-slate-700 space-y-1 text-[10px] font-mono text-slate-600 dark:text-slate-300">
                  <div className="truncate" dir="ltr">
                    <span className="text-slate-500 dark:text-slate-400">{t('settings.browserExtDir')}</span>{' '}
                    {browserReport.extension_dir}
                  </div>
                  <div className="truncate" dir="ltr">
                    <span className="text-slate-500 dark:text-slate-400">{t('settings.browserManifest')}</span>{' '}
                    {browserReport.manifest_path}
                  </div>
                </div>
              )}

              {browserActionMessage && (
                <div className="p-2.5 rounded-xl bg-emerald-50 dark:bg-emerald-950/70 border border-emerald-200 dark:border-emerald-500/40 text-emerald-800 dark:text-emerald-300 text-[11px] flex items-center gap-2">
                  <CheckCircle2 className="h-4 w-4 shrink-0" />
                  <span>{browserActionMessage}</span>
                </div>
              )}

              <div className="flex flex-wrap gap-2">
                <button
                  type="button"
                  onClick={handleRepairBrowserKeys}
                  className="px-3.5 py-2 rounded-lg bg-sky-700 hover:bg-sky-800 dark:hover:bg-sky-600 border border-transparent dark:border-cyan-400/30 text-white font-semibold flex items-center gap-2 transition-colors"
                >
                  <Wrench className="h-3.5 w-3.5" />
                  {t('settings.browserRepairBtn')}
                </button>
                <button
                  type="button"
                  onClick={handleOpenExtensionFolder}
                  className="octoheft-subcard px-3.5 py-2 rounded-lg bg-white dark:bg-slate-800 border border-slate-300 dark:border-slate-700 hover:border-slate-400 dark:hover:border-slate-600 text-slate-800 dark:text-slate-200 font-medium flex items-center gap-2 transition-colors"
                >
                  <FolderOpen className="h-3.5 w-3.5 text-amber-600 dark:text-amber-400" />
                  {t('settings.browserOpenFolderBtn')}
                </button>
              </div>
            </div>
          )}

          {/* Footer Actions */}
          <div className="pt-4 flex items-center justify-between border-t border-slate-200 dark:border-slate-700">
            {savedSuccess ? (
              <span className="text-emerald-700 dark:text-emerald-300 flex items-center gap-1.5 text-xs font-semibold">
                <CheckCircle2 className="h-4 w-4" /> {t('settings.saved')}
              </span>
            ) : (
              <div />
            )}

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
                {t('common.saveSettings')}
              </button>
            </div>
          </div>
        </form>
      </div>
    </div>
  );
};
