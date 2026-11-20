import React, { useEffect, useState, useMemo, useRef, useCallback } from 'react';
import {
  AppSettings,
  ClipboardUrlDetectedPayload,
  DiskSpaceStats,
  DownloadCategory,
  DownloadProgressPayload,
  DownloadStatus,
  DownloadTask,
  AddDownloadRequest,
  QueueConfig,
  BrowserIntegrationReport,
  VideoGrabberPayload,
} from './types';
import { Sidebar } from './components/Sidebar';
import { Header } from './components/Header';
import { DownloadTable } from './components/DownloadTable';
import { VideoDownloaderAddon } from './components/VideoDownloaderAddon';
import { AddDownloadModal } from './components/AddDownloadModal';
import { DownloadDetailModal } from './components/DownloadDetailModal';
import { BatchDownloadModal } from './components/BatchDownloadModal';
import { SchedulerModal } from './components/SchedulerModal';
import { ZipPreviewModal } from './components/ZipPreviewModal';
import { SettingsModal } from './components/SettingsModal';
import { BrowserIntegrationModal } from './components/BrowserIntegrationModal';

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useI18n } from './i18n';
import { useTheme } from './hooks/useTheme';
import { playCompletionChime } from './utils';

function normalizeTask(raw: DownloadTask): DownloadTask {
  return {
    ...raw,
    id: String(raw?.id || ''),
    url: String(raw?.url || ''),
    file_name: String(raw?.file_name || 'download.bin'),
    save_path: String(raw?.save_path || ''),
    total_size: Number(raw?.total_size) || 0,
    downloaded_bytes: Number(raw?.downloaded_bytes) || 0,
    speed: Number(raw?.speed) || 0,
    eta_seconds: raw?.eta_seconds ?? null,
    status: raw?.status || 'queued',
    category: raw?.category || 'other',
    supports_range: Boolean(raw?.supports_range),
    connections: Number(raw?.connections) || 1,
    segments: Array.isArray(raw?.segments) ? raw.segments : [],
  };
}

export const App: React.FC = () => {
  const { t } = useI18n();
  const { theme, cycleTheme } = useTheme();
  const [tasks, setTasks] = useState<DownloadTask[]>([]);
  const [queues, setQueues] = useState<QueueConfig[]>([]);
  const [appSettings, setAppSettings] = useState<AppSettings>({
    language: 'en',
    intercept_extensions: '',
    sound_on_complete: true,
    default_download_dir: '',
    use_category_subfolders: true,
    max_connections_per_download: 8,
  });
  const [currentCategory, setCurrentCategory] = useState<DownloadCategory>('all');
  const [currentStatus, setCurrentStatus] = useState<string>('all');
  const [activeView, setActiveView] = useState<'downloads' | 'octomedia'>('downloads');
  const [hasOpenedOctoMedia, setHasOpenedOctoMedia] = useState(false);
  const [videoGrabSignal, setVideoGrabSignal] = useState<{
    payload: VideoGrabberPayload;
    seq: number;
  } | null>(null);
  const [searchQuery, setSearchQuery] = useState('');
  const [isAddModalOpen, setIsAddModalOpen] = useState(false);
  const [prefillUrl, setPrefillUrl] = useState('');
  const [prefillFileName, setPrefillFileName] = useState('');
  const [prefillFromClipboard, setPrefillFromClipboard] = useState(false);
  const [prefillIsMedia, setPrefillIsMedia] = useState(false);
  const [clipboardMonitorActive, setClipboardMonitorActive] = useState(false);
  const [isBatchModalOpen, setIsBatchModalOpen] = useState(false);
  const [isSchedulerModalOpen, setIsSchedulerModalOpen] = useState(false);
  const [isZipModalOpen, setIsZipModalOpen] = useState(false);
  const [zipPreviewUrl, setZipPreviewUrl] = useState('');
  const [isSettingsModalOpen, setIsSettingsModalOpen] = useState(false);
  const [isBrowserModalOpen, setIsBrowserModalOpen] = useState(false);
  const [inspectingTaskId, setInspectingTaskId] = useState<string | null>(null);
  const [openWhenFinishedIds, setOpenWhenFinishedIds] = useState<Record<string, boolean>>({});
  const [autoCloseOnFinish, setAutoCloseOnFinish] = useState<boolean>(false);
  const [diskStats, setDiskStats] = useState<DiskSpaceStats | null>(null);
  const [sessionDownloadedBytes, setSessionDownloadedBytes] = useState<number>(0);

  const initialTasksLoadedRef = useRef(false);
  const grabSeqRef = useRef(0);
  const prevStatusMapRef = useRef<Map<string, DownloadStatus>>(new Map());
  const handledCompletionIdsRef = useRef<Set<string>>(new Set());
  const sessionTrackerRef = useRef<Map<string, { baseline: number; peak: number }>>(new Map());

  const refreshDiskStats = useCallback(async (overrideDir?: string) => {
    try {
      const trimmed = (overrideDir ?? '').trim();
      const stats = await invoke<DiskSpaceStats>('get_disk_space_stats', {
        targetDir: trimmed ? trimmed : null,
      });
      if (stats && typeof stats === 'object') {
        setDiskStats(stats);
      }
    } catch (err) {
      console.error('Failed to query disk space stats:', err);
    }
  }, []);

  const triggerVideoGrab = (payload: VideoGrabberPayload) => {
    if (!payload || !payload.url) return;
    grabSeqRef.current += 1;
    setHasOpenedOctoMedia(true);
    setActiveView('octomedia');
    setVideoGrabSignal({ payload, seq: grabSeqRef.current });
  };

  useEffect(() => {
    if (activeView === 'octomedia') {
      setHasOpenedOctoMedia(true);
    }
  }, [activeView]);

  const refreshTasks = async () => {
    try {
      const list = await invoke<DownloadTask[]>('get_tasks');
      if (Array.isArray(list)) {
        const normalized = list.filter((tk) => tk && tk.id).map(normalizeTask);
        if (!initialTasksLoadedRef.current) {
          for (const tk of normalized) {
            prevStatusMapRef.current.set(tk.id, tk.status);
            if (tk.status === 'completed') {
              handledCompletionIdsRef.current.add(tk.id);
            }
            if (!sessionTrackerRef.current.has(tk.id)) {
              sessionTrackerRef.current.set(tk.id, {
                baseline: tk.downloaded_bytes,
                peak: tk.downloaded_bytes,
              });
            }
          }
          initialTasksLoadedRef.current = true;
        }
        setTasks(normalized);
      }
    } catch (err) {
      console.error('Failed to load tasks:', err);
    }
  };

  const refreshQueues = async () => {
    try {
      const list = await invoke<QueueConfig[]>('get_queues');
      if (Array.isArray(list)) {
        setQueues(list);
      }
    } catch (err) {
      console.error('Failed to load queues:', err);
    }
  };

  const refreshSettings = async () => {
    try {
      const s = await invoke<AppSettings>('get_app_settings');
      if (s && typeof s === 'object') {
        setAppSettings((prev) => ({ ...prev, ...s }));
        if (typeof s.clipboard_monitor_enabled === 'boolean') {
          setClipboardMonitorActive(s.clipboard_monitor_enabled);
        }
      }
    } catch (err) {
      console.error('Failed to load app settings:', err);
    }
  };

  const openQuickPasteModal = async (passedUrl?: string) => {
    let candidate = (passedUrl || '').trim();
    if (!candidate.startsWith('http://') && !candidate.startsWith('https://')) {
      try {
        const winClip = await invoke<string | null>('get_clipboard_url');
        if (winClip) {
          candidate = winClip.trim();
        }
      } catch {
        try {
          const text = (await navigator.clipboard.readText()).trim();
          if (text.startsWith('http://') || text.startsWith('https://')) {
            candidate = text;
          }
        } catch {
          // Ignore clipboard read permission issues
        }
      }
    }

    setPrefillFromClipboard(false);
    setPrefillIsMedia(false);
    setPrefillFileName('');
    setPrefillUrl(candidate);
    setIsAddModalOpen(true);
  };

  const handleClipboardMonitorChanged = useCallback((enabled: boolean) => {
    setClipboardMonitorActive(enabled);
  }, []);

  useEffect(() => {
    let active = true;
    refreshTasks();
    refreshQueues();
    refreshSettings();

    invoke<boolean>('get_clipboard_monitor_status')
      .then((enabled) => {
        if (active) {
          setClipboardMonitorActive(Boolean(enabled));
        }
      })
      .catch(() => {});

    invoke<BrowserIntegrationReport>('get_browser_integration_status')
      .then((report) => {
        if (active && report && !report.onboarding_completed) {
          setIsBrowserModalOpen(true);
        }
      })
      .catch(console.error);

    let unlistenProgress: (() => void) | null = null;
    let unlistenHotkey: (() => void) | null = null;
    let unlistenNativeTask: (() => void) | null = null;
    let unlistenOpenProgress: (() => void) | null = null;
    let unlistenSettings: (() => void) | null = null;
    let unlistenGrab: (() => void) | null = null;
    let unlistenClipboard: (() => void) | null = null;

    invoke<VideoGrabberPayload | null>('take_pending_video_grab')
      .then((pending) => {
        if (active && pending && pending.url) {
          triggerVideoGrab(pending);
        }
      })
      .catch(console.error);

    listen('download-progress', (event: { payload: DownloadProgressPayload }) => {
      const payload = event.payload;
      if (!payload || !payload.id) return;
      setTasks((prev) =>
        prev.map((tk) => {
          if (tk.id === payload.id) {
            return {
              ...tk,
              status: payload.status || tk.status,
              downloaded_bytes: payload.downloaded_bytes ?? tk.downloaded_bytes,
              total_size: payload.total_size > 0 ? payload.total_size : tk.total_size,
              speed: payload.speed ?? 0,
              eta_seconds: payload.eta_seconds ?? null,
              segments: Array.isArray(payload.segments) ? payload.segments : tk.segments,
              error_message:
                payload.error_message !== undefined ? payload.error_message : tk.error_message,
              media_info: payload.media_info !== undefined ? payload.media_info : tk.media_info,
            };
          }
          return tk;
        })
      );
    })
      .then((unlisten) => {
        if (!active) unlisten();
        else unlistenProgress = unlisten;
      })
      .catch(console.error);

    listen<string>('global-quick-paste', (event) => {
      openQuickPasteModal(event.payload);
    })
      .then((unlisten) => {
        if (!active) unlisten();
        else unlistenHotkey = unlisten;
      })
      .catch(console.error);

    listen<DownloadTask>('tasks-updated', (event) => {
      const updatedTask = event.payload;
      if (updatedTask && updatedTask.id) {
        const normalized = normalizeTask(updatedTask);
        if (!sessionTrackerRef.current.has(normalized.id)) {
          sessionTrackerRef.current.set(normalized.id, {
            baseline: 0,
            peak: normalized.downloaded_bytes,
          });
        }
        setTasks((prev) => {
          const idx = prev.findIndex((tk) => tk.id === normalized.id);
          if (idx >= 0) {
            const copy = [...prev];
            copy[idx] = normalized;
            return copy;
          }
          return [normalized, ...prev];
        });
      } else {
        refreshTasks();
      }
    })
      .then((unlisten) => {
        if (!active) unlisten();
        else unlistenNativeTask = unlisten;
      })
      .catch(console.error);

    listen<string>('open-progress-dialog', (event) => {
      const taskId = event.payload;
      if (taskId) {
        refreshTasks();
        setInspectingTaskId(taskId);
      }
    })
      .then((unlisten) => {
        if (!active) unlisten();
        else unlistenOpenProgress = unlisten;
      })
      .catch(console.error);

    listen<AppSettings>('settings-updated', (event) => {
      if (event.payload) {
        setAppSettings((prev) => ({ ...prev, ...event.payload }));
        if (typeof event.payload.clipboard_monitor_enabled === 'boolean') {
          setClipboardMonitorActive(event.payload.clipboard_monitor_enabled);
        }
      }
    })
      .then((unlisten) => {
        if (!active) unlisten();
        else unlistenSettings = unlisten;
      })
      .catch(console.error);

    listen<VideoGrabberPayload>('open-video-grabber', (event) => {
      if (event.payload && event.payload.url) {
        triggerVideoGrab(event.payload);
      }
    })
      .then((unlisten) => {
        if (!active) unlisten();
        else unlistenGrab = unlisten;
      })
      .catch(console.error);

    listen<ClipboardUrlDetectedPayload>('clipboard-url-detected', (event) => {
      const payload = event.payload;
      if (!payload || !payload.url) return;

      try {
        if ('__TAURI_INTERNALS__' in window) {
          const win = getCurrentWindow();
          win.unminimize().catch(() => {});
          win.show().catch(() => {});
          win.setFocus().catch(() => {});
        }
      } catch {
        // Ignore outside native Tauri runtime
      }

      if (payload.is_media) {
        setIsAddModalOpen(false);
        setPrefillUrl('');
        setPrefillFileName('');
        setPrefillFromClipboard(false);
        setPrefillIsMedia(false);
        triggerVideoGrab({ url: payload.url });
        return;
      }

      setPrefillUrl(payload.url);
      setPrefillFileName(payload.filename || '');
      setPrefillFromClipboard(true);
      setPrefillIsMedia(false);
      setIsAddModalOpen(true);
    })
      .then((unlisten) => {
        if (!active) unlisten();
        else unlistenClipboard = unlisten;
      })
      .catch(console.error);

    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.altKey && (e.key === 'd' || e.key === 'D')) {
        e.preventDefault();
        openQuickPasteModal();
      }
    };
    window.addEventListener('keydown', handleKeyDown);

    return () => {
      active = false;
      if (unlistenProgress) unlistenProgress();
      if (unlistenHotkey) unlistenHotkey();
      if (unlistenNativeTask) unlistenNativeTask();
      if (unlistenOpenProgress) unlistenOpenProgress();
      if (unlistenSettings) unlistenSettings();
      if (unlistenGrab) unlistenGrab();
      if (unlistenClipboard) unlistenClipboard();
      window.removeEventListener('keydown', handleKeyDown);
    };
  }, []);

  // Detect live completion transitions -> completion chime, auto-open file, auto-close dialog
  useEffect(() => {
    let anyCompletedNow = false;
    for (const tk of tasks) {
      const prevStatus = prevStatusMapRef.current.get(tk.id);
      if (tk.status !== 'completed') {
        handledCompletionIdsRef.current.delete(tk.id);
      } else if (
        prevStatus &&
        prevStatus !== 'completed' &&
        !handledCompletionIdsRef.current.has(tk.id)
      ) {
        handledCompletionIdsRef.current.add(tk.id);
        anyCompletedNow = true;

        if (appSettings.sound_on_complete) {
          playCompletionChime();
        }
        if (openWhenFinishedIds[tk.id]) {
          invoke('open_file', { path: tk.save_path }).catch(console.error);
        }
        if (autoCloseOnFinish && inspectingTaskId === tk.id) {
          setInspectingTaskId(null);
        }
      }
      prevStatusMapRef.current.set(tk.id, tk.status);
    }
    if (anyCompletedNow) {
      refreshDiskStats(appSettings.default_download_dir);
    }
  }, [
    tasks,
    appSettings.sound_on_complete,
    appSettings.default_download_dir,
    openWhenFinishedIds,
    autoCloseOnFinish,
    inspectingTaskId,
    refreshDiskStats,
  ]);

  // Refresh live disk space stats on startup, folder setting change, and active downloads
  useEffect(() => {
    refreshDiskStats(appSettings.default_download_dir);
  }, [appSettings.default_download_dir, appSettings.use_category_subfolders, refreshDiskStats]);

  // Update active session downloaded bytes counter across progress updates
  useEffect(() => {
    const tracker = sessionTrackerRef.current;
    for (const tk of tasks) {
      const existing = tracker.get(tk.id);
      if (!existing) {
        tracker.set(tk.id, {
          baseline: initialTasksLoadedRef.current ? 0 : tk.downloaded_bytes,
          peak: tk.downloaded_bytes,
        });
      } else if (tk.downloaded_bytes > existing.peak) {
        existing.peak = tk.downloaded_bytes;
      }
    }
    let total = 0;
    for (const entry of tracker.values()) {
      total += Math.max(0, entry.peak - entry.baseline);
    }
    setSessionDownloadedBytes(total);
  }, [tasks]);

  const filteredTasks = useMemo(() => {
    return tasks.filter((tk) => {
      if (!tk) return false;
      if (currentCategory !== 'all' && tk.category !== currentCategory) {
        return false;
      }
      if (currentStatus !== 'all' && tk.status !== currentStatus) {
        return false;
      }
      if (searchQuery.trim()) {
        const q = searchQuery.toLowerCase();
        return (
          (tk.file_name || '').toLowerCase().includes(q) ||
          (tk.url || '').toLowerCase().includes(q)
        );
      }
      return true;
    });
  }, [tasks, currentCategory, currentStatus, searchQuery]);

  const totalSpeed = useMemo(() => {
    return tasks
      .filter((tk) => tk.status === 'downloading')
      .reduce((acc, tk) => acc + (tk.speed || 0), 0);
  }, [tasks]);

  const activeCount = useMemo(() => {
    return tasks.filter((tk) => tk.status === 'downloading').length;
  }, [tasks]);

  useEffect(() => {
    if (activeCount <= 0) return;
    const intervalId = window.setInterval(() => {
      refreshDiskStats(appSettings.default_download_dir);
    }, 15000);
    return () => window.clearInterval(intervalId);
  }, [activeCount, appSettings.default_download_dir, refreshDiskStats]);

  const activeDetailTask = useMemo(() => {
    if (!inspectingTaskId) return null;
    return tasks.find((tk) => tk.id === inspectingTaskId) || null;
  }, [tasks, inspectingTaskId]);

  const handleAddDownload = async (req: AddDownloadRequest, startImmediately: boolean) => {
    try {
      const newTask = await invoke<DownloadTask>('add_download', {
        request: req,
        startImmediately,
      });
      if (newTask && newTask.id) {
        const normalized = normalizeTask(newTask);
        prevStatusMapRef.current.set(normalized.id, normalized.status);
        if (!sessionTrackerRef.current.has(normalized.id)) {
          sessionTrackerRef.current.set(normalized.id, {
            baseline: 0,
            peak: normalized.downloaded_bytes,
          });
        }
        setTasks((prev) => [normalized, ...prev]);
        if (startImmediately) {
          setInspectingTaskId(normalized.id);
        }
      }
    } catch (err) {
      alert(t('error.download', { error: String(err) }));
    }
  };

  const handlePause = async (id: string) => {
    try {
      await invoke('pause_download', { id });
      setTasks((prev) =>
        prev.map((tk) => (tk.id === id ? { ...tk, status: 'paused', speed: 0 } : tk))
      );
    } catch (err) {
      console.error(err);
    }
  };

  const handleResume = async (id: string) => {
    try {
      await invoke('resume_download', { id });
      setTasks((prev) =>
        prev.map((tk) => (tk.id === id ? { ...tk, status: 'downloading' } : tk))
      );
    } catch (err) {
      console.error(err);
    }
  };

  const handleDelete = async (id: string) => {
    try {
      await invoke('delete_download', { id, deleteFile: false });
      setTasks((prev) => prev.filter((tk) => tk.id !== id));
      if (inspectingTaskId === id) {
        setInspectingTaskId(null);
      }
    } catch (err) {
      console.error(err);
    }
  };

  const handleCancelActiveTask = async (id: string) => {
    try {
      await invoke('pause_download', { id }).catch(() => {});
      await handleDelete(id);
    } catch (err) {
      console.error(err);
    }
  };

  const handleOpenFile = async (path: string) => {
    try {
      await invoke('open_file', { path });
    } catch (err) {
      console.error(err);
    }
  };

  const handleOpenFolder = async (path: string) => {
    try {
      await invoke('open_folder', { path });
    } catch (err) {
      console.error(err);
    }
  };

  const handleSetGlobalSpeedLimit = async (limit: number) => {
    try {
      await invoke('set_speed_limit', { id: null, limitBytesPerSec: limit });
    } catch (err) {
      console.error(err);
    }
  };

  const handleResumeAll = () => {
    tasks
      .filter((tk) => tk.status === 'paused' || tk.status === 'queued')
      .forEach((tk) => handleResume(tk.id));
  };

  const handlePauseAll = () => {
    tasks
      .filter((tk) => tk.status === 'downloading')
      .forEach((tk) => handlePause(tk.id));
  };

  const handleSaveQueue = async (q: QueueConfig) => {
    try {
      await invoke('save_queue', { config: q });
      refreshQueues();
    } catch (err) {
      console.error('Failed to save queue:', err);
    }
  };

  const handleDeleteQueue = async (id: string) => {
    try {
      await invoke('delete_queue', { id });
      refreshQueues();
    } catch (err) {
      console.error('Failed to delete queue:', err);
    }
  };

  const handleBatchSubmit = async (
    urls: string[],
    queueId: string | null,
    startImmediately: boolean
  ) => {
    try {
      const added = await invoke<DownloadTask[]>('add_batch_downloads', {
        urls,
        queueId,
        startImmediately,
      });
      if (Array.isArray(added)) {
        const normalized = added.filter((tk) => tk && tk.id).map(normalizeTask);
        for (const tk of normalized) {
          if (!sessionTrackerRef.current.has(tk.id)) {
            sessionTrackerRef.current.set(tk.id, {
              baseline: 0,
              peak: tk.downloaded_bytes,
            });
          }
        }
        setTasks((prev) => [...normalized, ...prev]);
        refreshQueues();
      }
    } catch (err) {
      alert(t('error.batch', { error: String(err) }));
    }
  };

  const handleOpenZipPreview = (url?: string) => {
    if (url) {
      setZipPreviewUrl(url);
    }
    setIsZipModalOpen(true);
  };

  const handleReorderTask = async (id: string, newIndex: number) => {
    try {
      await invoke('reorder_task', { id, newIndex });
      refreshTasks();
    } catch (err) {
      console.error('Reorder failed:', err);
    }
  };

  const handleMoveTaskUp = async (id: string) => {
    try {
      await invoke('move_task_up', { id });
      refreshTasks();
    } catch (err) {
      console.error('Move up failed:', err);
    }
  };

  const handleMoveTaskDown = async (id: string) => {
    try {
      await invoke('move_task_down', { id });
      refreshTasks();
    } catch (err) {
      console.error('Move down failed:', err);
    }
  };

  const handleStartMediaStream = async (url: string) => {
    try {
      const newTask = await invoke<DownloadTask>('start_media_stream_download', {
        url,
        savePath: null,
      });
      if (newTask && newTask.id) {
        const normalized = normalizeTask(newTask);
        prevStatusMapRef.current.set(normalized.id, normalized.status);
        if (!sessionTrackerRef.current.has(normalized.id)) {
          sessionTrackerRef.current.set(normalized.id, {
            baseline: 0,
            peak: normalized.downloaded_bytes,
          });
        }
        setTasks((prev) => [normalized, ...prev]);
        setInspectingTaskId(normalized.id);
      }
    } catch (err) {
      alert(t('error.mediaRemux', { error: String(err) }));
    }
  };

  return (
    <div className="bg-app flex h-screen w-screen bg-gradient-to-br from-slate-50 via-sky-50/40 to-slate-100 dark:from-slate-950 dark:via-slate-900 dark:to-slate-950 text-slate-900 dark:text-slate-100 overflow-hidden font-sans">
      {/* Category & Status Sidebar */}
      <Sidebar
        activeView={activeView}
        onSelectView={setActiveView}
        currentCategory={currentCategory}
        onSelectCategory={setCurrentCategory}
        currentStatus={currentStatus}
        onSelectStatus={setCurrentStatus}
        tasks={tasks}
        diskStats={diskStats}
        sessionDownloadedBytes={sessionDownloadedBytes}
      />

      {/* Main Panel */}
      <main className="octoheft-main flex-1 flex flex-col min-w-0 bg-white/40 dark:bg-transparent">
        <Header
          totalSpeed={totalSpeed}
          activeCount={activeCount}
          theme={theme}
          onCycleTheme={cycleTheme}
          clipboardMonitorActive={clipboardMonitorActive}
          onClipboardMonitorChanged={handleClipboardMonitorChanged}
          onOpenAddModal={() => {
            setPrefillUrl('');
            setPrefillFileName('');
            setPrefillFromClipboard(false);
            setPrefillIsMedia(false);
            setIsAddModalOpen(true);
          }}
          onOpenBatchModal={() => setIsBatchModalOpen(true)}
          onOpenSchedulerModal={() => setIsSchedulerModalOpen(true)}
          onOpenZipPreviewModal={() => handleOpenZipPreview('')}
          onOpenSettingsModal={() => setIsSettingsModalOpen(true)}
          onResumeAll={handleResumeAll}
          onPauseAll={handlePauseAll}
          onSetGlobalSpeedLimit={handleSetGlobalSpeedLimit}
          searchQuery={searchQuery}
          onSearchChange={setSearchQuery}
        />

        {hasOpenedOctoMedia && (
          <div
            className={
              activeView === 'octomedia'
                ? 'flex-1 min-w-0 min-h-0 flex flex-col overflow-hidden'
                : 'hidden'
            }
            aria-hidden={activeView !== 'octomedia'}
          >
            <VideoDownloaderAddon
              grabSignal={videoGrabSignal}
              appSettings={appSettings}
              tasks={tasks}
              onClearMedia={() => setVideoGrabSignal(null)}
              onTaskStarted={(tk) => {
                const normalized = normalizeTask(tk);
                prevStatusMapRef.current.set(normalized.id, normalized.status);
                if (!sessionTrackerRef.current.has(normalized.id)) {
                  sessionTrackerRef.current.set(normalized.id, {
                    baseline: 0,
                    peak: normalized.downloaded_bytes,
                  });
                }
                setTasks((prev) => [normalized, ...prev.filter((x) => x.id !== normalized.id)]);
              }}
              onInspectTask={(id) => setInspectingTaskId(id)}
              onPauseTask={handlePause}
              onResumeTask={handleResume}
              onOpenFolder={handleOpenFolder}
            />
          </div>
        )}

        {activeView !== 'octomedia' && (
          /* Download Table */
          <div className="flex-1 min-w-0 min-h-0 max-w-full box-border overflow-hidden flex flex-col p-4 lg:p-5">
            <div className="card-surface flex-1 min-w-0 min-h-0 max-w-full rounded-2xl border border-slate-200/90 dark:border-slate-800 bg-white/85 dark:bg-slate-900/85 backdrop-blur-md overflow-hidden flex flex-col shadow-sm">
              <DownloadTable
                tasks={filteredTasks}
                onSelectTask={(task) => setInspectingTaskId(task.id)}
                onPause={handlePause}
                onResume={handleResume}
                onDelete={handleDelete}
                onOpenFile={handleOpenFile}
                onOpenFolder={handleOpenFolder}
                onReorderTask={handleReorderTask}
                onMoveTaskUp={handleMoveTaskUp}
                onMoveTaskDown={handleMoveTaskDown}
              />
            </div>
          </div>
        )}
      </main>

      {/* Add Download Modal */}
      <AddDownloadModal
        isOpen={isAddModalOpen}
        initialUrl={prefillUrl}
        initialFileName={prefillFileName}
        defaultDownloadDir={appSettings.default_download_dir}
        defaultConnections={appSettings.max_connections_per_download || 8}
        capturedFromClipboard={prefillFromClipboard}
        isMediaUrl={prefillIsMedia}
        onClose={() => {
          setIsAddModalOpen(false);
          setPrefillUrl('');
          setPrefillFileName('');
          setPrefillFromClipboard(false);
          setPrefillIsMedia(false);
        }}
        onSubmit={handleAddDownload}
        onInspectZip={(u) => {
          setIsAddModalOpen(false);
          setPrefillUrl('');
          setPrefillFileName('');
          setPrefillFromClipboard(false);
          setPrefillIsMedia(false);
          handleOpenZipPreview(u);
        }}
        onStartMediaStream={handleStartMediaStream}
        onOpenInOctoMedia={(u) => {
          setIsAddModalOpen(false);
          setPrefillUrl('');
          setPrefillFileName('');
          setPrefillFromClipboard(false);
          setPrefillIsMedia(false);
          triggerVideoGrab({ url: u });
        }}
      />

      {/* Batch Pattern Modal */}
      <BatchDownloadModal
        isOpen={isBatchModalOpen}
        onClose={() => setIsBatchModalOpen(false)}
        queues={queues}
        onSubmit={handleBatchSubmit}
      />

      {/* Scheduler & Queue Modal */}
      <SchedulerModal
        isOpen={isSchedulerModalOpen}
        onClose={() => setIsSchedulerModalOpen(false)}
        queues={queues}
        onSaveQueue={handleSaveQueue}
        onDeleteQueue={handleDeleteQueue}
      />

      {/* Remote ZIP Preview & Selective Extraction Modal */}
      <ZipPreviewModal
        isOpen={isZipModalOpen}
        initialUrl={zipPreviewUrl}
        onClose={() => {
          setIsZipModalOpen(false);
          setZipPreviewUrl('');
        }}
        onDownloadFull={(fullUrl) => {
          setIsZipModalOpen(false);
          setZipPreviewUrl('');
          handleAddDownload({ url: fullUrl, connections: 8 }, true);
        }}
      />

      {/* Simplified 4-Tab Settings Modal */}
      <SettingsModal
        isOpen={isSettingsModalOpen}
        onClose={() => setIsSettingsModalOpen(false)}
        onSettingsSaved={(saved) => setAppSettings(saved)}
        onOpenBrowserWizard={() => {
          setIsSettingsModalOpen(false);
          setIsBrowserModalOpen(true);
        }}
      />

      {/* First-Run / Interactive Browser Integration Modal */}
      <BrowserIntegrationModal
        isOpen={isBrowserModalOpen}
        onClose={() => setIsBrowserModalOpen(false)}
      />

      {/* Standalone IDM-Style Download Progress Dialog */}
      {activeDetailTask && (
        <DownloadDetailModal
          task={activeDetailTask}
          openOnFinish={!!openWhenFinishedIds[activeDetailTask.id]}
          onToggleOpenOnFinish={(checked) =>
            setOpenWhenFinishedIds((prev) => ({ ...prev, [activeDetailTask.id]: checked }))
          }
          autoCloseOnFinish={autoCloseOnFinish}
          onToggleAutoCloseOnFinish={setAutoCloseOnFinish}
          onClose={() => setInspectingTaskId(null)}
          onCancelTask={handleCancelActiveTask}
          onPause={handlePause}
          onResume={handleResume}
          onOpenFile={handleOpenFile}
          onOpenFolder={handleOpenFolder}
        />
      )}
    </div>
  );
};

export default App;
