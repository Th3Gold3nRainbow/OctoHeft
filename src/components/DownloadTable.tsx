import React, { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { DownloadTask } from '../types';
import { formatBytes, formatEta, formatSpeed } from '../utils';
import { useI18n } from '../i18n';
import {
  FolderArchive,
  Terminal,
  Video,
  FileText,
  Music,
  File,
  Play,
  Pause,
  Trash2,
  ExternalLink,
  Folder,
  BarChart2,
  AlertCircle,
  CheckCircle2,
  Clock,
  ChevronUp,
  ChevronDown,
  GripVertical,
} from 'lucide-react';

interface DownloadTableProps {
  tasks: DownloadTask[];
  onSelectTask: (task: DownloadTask) => void;
  onPause: (id: string) => void;
  onResume: (id: string) => void;
  onDelete: (id: string) => void;
  onOpenFile: (path: string) => void;
  onOpenFolder: (path: string) => void;
  onReorderTask?: (id: string, newIndex: number) => void;
  onMoveTaskUp?: (id: string) => void;
  onMoveTaskDown?: (id: string) => void;
}

export const DownloadTable: React.FC<DownloadTableProps> = ({
  tasks,
  onSelectTask,
  onPause,
  onResume,
  onDelete,
  onOpenFile,
  onOpenFolder,
  onReorderTask,
  onMoveTaskUp,
  onMoveTaskDown,
}) => {
  const { t, language } = useI18n();
  const [draggedTaskId, setDraggedTaskId] = useState<string | null>(null);
  const [dragOverIndex, setDragOverIndex] = useState<number | null>(null);
  const tableRef = useRef<HTMLTableElement | null>(null);
  const outboundDragInFlightRef = useRef(false);

  useEffect(() => {
    return () => {
      document.documentElement.classList.remove('is-file-dragging');
    };
  }, []);

  const startOutboundFileDrag = (e: React.DragEvent, task: DownloadTask) => {
    if (task.status !== 'completed' || !task.save_path) return;
    e.preventDefault();
    e.stopPropagation();
    if (outboundDragInFlightRef.current) return;
    outboundDragInFlightRef.current = true;
    document.documentElement.classList.add('is-file-dragging');
    invoke('start_file_drag', { filePath: task.save_path })
      .catch((err) => {
        console.error('Failed starting Windows outbound file drag:', err);
      })
      .finally(() => {
        outboundDragInFlightRef.current = false;
        document.documentElement.classList.remove('is-file-dragging');
      });
  };

  const getCategoryIcon = (category: string) => {
    switch (category) {
      case 'compressed':
        return <FolderArchive className="h-4 w-4 text-amber-600 dark:text-amber-400" />;
      case 'programs':
        return <Terminal className="h-4 w-4 text-sky-600 dark:text-cyan-400" />;
      case 'video':
        return <Video className="h-4 w-4 text-indigo-600 dark:text-indigo-400" />;
      case 'documents':
        return <FileText className="h-4 w-4 text-blue-600 dark:text-blue-400" />;
      case 'audio':
        return <Music className="h-4 w-4 text-teal-600 dark:text-teal-400" />;
      default:
        return <File className="h-4 w-4 text-slate-500 dark:text-slate-400" />;
    }
  };

  const getStatusBadge = (status: string, error?: string) => {
    switch (status) {
      case 'downloading':
        return (
          <span data-status="downloading" className="inline-flex items-center gap-1 max-w-full px-2 py-0.5 rounded-full text-[11px] font-semibold bg-sky-50 dark:bg-sky-950/80 text-sky-700 dark:text-cyan-300 border border-sky-200 dark:border-cyan-500/40">
            <span className="h-1.5 w-1.5 rounded-full bg-sky-500 dark:bg-cyan-400 animate-ping shrink-0" />
            <span className="truncate">{t('status.downloading')}</span>
          </span>
        );
      case 'connecting':
        return (
          <span data-status="connecting" className="inline-flex items-center gap-1 max-w-full px-2 py-0.5 rounded-full text-[11px] font-semibold bg-cyan-50 dark:bg-cyan-950/80 text-cyan-700 dark:text-cyan-300 border border-cyan-200 dark:border-cyan-500/40">
            <Clock className="h-3 w-3 animate-spin shrink-0" />
            <span className="truncate">{t('status.connecting')}</span>
          </span>
        );
      case 'completed':
        return (
          <span data-status="completed" className="inline-flex items-center gap-1 max-w-full px-2 py-0.5 rounded-full text-[11px] font-semibold bg-emerald-50 dark:bg-emerald-950/70 text-emerald-700 dark:text-emerald-300 border border-emerald-200 dark:border-emerald-500/40">
            <CheckCircle2 className="h-3 w-3 shrink-0" />
            <span className="truncate">{t('status.completed')}</span>
          </span>
        );
      case 'paused':
        return (
          <span data-status="paused" className="inline-flex items-center gap-1 max-w-full px-2 py-0.5 rounded-full text-[11px] font-semibold bg-amber-50 dark:bg-amber-950/70 text-amber-800 dark:text-amber-300 border border-amber-200 dark:border-amber-500/40">
            <Pause className="h-3 w-3 shrink-0" />
            <span className="truncate">{t('status.paused')}</span>
          </span>
        );
      case 'error':
        return (
          <span
            data-status="error"
            className="inline-flex items-center gap-1 max-w-full px-2 py-0.5 rounded-full text-[11px] font-semibold bg-rose-50 dark:bg-rose-950/70 text-rose-700 dark:text-rose-300 border border-rose-200 dark:border-rose-500/40"
            title={error || t('status.downloadFailed')}
          >
            <AlertCircle className="h-3 w-3 shrink-0" />
            <span className="truncate">{t('status.error')}</span>
          </span>
        );
      default:
        return (
          <span data-status={status} className="inline-flex items-center max-w-full px-2 py-0.5 rounded-full text-[11px] font-medium bg-slate-100 dark:bg-slate-800 text-slate-600 dark:text-slate-300 border border-slate-200 dark:border-slate-700">
            <span className="truncate">{t('status.queued')}</span>
          </span>
        );
    }
  };

  const safeTasks = Array.isArray(tasks) ? tasks : [];

  useEffect(() => {
    const tableEl = tableRef.current;
    if (!tableEl) return;

    const labels = tableEl.querySelectorAll<HTMLElement>('.downloads-single-line');
    const updateTruncation = () => {
      labels.forEach((el) => {
        el.classList.toggle('is-truncated', el.scrollWidth > el.clientWidth);
      });
    };

    updateTruncation();
    if (typeof ResizeObserver === 'undefined') return;

    const observer = new ResizeObserver((entries) => {
      for (const { target } of entries) {
        if (target instanceof HTMLElement) {
          target.classList.toggle('is-truncated', target.scrollWidth > target.clientWidth);
        }
      }
    });
    labels.forEach((el) => observer.observe(el));

    return () => observer.disconnect();
  }, [safeTasks]);

  if (safeTasks.length === 0) {
    return (
      <div className="flex-1 flex flex-col items-center justify-center p-12 text-center select-none bg-white/40 dark:bg-transparent">
        <div className="octoheft-subcard flex h-16 w-16 items-center justify-center rounded-2xl bg-sky-50 dark:bg-sky-950/70 border border-sky-200 dark:border-cyan-500/40 text-sky-600 dark:text-cyan-300 mb-4 shadow-sm">
          <File className="h-8 w-8" />
        </div>
        <h3 className="text-base font-semibold text-slate-900 dark:text-slate-100">
          {t('table.emptyTitle')}
        </h3>
        <p className="text-xs text-slate-600 dark:text-slate-300 max-w-sm mt-1.5 leading-relaxed">
          {t('table.emptyDescPrefix')}{' '}
          <span className="accent-neon text-sky-700 dark:text-cyan-300 font-semibold">
            {t('table.emptyDescButton')}
          </span>{' '}
          {t('table.emptyDescSuffix')}
        </p>
      </div>
    );
  }

  return (
    <div
      tabIndex={0}
      role="region"
      aria-labelledby="downloads-table-caption"
      className="downloads-table-wrapper flex-1 min-w-0 min-h-0 overflow-y-auto overflow-x-hidden focus-visible:outline focus-visible:outline-2 focus-visible:outline-sky-600 dark:focus-visible:outline-cyan-400 focus-visible:-outline-offset-2"
    >
      <table
        ref={tableRef}
        className="downloads-table text-start text-xs"
        style={
          {
            '--col-order': `"${t('table.colOrder').replace(/"/g, '\\"')}"`,
            '--col-size': `"${t('table.colSize').replace(/"/g, '\\"')}"`,
            '--col-progress': `"${t('table.colProgress').replace(/"/g, '\\"')}"`,
            '--col-speed': `"${t('table.colSpeed').replace(/"/g, '\\"')}"`,
            '--col-eta': `"${t('table.colEta').replace(/"/g, '\\"')}"`,
            '--col-status': `"${t('table.colStatus').replace(/"/g, '\\"')}"`,
            '--col-actions': `"${t('table.colActions').replace(/"/g, '\\"')}"`,
          } as React.CSSProperties
        }
      >
        <caption id="downloads-table-caption" className="visually-hidden">
          {t('sidebar.viewDownloads')} ({safeTasks.length})
        </caption>
        <colgroup>
          <col className="downloads-col-order" />
          <col className="downloads-col-file" />
          <col className="downloads-col-size" />
          <col className="downloads-col-progress" />
          <col className="downloads-col-speed" />
          <col className="downloads-col-eta" />
          <col className="downloads-col-status" />
          <col className="downloads-col-actions" />
        </colgroup>
        <thead className="bg-slate-50/95 dark:bg-slate-900/95 backdrop-blur-md text-slate-500 dark:text-slate-400 font-semibold uppercase tracking-wider text-[10px] select-none">
          <tr>
            <th scope="col" className="py-3 px-1.5 text-center border-b border-slate-200 dark:border-slate-700 bg-slate-50/95 dark:bg-slate-900/95">
              {t('table.colOrder')}
            </th>
            <th scope="col" className="py-3 px-2.5 text-start border-b border-slate-200 dark:border-slate-700 bg-slate-50/95 dark:bg-slate-900/95">
              {t('table.colFileName')}
            </th>
            <th scope="col" className="py-3 px-2 text-end border-b border-slate-200 dark:border-slate-700 bg-slate-50/95 dark:bg-slate-900/95">
              {t('table.colSize')}
            </th>
            <th scope="col" className="py-3 px-2 text-start border-b border-slate-200 dark:border-slate-700 bg-slate-50/95 dark:bg-slate-900/95">
              {t('table.colProgress')}
            </th>
            <th scope="col" className="py-3 px-2 text-end border-b border-slate-200 dark:border-slate-700 bg-slate-50/95 dark:bg-slate-900/95">
              {t('table.colSpeed')}
            </th>
            <th scope="col" className="py-3 px-2 text-end border-b border-slate-200 dark:border-slate-700 bg-slate-50/95 dark:bg-slate-900/95">
              {t('table.colEta')}
            </th>
            <th scope="col" className="py-3 px-2 text-center border-b border-slate-200 dark:border-slate-700 bg-slate-50/95 dark:bg-slate-900/95">
              {t('table.colStatus')}
            </th>
            <th scope="col" className="py-3 px-2 text-end border-b border-slate-200 dark:border-slate-700 bg-slate-50/95 dark:bg-slate-900/95">
              {t('table.colActions')}
            </th>
          </tr>
        </thead>
        <tbody>
          {safeTasks.map((task, idx) => {
            const segments = Array.isArray(task.segments) ? task.segments : [];
            const percent =
              task.total_size > 0
                ? Math.min(100, Math.floor((task.downloaded_bytes / task.total_size) * 100))
                : 0;

            const isDragOver = dragOverIndex === idx;
            const isCompletedDragReady =
              task.status === 'completed' && Boolean(task.save_path && task.save_path.trim());

            return (
              <tr
                key={task.id}
                draggable={true}
                data-outbound-drag={isCompletedDragReady ? 'true' : undefined}
                title={isCompletedDragReady ? t('table.dragOutTitle') : undefined}
                onDragStart={(e) => {
                  if (isCompletedDragReady) {
                    startOutboundFileDrag(e, task);
                    return;
                  }
                  setDraggedTaskId(task.id);
                  e.dataTransfer.setData('text/plain', task.id);
                }}
                onDragOver={(e) => {
                  e.preventDefault();
                  if (dragOverIndex !== idx) setDragOverIndex(idx);
                }}
                onDragLeave={() => {
                  if (dragOverIndex === idx) setDragOverIndex(null);
                }}
                onDrop={(e) => {
                  e.preventDefault();
                  setDragOverIndex(null);
                  const srcId = e.dataTransfer.getData('text/plain') || draggedTaskId;
                  if (srcId && onReorderTask) {
                    onReorderTask(srcId, idx);
                  }
                  setDraggedTaskId(null);
                }}
                onDoubleClick={() => onSelectTask(task)}
                className={`group transition-colors ${
                  isCompletedDragReady ? 'cursor-grab active:cursor-grabbing' : 'cursor-pointer'
                } ${
                  isDragOver
                    ? 'bg-sky-50 dark:bg-sky-950/70 ring-1 ring-inset ring-sky-500 dark:ring-cyan-400'
                    : 'hover:bg-sky-50/40 dark:hover:bg-slate-800/60'
                }`}
              >
                {/* Reorder / Priority Controls */}
                <td
                  data-col="order"
                  className="py-2.5 px-1.5 text-center align-middle whitespace-nowrap"
                  onClick={(e) => e.stopPropagation()}
                >
                  <div className="flex items-center justify-center gap-0.5">
                    <div
                      draggable={true}
                      onDragStart={(e) => {
                        e.stopPropagation();
                        setDraggedTaskId(task.id);
                        e.dataTransfer.setData('text/plain', task.id);
                      }}
                      style={{
                        touchAction: 'none',
                        WebkitUserSelect: 'none',
                        userSelect: 'none',
                      }}
                      className="cursor-grab active:cursor-grabbing text-slate-400 dark:text-slate-500 hover:text-slate-700 dark:hover:text-slate-200 transition-colors p-0.5"
                      title={t('table.dragTitle')}
                    >
                      <GripVertical className="h-3.5 w-3.5" />
                    </div>
                    <div className="flex flex-col">
                      <button
                        type="button"
                        onClick={() => onMoveTaskUp && onMoveTaskUp(task.id)}
                        disabled={idx === 0}
                        title={t('table.moveUpTitle')}
                        aria-label={t('table.moveUpTitle')}
                        className="text-slate-400 dark:text-slate-500 hover:text-sky-600 dark:hover:text-cyan-400 disabled:opacity-25 transition-colors p-0.5"
                      >
                        <ChevronUp className="h-3 w-3" />
                      </button>
                      <button
                        type="button"
                        onClick={() => onMoveTaskDown && onMoveTaskDown(task.id)}
                        disabled={idx === safeTasks.length - 1}
                        title={t('table.moveDownTitle')}
                        aria-label={t('table.moveDownTitle')}
                        className="text-slate-400 dark:text-slate-500 hover:text-sky-600 dark:hover:text-cyan-400 disabled:opacity-25 transition-colors p-0.5"
                      >
                        <ChevronDown className="h-3 w-3" />
                      </button>
                    </div>
                  </div>
                </td>

                {/* File Name & Icon */}
                <th
                  scope="row"
                  className="downloads-cell-file py-2.5 px-2.5 text-start font-normal align-middle min-w-0"
                >
                  <div className="flex items-center gap-2.5 min-w-0">
                    <div
                      draggable={isCompletedDragReady}
                      onDragStart={(e) => {
                        if (isCompletedDragReady) {
                          startOutboundFileDrag(e, task);
                        }
                      }}
                      title={isCompletedDragReady ? t('table.dragOutTitle') : undefined}
                      style={
                        isCompletedDragReady
                          ? {
                              touchAction: 'none',
                              WebkitUserSelect: 'none',
                              userSelect: 'none',
                            }
                          : undefined
                      }
                      className={`octoheft-subcard flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-slate-100 dark:bg-slate-800 border border-slate-200/80 dark:border-slate-700 transition-colors ${
                        isCompletedDragReady
                          ? 'cursor-grab active:cursor-grabbing group-hover:border-sky-400/70 dark:group-hover:border-cyan-400/60 group-hover:shadow-sm'
                          : ''
                      }`}
                    >
                      {getCategoryIcon(task.category)}
                    </div>
                    <div className="min-w-0 flex-1 overflow-hidden">
                      <div className="flex items-center gap-1.5 min-w-0">
                        <button
                          type="button"
                          onClick={(e) => {
                            e.stopPropagation();
                            onSelectTask(task);
                          }}
                          aria-label={task.file_name || 'download.bin'}
                          className="downloads-single-line flex-1 min-w-[72px] font-semibold text-slate-900 dark:text-slate-100 group-hover:text-sky-700 dark:group-hover:text-cyan-300 transition-colors cursor-pointer"
                        >
                          <span>{task.file_name || 'download.bin'}</span>
                        </button>
                        {task.media_info && (
                          <span className="px-1.5 py-0.5 rounded text-[10px] font-bold tracking-wider bg-indigo-50 dark:bg-indigo-950/70 text-indigo-700 dark:text-indigo-300 border border-indigo-200 dark:border-indigo-500/40 shrink-0 max-w-[82px] truncate">
                            {task.media_info.protocol}
                            {task.media_info.video_resolution
                              ? ` • ${task.media_info.video_resolution}`
                              : ''}
                          </span>
                        )}
                      </div>
                      <div className="text-[11px] text-slate-500 dark:text-slate-400 truncate min-w-0" dir="ltr">
                        {task.url || ''}
                      </div>
                    </div>
                  </div>
                </th>

                {/* Size */}
                <td
                  data-col="size"
                  className="py-2.5 px-2 whitespace-nowrap font-mono text-slate-800 dark:text-slate-200 text-end align-middle"
                >
                  <div className="min-w-0 text-end">
                    <div>{formatBytes(task.downloaded_bytes)}</div>
                    <div className="text-[10px] text-slate-500 dark:text-slate-400">
                      {t('table.ofTotal', { total: formatBytes(task.total_size) })}
                    </div>
                  </div>
                </td>

                {/* Progress Bar & Multi-chunk preview */}
                <td data-col="progress" className="py-2.5 px-2 whitespace-nowrap align-middle">
                  <div className="downloads-progress-inner min-w-0 w-full">
                    <div className="flex justify-between items-center gap-1 text-[11px] mb-1 font-mono">
                      <span className="font-semibold text-slate-800 dark:text-slate-100 shrink-0">
                        {percent}%
                      </span>
                      <span className="text-[10px] text-slate-500 dark:text-slate-400 truncate min-w-0">
                        {segments.length > 1
                          ? t('table.chunksCount', { count: segments.length })
                          : t('table.singleStream')}
                      </span>
                    </div>
                    <div className="h-2 w-full bg-slate-200 dark:bg-slate-800 rounded-full overflow-hidden flex gap-[1px] p-[1px]">
                      {segments.length > 1 ? (
                        segments.map((seg, sIdx) => {
                          const segSpan = seg.end_byte - seg.start_byte + 1;
                          const total = task.total_size > 0 ? task.total_size : 1;
                          const segWidthPct = (segSpan / total) * 100;
                          const segDonePct =
                            segSpan > 0
                              ? Math.min(100, Math.max(0, (seg.downloaded / segSpan) * 100))
                              : 0;

                          return (
                            <div
                              key={seg.id}
                              style={{ width: `${segWidthPct}%` }}
                              className="h-full bg-slate-100 dark:bg-slate-700 rounded-full overflow-hidden relative"
                            >
                              <div
                                style={{ width: `${segDonePct}%` }}
                                className={`h-full ${
                                  seg.status === 'completed'
                                    ? 'bg-emerald-500'
                                    : sIdx % 2 === 0
                                    ? 'bg-sky-500 dark:bg-cyan-400'
                                    : 'bg-cyan-500 dark:bg-sky-400'
                                }`}
                              />
                            </div>
                          );
                        })
                      ) : (
                        <div
                          style={{ width: `${percent}%` }}
                          className={`h-full rounded-full transition-all ${
                            task.status === 'completed'
                              ? 'bg-emerald-500'
                              : 'bg-sky-500 dark:bg-cyan-400'
                          }`}
                        />
                      )}
                    </div>
                  </div>
                </td>

                {/* Speed */}
                <td
                  data-col="speed"
                  className="py-2.5 px-2 whitespace-nowrap font-mono tabular-nums font-medium text-end align-middle"
                >
                  {task.status === 'downloading' ? (
                    <span className="accent-neon inline-block min-w-[84px] text-end text-sky-700 dark:text-cyan-300 font-semibold tabular-nums">
                      {formatSpeed(task.speed, language)}
                    </span>
                  ) : (
                    <span className="inline-block min-w-[84px] text-end text-slate-400 dark:text-slate-500">
                      --
                    </span>
                  )}
                </td>

                {/* ETA */}
                <td
                  data-col="eta"
                  className="py-2.5 px-2 whitespace-nowrap font-mono tabular-nums text-slate-600 dark:text-slate-300 text-end align-middle"
                >
                  <span className="tabular-nums">{task.status === 'downloading' ? formatEta(task.eta_seconds) : '--'}</span>
                </td>

                {/* Status */}
                <td
                  data-col="status"
                  className="py-2.5 px-2 whitespace-nowrap text-center align-middle"
                >
                  {getStatusBadge(task.status, task.error_message)}
                </td>

                {/* Actions */}
                <td
                  data-col="actions"
                  className="py-2.5 px-1.5 sm:px-2 whitespace-nowrap text-end align-middle"
                >
                  <div
                    className="flex items-center justify-end gap-0.5 shrink-0"
                    onClick={(e) => e.stopPropagation()}
                  >
                    {task.status === 'downloading' ? (
                      <button
                        type="button"
                        onClick={() => onPause(task.id)}
                        title={t('table.pauseTitle')}
                        aria-label={t('table.pauseTitle')}
                        className="p-1 rounded-lg text-slate-500 dark:text-slate-400 hover:text-amber-700 dark:hover:text-amber-300 hover:bg-amber-50 dark:hover:bg-amber-950/50 focus-visible:outline focus-visible:outline-2 focus-visible:outline-sky-600 focus-visible:-outline-offset-1 transition-colors"
                      >
                        <Pause className="h-4 w-4" />
                      </button>
                    ) : task.status !== 'completed' ? (
                      <button
                        type="button"
                        onClick={() => onResume(task.id)}
                        title={t('table.resumeTitle')}
                        aria-label={t('table.resumeTitle')}
                        className="p-1 rounded-lg text-slate-500 dark:text-slate-400 hover:text-sky-700 dark:hover:text-cyan-300 hover:bg-sky-50 dark:hover:bg-sky-950/60 focus-visible:outline focus-visible:outline-2 focus-visible:outline-sky-600 focus-visible:-outline-offset-1 transition-colors"
                      >
                        <Play className="h-4 w-4" />
                      </button>
                    ) : null}

                    {task.status === 'completed' && (
                      <>
                        <button
                          type="button"
                          onClick={() => onOpenFile(task.save_path)}
                          title={t('table.openFileTitle')}
                          aria-label={t('table.openFileTitle')}
                          className="p-1 rounded-lg text-slate-500 dark:text-slate-400 hover:text-sky-700 dark:hover:text-cyan-300 hover:bg-sky-50 dark:hover:bg-sky-950/60 focus-visible:outline focus-visible:outline-2 focus-visible:outline-sky-600 focus-visible:-outline-offset-1 transition-colors"
                        >
                          <ExternalLink className="h-4 w-4" />
                        </button>
                        <button
                          type="button"
                          onClick={() => onOpenFolder(task.save_path)}
                          title={t('table.openFolderTitle')}
                          aria-label={t('table.openFolderTitle')}
                          className="p-1 rounded-lg text-slate-500 dark:text-slate-400 hover:text-slate-900 dark:hover:text-white hover:bg-slate-100 dark:hover:bg-slate-700/70 focus-visible:outline focus-visible:outline-2 focus-visible:outline-sky-600 focus-visible:-outline-offset-1 transition-colors"
                        >
                          <Folder className="h-4 w-4" />
                        </button>
                      </>
                    )}

                    <button
                      type="button"
                      onClick={() => onSelectTask(task)}
                      title={t('table.inspectTitle')}
                      aria-label={t('table.inspectTitle')}
                      className="p-1 rounded-lg text-slate-500 dark:text-slate-400 hover:text-sky-700 dark:hover:text-cyan-300 hover:bg-sky-50 dark:hover:bg-sky-950/60 focus-visible:outline focus-visible:outline-2 focus-visible:outline-sky-600 focus-visible:-outline-offset-1 transition-colors"
                    >
                      <BarChart2 className="h-4 w-4" />
                    </button>

                    <button
                      type="button"
                      onClick={() => onDelete(task.id)}
                      title={t('table.deleteTitle')}
                      aria-label={t('table.deleteTitle')}
                      className="p-1 rounded-lg text-slate-500 dark:text-slate-400 hover:text-rose-700 dark:hover:text-rose-300 hover:bg-rose-50 dark:hover:bg-rose-950/50 focus-visible:outline focus-visible:outline-2 focus-visible:outline-sky-600 focus-visible:-outline-offset-1 transition-colors"
                    >
                      <Trash2 className="h-4 w-4" />
                    </button>
                  </div>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
};
