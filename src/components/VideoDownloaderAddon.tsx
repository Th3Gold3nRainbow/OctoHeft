import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  AlertCircle,
  CheckCircle2,
  Clapperboard,
  Cookie,
  Download,
  ExternalLink,
  Film,
  FolderOpen,
  HardDriveDownload,
  Loader2,
  Music,
  Pause,
  Play,
  RefreshCw,
  RotateCcw,
  Search,
  Sparkles,
  Wrench,
  X,
} from 'lucide-react';
import type {
  AppSettings,
  CategoryRoutes,
  DownloadTask,
  OmnigetFormatOption,
  OmnigetMediaInfo,
  OmnigetToolsStatus,
  VideoGrabberPayload,
} from '../types';
import { formatBytes, formatEta, formatSpeed } from '../utils';
import { useI18n } from '../i18n';

interface VideoDownloaderAddonProps {
  grabSignal: { payload: VideoGrabberPayload; seq: number } | null;
  appSettings: AppSettings;
  tasks: DownloadTask[];
  onClearMedia?: () => void;
  onTaskStarted: (task: DownloadTask) => void;
  onInspectTask: (taskId: string) => void;
  onPauseTask: (taskId: string) => void;
  onResumeTask: (taskId: string) => void;
  onOpenFolder: (path: string) => void;
}

function normalizeUrlValidation(
  raw: string,
  t: ReturnType<typeof useI18n>['t']
): string | null {
  const trimmed = raw.trim();
  if (!trimmed) {
    return t('videoAddon.urlErrorEmpty');
  }
  const lower = trimmed.toLowerCase();
  if (lower.startsWith('blob:')) {
    return t('videoAddon.urlErrorScheme');
  }
  try {
    const parsed = new URL(trimmed);
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
      return t('videoAddon.urlErrorScheme');
    }
    return null;
  } catch {
    return t('videoAddon.urlErrorScheme');
  }
}

export const VideoDownloaderAddon: React.FC<VideoDownloaderAddonProps> = ({
  grabSignal,
  appSettings,
  tasks,
  onClearMedia,
  onTaskStarted,
  onInspectTask,
  onPauseTask,
  onResumeTask,
  onOpenFolder,
}) => {
  const { t, language } = useI18n();
  const urlInputRef = useRef<HTMLInputElement | null>(null);
  const lastSeqHandledRef = useRef<number>(0);
  const fetchCounterRef = useRef<number>(0);
  const blurAnnounceTimerRef = useRef<number | null>(null);

  const [toolsStatus, setToolsStatus] = useState<OmnigetToolsStatus | null>(null);
  const [checkingTools, setCheckingTools] = useState(true);
  const [settingUpTools, setSettingUpTools] = useState(false);
  const [toolsError, setToolsError] = useState<string | null>(null);

  const [urlInput, setUrlInput] = useState('');
  const [urlDirty, setUrlDirty] = useState(false);
  const [urlTouched, setUrlTouched] = useState(false);
  const [urlSubmitted, setUrlSubmitted] = useState(false);
  const [urlValidationError, setUrlValidationError] = useState<string | null>(null);

  const [grabContext, setGrabContext] = useState<VideoGrabberPayload | null>(null);
  const [fetching, setFetching] = useState(false);
  const [fetchError, setFetchError] = useState<string | null>(null);
  const [mediaInfo, setMediaInfo] = useState<OmnigetMediaInfo | null>(null);
  const [thumbFailed, setThumbFailed] = useState(false);

  const [selectedFormatId, setSelectedFormatId] = useState<string>('');
  const [customFileName, setCustomFileName] = useState('');
  const [saveDir, setSaveDir] = useState('');
  const [userPickedDir, setUserPickedDir] = useState(false);
  const [routes, setRoutes] = useState<CategoryRoutes | null>(null);

  const [startingDownload, setStartingDownload] = useState(false);
  const [downloadError, setDownloadError] = useState<string | null>(null);
  const [lastStartedTaskId, setLastStartedTaskId] = useState<string | null>(null);
  const [statusAnnouncement, setStatusAnnouncement] = useState('');

  const selectedFormat = useMemo<OmnigetFormatOption | null>(() => {
    if (!mediaInfo || mediaInfo.formats.length === 0) return null;
    return (
      mediaInfo.formats.find((f) => f.id === selectedFormatId) ||
      mediaInfo.formats.find((f) => f.recommended) ||
      mediaInfo.formats[0]
    );
  }, [mediaInfo, selectedFormatId]);

  const computeDefaultDir = useCallback(
    (categoryRoutes: CategoryRoutes | null, audioOnly: boolean): string => {
      if (!categoryRoutes) return appSettings.default_download_dir || '';
      const catKey = audioOnly ? 'audio' : 'video';
      const route = categoryRoutes.routes.find((r) => r.category === catKey);
      if (route?.custom_path && route.custom_path.trim()) {
        return route.custom_path;
      }
      const base = (categoryRoutes.base_download_dir || appSettings.default_download_dir || '').replace(
        /[\\/]+$/,
        ''
      );
      if (!base) return '';
      if (!appSettings.use_category_subfolders || !route?.relative_path) {
        return base;
      }
      const sep = base.includes('/') && !base.includes('\\') ? '/' : '\\';
      return `${base}${sep}${route.relative_path}`;
    },
    [appSettings.default_download_dir, appSettings.use_category_subfolders]
  );

  const refreshTools = useCallback(async () => {
    setCheckingTools(true);
    setToolsError(null);
    try {
      const st = await invoke<OmnigetToolsStatus>('omniget_check_tools');
      setToolsStatus(st);
    } catch (err) {
      setToolsError(String(err));
    } finally {
      setCheckingTools(false);
    }
  }, []);

  useEffect(() => {
    refreshTools();
    invoke<CategoryRoutes>('get_category_routes')
      .then((r) => {
        setRoutes(r);
        setSaveDir((prev) => prev || computeDefaultDir(r, false));
      })
      .catch(console.error);
  }, [refreshTools, computeDefaultDir]);

  useEffect(() => {
    if (!userPickedDir && routes) {
      const next = computeDefaultDir(routes, Boolean(selectedFormat?.audio_only));
      if (next) setSaveDir(next);
    }
  }, [selectedFormat?.audio_only, userPickedDir, routes, computeDefaultDir]);

  const performFetch = useCallback(
    async (targetUrl: string, context: VideoGrabberPayload | null) => {
      const reqSeq = ++fetchCounterRef.current;
      const cleanUrl = targetUrl.trim();
      setFetching(true);
      setFetchError(null);
      setDownloadError(null);
      setThumbFailed(false);

      const useCtx = context && context.url.trim() === cleanUrl ? context : null;
      try {
        const info = await invoke<OmnigetMediaInfo>('omniget_fetch_formats', {
          request: {
            url: cleanUrl,
            candidate_src: useCtx?.candidate_src || null,
            cookies: useCtx?.cookies || null,
            cookie_entries: useCtx?.cookie_entries || null,
            user_agent: useCtx?.user_agent || null,
            referrer: useCtx?.referrer || null,
          },
        });
        if (fetchCounterRef.current !== reqSeq) return;
        setMediaInfo(info);
        const rec = info.formats.find((f) => f.recommended) || info.formats[0];
        setSelectedFormatId(rec ? rec.id : '');
        if (!customFileName.trim() && info.title) {
          setCustomFileName(info.title);
        }
        setStatusAnnouncement(
          t('videoAddon.statusReady', {
            count: info.formats.length,
            title: info.title,
          })
        );
      } catch (err) {
        if (fetchCounterRef.current !== reqSeq) return;
        setFetchError(String(err));
      } finally {
        if (fetchCounterRef.current === reqSeq) {
          setFetching(false);
        }
      }
    },
    [customFileName, t]
  );

  // React cleanly when triggered by the browser extension floating button / popup
  useEffect(() => {
    if (!grabSignal || grabSignal.seq <= lastSeqHandledRef.current) return;
    lastSeqHandledRef.current = grabSignal.seq;
    const payload = grabSignal.payload;
    if (!payload?.url) return;

    setGrabContext(payload);
    setUrlInput(payload.url);
    setCustomFileName(payload.title || '');
    setUrlDirty(false);
    setUrlTouched(false);
    setUrlSubmitted(false);
    setUrlValidationError(null);
    setMediaInfo(null);
    setSelectedFormatId('');
    performFetch(payload.url, payload);
  }, [grabSignal, performFetch]);

  const handleSetupTools = async () => {
    setSettingUpTools(true);
    setToolsError(null);
    try {
      const updated = await invoke<OmnigetToolsStatus>('omniget_ensure_tools');
      setToolsStatus(updated);
    } catch (err) {
      setToolsError(String(err));
    } finally {
      setSettingUpTools(false);
    }
  };

  const handleUrlChange = (value: string) => {
    setUrlInput(value);
    setUrlDirty(true);
    setFetchError(null);
    if (blurAnnounceTimerRef.current !== null) {
      window.clearTimeout(blurAnnounceTimerRef.current);
      blurAnnounceTimerRef.current = null;
    }
    // Clear existing visible error promptly as soon as input becomes valid
    if (urlValidationError) {
      const nextErr = normalizeUrlValidation(value, t);
      if (!nextErr) {
        setUrlValidationError(null);
        setStatusAnnouncement('');
      }
    }
  };

  const handleUrlBlur = () => {
    if (!urlDirty) return;
    setUrlTouched(true);
    const err = normalizeUrlValidation(urlInput, t);
    setUrlValidationError(err);
    if (blurAnnounceTimerRef.current !== null) {
      window.clearTimeout(blurAnnounceTimerRef.current);
      blurAnnounceTimerRef.current = null;
    }
    if (err) {
      // Delay slightly so announcement does not collide with the next focused control
      blurAnnounceTimerRef.current = window.setTimeout(() => {
        setStatusAnnouncement(`${t('videoAddon.errorPrefix')} ${err}`);
      }, 450);
    }
  };

  useEffect(() => {
    return () => {
      if (blurAnnounceTimerRef.current !== null) {
        window.clearTimeout(blurAnnounceTimerRef.current);
      }
    };
  }, []);

  const handleClearMedia = useCallback(() => {
    fetchCounterRef.current += 1;
    if (blurAnnounceTimerRef.current !== null) {
      window.clearTimeout(blurAnnounceTimerRef.current);
      blurAnnounceTimerRef.current = null;
    }
    setFetching(false);
    setFetchError(null);
    setDownloadError(null);
    setUrlInput('');
    setUrlDirty(false);
    setUrlTouched(false);
    setUrlSubmitted(false);
    setUrlValidationError(null);
    setGrabContext(null);
    setMediaInfo(null);
    setSelectedFormatId('');
    setCustomFileName('');
    setThumbFailed(false);
    setLastStartedTaskId(null);
    setStatusAnnouncement(t('videoAddon.statusCleared'));
    onClearMedia?.();
    urlInputRef.current?.focus();
  }, [onClearMedia, t]);

  const handleUrlSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    setUrlSubmitted(true);
    const err = normalizeUrlValidation(urlInput, t);
    setUrlValidationError(err);
    if (err) {
      urlInputRef.current?.focus();
      return;
    }
    const matchingCtx =
      grabContext && grabContext.url.trim() === urlInput.trim() ? grabContext : null;
    performFetch(urlInput, matchingCtx);
  };

  const handleBrowseFolder = async () => {
    try {
      const chosen = await invoke<string | null>('pick_download_folder', {
        title: t('videoAddon.saveFolderLabel'),
      });
      if (chosen) {
        setSaveDir(chosen);
        setUserPickedDir(true);
      }
    } catch (err) {
      setDownloadError(String(err));
    }
  };

  const handleStartDownload = async () => {
    if (!mediaInfo || !selectedFormat || startingDownload) return;
    setStartingDownload(true);
    setDownloadError(null);

    const useCtx =
      grabContext && grabContext.url.trim() === mediaInfo.url.trim() ? grabContext : null;

    try {
      const task = await invoke<DownloadTask>('omniget_start_download', {
        request: {
          url: mediaInfo.url,
          title: mediaInfo.title,
          format_spec: selectedFormat.format_spec,
          format_id: selectedFormat.id,
          container: selectedFormat.container,
          resolution: selectedFormat.resolution,
          vcodec: selectedFormat.vcodec,
          acodec: selectedFormat.acodec,
          audio_only: selectedFormat.audio_only,
          estimated_bytes: selectedFormat.filesize_bytes ?? null,
          save_dir: saveDir.trim() || null,
          file_name: customFileName.trim() || mediaInfo.title || null,
          cookies: useCtx?.cookies || null,
          cookie_entries: useCtx?.cookie_entries || null,
          user_agent: useCtx?.user_agent || null,
          referrer: useCtx?.referrer || null,
          direct_url: selectedFormat.direct_url || null,
        },
      });
      setLastStartedTaskId(task.id);
      setStatusAnnouncement(t('videoAddon.statusStarted', { file: task.file_name }));
      onTaskStarted(task);
    } catch (err) {
      setDownloadError(String(err));
    } finally {
      setStartingDownload(false);
    }
  };

  const activeOctoMediaTasks = useMemo(() => {
    const ytTasks = tasks.filter((tk) => tk.media_info?.protocol === 'YT-DLP');
    const liveOrRecent = ytTasks.filter(
      (tk) =>
        tk.status === 'downloading' ||
        tk.status === 'connecting' ||
        tk.status === 'paused' ||
        tk.id === lastStartedTaskId
    );
    return liveOrRecent.slice(0, 3);
  }, [tasks, lastStartedTaskId]);

  const showUrlError = Boolean(urlValidationError && (urlTouched || urlSubmitted));
  const urlDescribedBy = showUrlError
    ? 'octomedia-url-hint octomedia-url-error'
    : 'octomedia-url-hint';
  const hasSessionCookies = Boolean(
    grabContext &&
      grabContext.url.trim() === (mediaInfo?.url || urlInput).trim() &&
      (grabContext.cookies || (grabContext.cookie_entries && grabContext.cookie_entries.length > 0))
  );

  return (
    <div className="flex-1 overflow-y-auto p-5 lg:p-6 space-y-5">
      {/* Centralized polite screen-reader announcement region */}
      <div role="status" aria-live="polite" className="visually-hidden">
        {statusAnnouncement}
      </div>

      {/* Header + Engine Status Card */}
      <div className="card-surface rounded-2xl border border-slate-200/90 dark:border-slate-700 bg-white/90 dark:bg-slate-900/85 backdrop-blur-md p-5 shadow-sm flex flex-col gap-4 md:flex-row md:items-center md:justify-between">
        <div className="space-y-1">
          <div className="flex items-center gap-2.5">
            <span className="inline-flex items-center justify-center h-9 w-9 rounded-xl bg-sky-100 dark:bg-sky-950/80 text-sky-700 dark:text-cyan-300 border border-sky-200 dark:border-cyan-500/40">
              <Clapperboard className="h-5 w-5" aria-hidden="true" />
            </span>
            <div>
              <h1
                id="octomedia-heading"
                className="text-base font-bold text-slate-900 dark:text-slate-100 leading-snug"
              >
                {t('videoAddon.title')}
              </h1>
              <p className="text-xs text-slate-600 dark:text-slate-300">{t('videoAddon.subtitle')}</p>
            </div>
          </div>
        </div>

        <div className="flex flex-wrap items-center gap-2.5 shrink-0">
          {checkingTools ? (
            <span className="octoheft-subcard inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-slate-100 dark:bg-slate-800 text-slate-700 dark:text-slate-200 border border-slate-200 dark:border-slate-700 text-xs font-medium">
              <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
              {t('videoAddon.checkingTools')}
            </span>
          ) : toolsStatus?.ready ? (
            <span className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-emerald-50 dark:bg-emerald-950/70 text-emerald-800 dark:text-emerald-300 border border-emerald-200 dark:border-emerald-500/40 text-xs font-semibold">
              <CheckCircle2
                className="h-3.5 w-3.5 text-emerald-600 dark:text-emerald-400 shrink-0"
                aria-hidden="true"
              />
              <span>
                {t('videoAddon.toolsReady', {
                  ytdlp: toolsStatus.ytdlp_version
                    ? `yt-dlp ${toolsStatus.ytdlp_version}`
                    : 'yt-dlp',
                })}
              </span>
            </span>
          ) : (
            <div className="flex items-center gap-2 flex-wrap">
              <span className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-amber-50 dark:bg-amber-950/70 text-amber-900 dark:text-amber-300 border border-amber-200 dark:border-amber-500/40 text-xs font-semibold">
                <AlertCircle
                  className="h-3.5 w-3.5 text-amber-600 dark:text-amber-400 shrink-0"
                  aria-hidden="true"
                />
                {t('videoAddon.toolsMissing')}
              </span>
              <button
                type="button"
                onClick={handleSetupTools}
                disabled={settingUpTools}
                className="min-h-[44px] inline-flex items-center gap-1.5 px-3.5 py-2 rounded-lg text-xs font-semibold bg-sky-700 text-white hover:bg-sky-800 dark:hover:bg-sky-600 border border-transparent dark:border-cyan-400/30 disabled:opacity-60 focus-visible:outline focus-visible:outline-2 focus-visible:outline-sky-600 focus-visible:outline-offset-2 transition-colors"
              >
                {settingUpTools ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
                ) : (
                  <Wrench className="h-3.5 w-3.5" aria-hidden="true" />
                )}
                <span>
                  {settingUpTools ? t('videoAddon.settingUpTools') : t('videoAddon.setupToolsBtn')}
                </span>
              </button>
            </div>
          )}
          <button
            type="button"
            onClick={refreshTools}
            disabled={checkingTools || settingUpTools}
            aria-label={t('videoAddon.checkingTools')}
            className="octoheft-subcard min-h-[44px] min-w-[44px] inline-flex items-center justify-center rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-800 text-slate-600 dark:text-slate-300 hover:bg-slate-50 dark:hover:bg-slate-700 hover:text-slate-900 dark:hover:text-white focus-visible:outline focus-visible:outline-2 focus-visible:outline-sky-600 focus-visible:outline-offset-2 transition-colors"
          >
            <RefreshCw
              className={`h-4 w-4 ${checkingTools ? 'animate-spin' : ''}`}
              aria-hidden="true"
            />
          </button>
        </div>
      </div>

      {toolsError && (
        <div
          role="alert"
          className="rounded-xl border border-rose-300 dark:border-rose-500/40 bg-rose-50 dark:bg-rose-950/70 px-4 py-3 text-xs text-rose-900 dark:text-rose-200 flex items-start gap-2"
        >
          <AlertCircle className="h-4 w-4 text-rose-600 dark:text-rose-400 shrink-0 mt-0.5" aria-hidden="true" />
          <span>
            <strong className="font-semibold">{t('videoAddon.errorPrefix')}</strong> {toolsError}
          </span>
        </div>
      )}

      {/* URL Extraction Form */}
      <form
        noValidate
        aria-labelledby="octomedia-heading"
        onSubmit={handleUrlSubmit}
        className="card-surface rounded-2xl border border-slate-200/90 dark:border-slate-700 bg-white/90 dark:bg-slate-900/85 backdrop-blur-md p-5 shadow-sm space-y-3"
      >
        <div className="flex flex-col gap-1.5">
          <label
            htmlFor="octomedia-url-input"
            className="text-xs font-semibold text-slate-800 dark:text-slate-200 inline-block"
          >
            {t('videoAddon.urlLabel')}
          </label>
          {/* Hint placed above input so autocomplete or browser popovers do not obscure it */}
          <span id="octomedia-url-hint" className="text-xs text-slate-600 dark:text-slate-300">
            {t('videoAddon.urlHint')}
          </span>

          <div className="flex flex-col sm:flex-row gap-2.5 pt-0.5">
            <div className="relative flex-1 min-w-0 flex items-center">
              <input
                ref={urlInputRef}
                id="octomedia-url-input"
                name="media_url"
                type="url"
                dir="ltr"
                required
                spellCheck={false}
                autoCapitalize="none"
                autoCorrect="off"
                enterKeyHint="go"
                placeholder={t('videoAddon.urlPlaceholder')}
                value={urlInput}
                onChange={(e) => handleUrlChange(e.target.value)}
                onBlur={handleUrlBlur}
                aria-describedby={urlDescribedBy}
                aria-invalid={showUrlError ? 'true' : 'false'}
                className={`w-full min-h-[44px] rounded-xl ps-3.5 ${
                  urlInput.length > 0 ? 'pe-11' : 'pe-3.5'
                } py-2 text-sm font-mono transition-colors focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 ${
                  showUrlError
                    ? 'border-2 border-rose-600 bg-rose-50/70 dark:bg-rose-950/60 text-slate-900 dark:text-rose-100 focus-visible:outline-rose-600'
                    : 'border border-slate-300 dark:border-slate-700 bg-white dark:bg-slate-800 text-slate-900 dark:text-slate-100 focus-visible:outline-sky-600 dark:focus-visible:outline-cyan-400 focus:border-sky-600 dark:focus:border-cyan-400'
                }`}
              />
              {urlInput.length > 0 && (
                <button
                  type="button"
                  onClick={handleClearMedia}
                  aria-label={t('videoAddon.clearUrlBtn')}
                  title={t('videoAddon.clearUrlBtn')}
                  className="absolute end-1 top-1/2 -translate-y-1/2 inline-flex items-center justify-center h-9 w-9 rounded-lg text-slate-500 dark:text-slate-400 hover:text-slate-900 dark:hover:text-white hover:bg-slate-200/70 dark:hover:bg-slate-700/70 focus-visible:outline focus-visible:outline-2 focus-visible:outline-sky-600 focus-visible:-outline-offset-2 transition-colors"
                >
                  <X className="h-4 w-4" aria-hidden="true" />
                </button>
              )}
            </div>

            <button
              type="submit"
              disabled={fetching}
              className="min-h-[44px] inline-flex items-center justify-center gap-2 px-5 py-2.5 rounded-xl text-xs font-semibold bg-sky-700 text-white hover:bg-sky-800 dark:hover:bg-sky-600 border border-transparent dark:border-cyan-400/30 disabled:opacity-60 shadow-sm shadow-sky-700/20 focus-visible:outline focus-visible:outline-2 focus-visible:outline-sky-600 focus-visible:outline-offset-2 transition-colors shrink-0"
            >
              {fetching ? (
                <Loader2 className="h-4 w-4 animate-spin" aria-hidden="true" />
              ) : (
                <Search className="h-4 w-4" aria-hidden="true" />
              )}
              <span>{fetching ? t('videoAddon.fetchingBtn') : t('videoAddon.fetchBtn')}</span>
            </button>
          </div>

          {showUrlError && (
            <p
              id="octomedia-url-error"
              className="text-xs font-medium text-rose-700 dark:text-rose-300 flex items-center gap-1.5 pt-1"
            >
              <AlertCircle
                className="h-3.5 w-3.5 text-rose-600 dark:text-rose-400 shrink-0"
                aria-hidden="true"
              />
              <span>
                <strong className="font-semibold">{t('videoAddon.errorPrefix')}</strong>{' '}
                {urlValidationError}
              </span>
            </p>
          )}
        </div>

        {fetchError && (
          <div
            role="alert"
            className="rounded-xl border border-rose-300 dark:border-rose-500/40 bg-rose-50 dark:bg-rose-950/70 px-3.5 py-2.5 text-xs text-rose-900 dark:text-rose-200 flex items-start gap-2"
          >
            <AlertCircle
              className="h-4 w-4 text-rose-600 dark:text-rose-400 shrink-0 mt-0.5"
              aria-hidden="true"
            />
            <span>
              <strong className="font-semibold">{t('videoAddon.errorPrefix')}</strong> {fetchError}
            </span>
          </div>
        )}
      </form>

      {/* Media Preview + Responsive Format Selection Table */}
      {mediaInfo ? (
        <div className="card-surface rounded-2xl border border-slate-200/90 dark:border-slate-700 bg-white/95 dark:bg-slate-900/90 backdrop-blur-md p-5 shadow-sm space-y-5">
          {/* Metadata Header */}
          <div className="flex flex-col sm:flex-row gap-4 items-start">
            <div className="octoheft-subcard w-full sm:w-44 aspect-video rounded-xl overflow-hidden bg-slate-100 dark:bg-slate-800 border border-slate-200 dark:border-slate-700 flex items-center justify-center shrink-0">
              {mediaInfo.thumbnail && !thumbFailed ? (
                <img
                  src={mediaInfo.thumbnail}
                  alt={mediaInfo.title}
                  onError={() => setThumbFailed(true)}
                  className="w-full h-full object-cover"
                />
              ) : (
                <Film className="h-8 w-8 text-slate-400 dark:text-slate-500" aria-hidden="true" />
              )}
            </div>

            <div className="flex-1 min-w-0 space-y-2">
              <div className="flex flex-wrap items-start justify-between gap-2.5">
                <h2
                  id="octomedia-media-title"
                  className="text-base font-bold text-slate-900 dark:text-slate-100 leading-snug break-words flex-1 min-w-0"
                >
                  {mediaInfo.title}
                </h2>
                <button
                  type="button"
                  onClick={handleClearMedia}
                  className="octoheft-subcard min-h-[36px] inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-semibold text-slate-700 dark:text-slate-200 bg-slate-100 dark:bg-slate-800 hover:bg-rose-50 dark:hover:bg-rose-950/60 hover:text-rose-700 dark:hover:text-rose-300 border border-slate-200 dark:border-slate-700 hover:border-rose-200 dark:hover:border-rose-500/40 focus-visible:outline focus-visible:outline-2 focus-visible:outline-sky-600 focus-visible:outline-offset-2 transition-colors shrink-0"
                >
                  <RotateCcw className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                  <span>{t('videoAddon.cancelVideoBtn')}</span>
                </button>
              </div>

              <div className="flex flex-wrap items-center gap-2 text-xs">
                {mediaInfo.uploader && (
                  <span className="octoheft-subcard px-2.5 py-1 rounded-lg bg-slate-100 dark:bg-slate-800 text-slate-800 dark:text-slate-200 border border-slate-200 dark:border-slate-700 font-medium">
                    {mediaInfo.uploader}
                  </span>
                )}
                {mediaInfo.extractor && (
                  <span className="px-2.5 py-1 rounded-lg bg-sky-50 dark:bg-sky-950/80 text-sky-800 dark:text-cyan-300 border border-sky-200 dark:border-cyan-500/40 font-semibold uppercase text-[11px]">
                    {mediaInfo.extractor}
                  </span>
                )}
                {mediaInfo.duration_formatted && (
                  <span className="px-2.5 py-1 rounded-lg bg-indigo-50 dark:bg-indigo-950/70 text-indigo-800 dark:text-indigo-300 border border-indigo-200 dark:border-indigo-500/40 font-mono text-[11px] font-semibold">
                    {t('videoAddon.durationLabel')}: {mediaInfo.duration_formatted}
                  </span>
                )}
                {hasSessionCookies && (
                  <span className="inline-flex items-center gap-1 px-2.5 py-1 rounded-lg bg-emerald-50 dark:bg-emerald-950/70 text-emerald-800 dark:text-emerald-300 border border-emerald-200 dark:border-emerald-500/40 text-[11px] font-medium">
                    <Cookie
                      className="h-3.5 w-3.5 text-emerald-600 dark:text-emerald-400 shrink-0"
                      aria-hidden="true"
                    />
                    {t('videoAddon.browserSessionActive')}
                  </span>
                )}
              </div>
            </div>
          </div>

          {/* Format Picker Table (Sticky headers + @container stacked cards under 600px) */}
          <div
            tabIndex={0}
            role="region"
            aria-labelledby="octomedia-formats-caption"
            className="octomedia-table-wrapper octoheft-subcard rounded-xl border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 focus-visible:outline focus-visible:outline-2 focus-visible:outline-sky-600 dark:focus-visible:outline-cyan-400 focus-visible:outline-offset-2"
          >
            <table
              className="octomedia-formats-table text-xs text-start"
              style={
                {
                  '--col-codecs': `"${t('videoAddon.colCodecs').replace(/"/g, '\\"')}"`,
                  '--col-format': `"${t('videoAddon.colContainer').replace(/"/g, '\\"')}"`,
                  '--col-size': `"${t('videoAddon.colSize').replace(/"/g, '\\"')}"`,
                } as React.CSSProperties
              }
            >
              <caption
                id="octomedia-formats-caption"
                className="text-xs font-semibold text-slate-700 dark:text-slate-200 bg-slate-50 dark:bg-slate-800/90 px-4 py-2.5 border-b border-slate-200 dark:border-slate-700 text-start"
              >
                {t('videoAddon.tableCaption')} ({mediaInfo.formats.length})
              </caption>
              <thead className="bg-slate-50/95 dark:bg-slate-900 text-[11px] uppercase tracking-wider text-slate-600 dark:text-slate-300 border-b border-slate-200 dark:border-slate-700">
                <tr>
                  <th scope="col" className="px-3 py-2.5 w-14 text-center border-b border-slate-200 dark:border-slate-700">
                    {t('videoAddon.colSelect')}
                  </th>
                  <th scope="col" className="px-3 py-2.5 text-start border-b border-slate-200 dark:border-slate-700">
                    {t('videoAddon.colQuality')}
                  </th>
                  <th scope="col" className="px-3 py-2.5 text-start border-b border-slate-200 dark:border-slate-700">
                    {t('videoAddon.colCodecs')}
                  </th>
                  <th scope="col" className="px-3 py-2.5 text-start border-b border-slate-200 dark:border-slate-700">
                    {t('videoAddon.colContainer')}
                  </th>
                  <th scope="col" className="px-3 py-2.5 text-end border-b border-slate-200 dark:border-slate-700">
                    {t('videoAddon.colSize')}
                  </th>
                </tr>
              </thead>
              <tbody className="divide-y divide-slate-100 dark:divide-slate-800">
                {mediaInfo.formats.map((fmt) => {
                  const isSelected = selectedFormat?.id === fmt.id;
                  const radioId = `octomedia-fmt-${fmt.id}`;

                  return (
                    <tr
                      key={fmt.id}
                      className={`transition-colors ${
                        isSelected
                          ? 'octomedia-row-selected bg-sky-50/90 dark:bg-sky-950/60'
                          : 'bg-white dark:bg-slate-900 hover:bg-slate-50/80 dark:hover:bg-slate-800/70'
                      }`}
                    >
                      <td className="px-3 py-1 text-center align-middle">
                        <div className="min-h-[44px] flex items-center justify-center">
                          <input
                            type="radio"
                            id={radioId}
                            name="octomedia_format"
                            checked={isSelected}
                            onChange={() => setSelectedFormatId(fmt.id)}
                            aria-label={`${fmt.label} ${fmt.container.toUpperCase()}`}
                            className="h-4 w-4 accent-sky-700 dark:accent-cyan-400 cursor-pointer focus-visible:outline focus-visible:outline-2 focus-visible:outline-sky-600 focus-visible:outline-offset-2"
                          />
                        </div>
                      </td>

                      <th scope="row" className="px-3 py-1 font-medium text-slate-900 dark:text-slate-100 align-middle">
                        <label
                          htmlFor={radioId}
                          className="min-h-[44px] flex items-center flex-wrap gap-2 cursor-pointer py-1"
                        >
                          {fmt.audio_only ? (
                            <Music className="h-4 w-4 text-teal-600 dark:text-teal-400 shrink-0" aria-hidden="true" />
                          ) : (
                            <Film className="h-4 w-4 text-indigo-600 dark:text-indigo-400 shrink-0" aria-hidden="true" />
                          )}
                          <span className="font-semibold text-slate-900 dark:text-slate-100">{fmt.label}</span>
                          {fmt.recommended && (
                            <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-[10px] font-bold bg-sky-100 dark:bg-sky-950/80 text-sky-800 dark:text-cyan-300 border border-sky-300 dark:border-cyan-500/40">
                              <Sparkles className="h-3 w-3 text-sky-600 dark:text-cyan-400" aria-hidden="true" />
                              {t('videoAddon.recommended')}
                            </span>
                          )}
                          {fmt.audio_only && (
                            <span className="inline-flex items-center px-2 py-0.5 rounded-full text-[10px] font-semibold bg-teal-50 dark:bg-teal-950/70 text-teal-800 dark:text-teal-300 border border-teal-200 dark:border-teal-500/40">
                              {t('videoAddon.audioOnlyBadge')}
                            </span>
                          )}
                        </label>
                      </th>

                      <td data-col="codecs" className="px-3 py-2 text-slate-700 dark:text-slate-300 font-mono align-middle">
                        {fmt.audio_only ? fmt.acodec : `${fmt.vcodec} + ${fmt.acodec}`}
                      </td>

                      <td data-col="format" className="px-3 py-2 align-middle">
                        <span
                          className={`inline-block px-2 py-0.5 rounded-md font-mono text-[11px] font-bold uppercase border ${
                            fmt.container === 'mp4' || fmt.container === 'm4a'
                              ? 'bg-slate-100 dark:bg-slate-800 text-slate-800 dark:text-slate-200 border-slate-300 dark:border-slate-600'
                              : fmt.container === 'webm' || fmt.container === 'opus'
                              ? 'bg-indigo-50 dark:bg-indigo-950/70 text-indigo-800 dark:text-indigo-300 border-indigo-200 dark:border-indigo-500/40'
                              : 'bg-amber-50 dark:bg-amber-950/70 text-amber-900 dark:text-amber-300 border-amber-200 dark:border-amber-500/40'
                          }`}
                        >
                          .{fmt.container}
                        </span>
                      </td>

                      <td
                        data-col="size"
                        className="px-3 py-2 text-end font-mono font-semibold text-slate-800 dark:text-slate-200 align-middle"
                      >
                        {fmt.filesize_bytes && fmt.filesize_bytes > 0
                          ? formatBytes(fmt.filesize_bytes)
                          : t('videoAddon.sizeUnknown')}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>

          {/* Save Destination & Output Filename + Download Action */}
          <div className="grid grid-cols-1 md:grid-cols-2 gap-4 pt-1">
            <div className="flex flex-col gap-1.5">
              <label htmlFor="octomedia-output-name" className="text-xs font-semibold text-slate-800 dark:text-slate-200">
                {t('videoAddon.fileNameLabel')}
              </label>
              <input
                id="octomedia-output-name"
                name="output_name"
                type="text"
                value={customFileName}
                onChange={(e) => setCustomFileName(e.target.value)}
                placeholder={t('videoAddon.fileNamePlaceholder')}
                className="min-h-[44px] rounded-xl border border-slate-300 dark:border-slate-700 bg-white dark:bg-slate-800 px-3.5 py-2 text-xs text-slate-900 dark:text-slate-100 focus-visible:outline focus-visible:outline-2 focus-visible:outline-sky-600 dark:focus-visible:outline-cyan-400 focus-visible:outline-offset-2"
              />
            </div>

            <div className="flex flex-col gap-1.5">
              <label htmlFor="octomedia-save-dir" className="text-xs font-semibold text-slate-800 dark:text-slate-200">
                {t('videoAddon.saveFolderLabel')}
              </label>
              <div className="flex gap-2">
                <input
                  id="octomedia-save-dir"
                  name="save_dir"
                  type="text"
                  dir="ltr"
                  spellCheck={false}
                  autoCapitalize="none"
                  autoCorrect="off"
                  value={saveDir}
                  onChange={(e) => {
                    setSaveDir(e.target.value);
                    setUserPickedDir(true);
                  }}
                  className="flex-1 min-h-[44px] rounded-xl border border-slate-300 dark:border-slate-700 bg-slate-50 dark:bg-slate-800 px-3.5 py-2 text-xs font-mono text-slate-800 dark:text-slate-100 focus-visible:outline focus-visible:outline-2 focus-visible:outline-sky-600 dark:focus-visible:outline-cyan-400 focus-visible:outline-offset-2"
                />
                <button
                  type="button"
                  onClick={handleBrowseFolder}
                  className="octoheft-subcard min-h-[44px] inline-flex items-center gap-1.5 px-3.5 py-2 rounded-xl border border-slate-300 dark:border-slate-700 bg-white dark:bg-slate-800 text-xs font-semibold text-slate-800 dark:text-slate-200 hover:bg-slate-50 dark:hover:bg-slate-700 focus-visible:outline focus-visible:outline-2 focus-visible:outline-sky-600 focus-visible:outline-offset-2 transition-colors shrink-0"
                >
                  <FolderOpen className="h-4 w-4 text-amber-600 dark:text-amber-400" aria-hidden="true" />
                  <span>{t('videoAddon.browseBtn')}</span>
                </button>
              </div>
            </div>
          </div>

          {downloadError && (
            <div
              role="alert"
              className="rounded-xl border border-rose-300 dark:border-rose-500/40 bg-rose-50 dark:bg-rose-950/70 px-3.5 py-2.5 text-xs text-rose-900 dark:text-rose-200 flex items-start gap-2"
            >
              <AlertCircle
                className="h-4 w-4 text-rose-600 dark:text-rose-400 shrink-0 mt-0.5"
                aria-hidden="true"
              />
              <span>
                <strong className="font-semibold">{t('videoAddon.errorPrefix')}</strong>{' '}
                {downloadError}
              </span>
            </div>
          )}

          <div className="flex justify-end pt-1">
            <button
              type="button"
              onClick={handleStartDownload}
              disabled={startingDownload || !selectedFormat}
              className="min-h-[44px] inline-flex items-center gap-2 px-6 py-2.5 rounded-xl text-sm font-semibold bg-sky-700 text-white hover:bg-sky-800 dark:hover:bg-sky-600 border border-transparent dark:border-cyan-400/30 disabled:opacity-55 shadow-sm shadow-sky-700/20 focus-visible:outline focus-visible:outline-2 focus-visible:outline-sky-600 focus-visible:outline-offset-2 transition-colors"
            >
              {startingDownload ? (
                <Loader2 className="h-4 w-4 animate-spin" aria-hidden="true" />
              ) : (
                <Download className="h-4 w-4" aria-hidden="true" />
              )}
              <span>
                {startingDownload ? t('videoAddon.startingBtn') : t('videoAddon.downloadBtn')}
              </span>
            </button>
          </div>
        </div>
      ) : (
        /* Empty Prompt State */
        <div className="card-surface rounded-2xl border border-dashed border-slate-300 dark:border-slate-700 bg-white/75 dark:bg-slate-900/60 p-10 text-center space-y-2.5">
          <HardDriveDownload
            className="h-10 w-10 text-sky-600 dark:text-cyan-400 mx-auto"
            aria-hidden="true"
          />
          <h2 className="text-sm font-bold text-slate-900 dark:text-slate-100">
            {t('videoAddon.emptyTitle')}
          </h2>
          <p className="text-xs text-slate-600 dark:text-slate-300 max-w-lg mx-auto leading-relaxed">
            {t('videoAddon.emptyDesc')}
          </p>
        </div>
      )}

      {/* Live OctoMedia Transfers Monitor */}
      {activeOctoMediaTasks.length > 0 && (
        <div className="card-surface rounded-2xl border border-slate-200/90 dark:border-slate-700 bg-white/90 dark:bg-slate-900/85 backdrop-blur-md p-5 shadow-sm space-y-3">
          <h2 className="text-xs font-bold uppercase tracking-wider text-slate-600 dark:text-slate-300">
            {t('videoAddon.activeTransferTitle')}
          </h2>
          <div className="space-y-3">
            {activeOctoMediaTasks.map((tk) => {
              const pct =
                tk.status === 'completed'
                  ? 100
                  : tk.total_size > 0
                  ? Math.min(100, Math.round((tk.downloaded_bytes / tk.total_size) * 100))
                  : Math.round(tk.media_info?.mux_percent || 0);
              const isRunning = tk.status === 'downloading' || tk.status === 'connecting';

              return (
                <div
                  key={tk.id}
                  className="octoheft-subcard rounded-xl border border-slate-200 dark:border-slate-700 bg-slate-50/80 dark:bg-slate-800/80 p-3.5 space-y-2.5"
                >
                  <div className="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-2">
                    <div className="min-w-0">
                      <div className="text-xs font-semibold text-slate-900 dark:text-slate-100 truncate">
                        {tk.file_name}
                      </div>
                      <div className="text-[11px] font-mono text-slate-600 dark:text-slate-300 flex flex-wrap gap-3 mt-0.5">
                        <span>{t(`status.${tk.status}` as Parameters<typeof t>[0])}</span>
                        {tk.media_info?.video_resolution && (
                          <span>{tk.media_info.video_resolution}</span>
                        )}
                        <span>
                          {formatBytes(tk.downloaded_bytes)}
                          {tk.total_size > 0 ? ` / ${formatBytes(tk.total_size)}` : ''} ({pct}%)
                        </span>
                        {isRunning && <span className="tabular-nums">{formatSpeed(tk.speed, language)}</span>}
                        {isRunning && tk.eta_seconds !== null && (
                          <span className="tabular-nums">{formatEta(tk.eta_seconds)}</span>
                        )}
                      </div>
                    </div>

                    <div className="flex items-center gap-2 shrink-0">
                      {isRunning ? (
                        <button
                          type="button"
                          onClick={() => onPauseTask(tk.id)}
                          className="min-h-[36px] inline-flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs font-semibold bg-amber-50 dark:bg-amber-950/70 text-amber-800 dark:text-amber-300 border border-amber-200 dark:border-amber-500/40 hover:bg-amber-100 dark:hover:bg-amber-900/70 focus-visible:outline focus-visible:outline-2 focus-visible:outline-sky-600 focus-visible:outline-offset-2"
                        >
                          <Pause className="h-3.5 w-3.5" aria-hidden="true" />
                          <span>
                            {t('detailModal.pause')}
                            <span className="visually-hidden"> {tk.file_name}</span>
                          </span>
                        </button>
                      ) : tk.status === 'paused' || tk.status === 'error' ? (
                        <button
                          type="button"
                          onClick={() => onResumeTask(tk.id)}
                          className="min-h-[36px] inline-flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs font-semibold bg-sky-50 dark:bg-sky-950/80 text-sky-800 dark:text-cyan-300 border border-sky-200 dark:border-cyan-500/40 hover:bg-sky-100 dark:hover:bg-sky-900/80 focus-visible:outline focus-visible:outline-2 focus-visible:outline-sky-600 focus-visible:outline-offset-2"
                        >
                          <Play className="h-3.5 w-3.5" aria-hidden="true" />
                          <span>
                            {t('detailModal.resume')}
                            <span className="visually-hidden"> {tk.file_name}</span>
                          </span>
                        </button>
                      ) : null}

                      <button
                        type="button"
                        onClick={() => onInspectTask(tk.id)}
                        className="octoheft-subcard min-h-[36px] inline-flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs font-medium bg-white dark:bg-slate-800 text-slate-800 dark:text-slate-200 border border-slate-200 dark:border-slate-700 hover:bg-slate-100 dark:hover:bg-slate-700 focus-visible:outline focus-visible:outline-2 focus-visible:outline-sky-600 focus-visible:outline-offset-2"
                      >
                        <ExternalLink className="h-3.5 w-3.5 text-sky-600 dark:text-cyan-400" aria-hidden="true" />
                        <span>
                          {t('videoAddon.viewDetailsBtn')}
                          <span className="visually-hidden"> {tk.file_name}</span>
                        </span>
                      </button>

                      <button
                        type="button"
                        onClick={() => onOpenFolder(tk.save_path)}
                        className="octoheft-subcard min-h-[36px] inline-flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs font-medium bg-white dark:bg-slate-800 text-slate-800 dark:text-slate-200 border border-slate-200 dark:border-slate-700 hover:bg-slate-100 dark:hover:bg-slate-700 focus-visible:outline focus-visible:outline-2 focus-visible:outline-sky-600 focus-visible:outline-offset-2"
                      >
                        <FolderOpen className="h-3.5 w-3.5 text-amber-600 dark:text-amber-400" aria-hidden="true" />
                        <span>
                          {t('videoAddon.openFolderBtn')}
                          <span className="visually-hidden"> {tk.file_name}</span>
                        </span>
                      </button>
                    </div>
                  </div>

                  <progress
                    value={pct}
                    max={100}
                    aria-label={`${tk.file_name} (${pct}%)`}
                    className="octoheft-disk-progress w-full"
                  />
                </div>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
};
