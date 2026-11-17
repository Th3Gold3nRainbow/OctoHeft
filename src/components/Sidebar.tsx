import React from 'react';
import { DiskSpaceStats, DownloadCategory, DownloadTask } from '../types';
import { formatBytes } from '../utils';
import { useI18n } from '../i18n';
import {
  FolderArchive,
  Terminal,
  Video,
  FileText,
  Music,
  Layers,
  ArrowDownCircle,
  CheckCircle2,
  PauseCircle,
  Clapperboard,
  ListChecks,
  HardDrive,
} from 'lucide-react';

interface SidebarProps {
  activeView: 'downloads' | 'octomedia';
  onSelectView: (view: 'downloads' | 'octomedia') => void;
  currentCategory: DownloadCategory;
  onSelectCategory: (cat: DownloadCategory) => void;
  currentStatus: string;
  onSelectStatus: (status: string) => void;
  tasks: DownloadTask[];
  diskStats: DiskSpaceStats | null;
  sessionDownloadedBytes: number;
}

export const Sidebar: React.FC<SidebarProps> = ({
  activeView,
  onSelectView,
  currentCategory,
  onSelectCategory,
  currentStatus,
  onSelectStatus,
  tasks,
  diskStats,
  sessionDownloadedBytes,
}) => {
  const { t, language } = useI18n();

  const categories: { id: DownloadCategory; label: string; icon: React.ReactNode }[] = [
    { id: 'all', label: t('sidebar.category.all'), icon: <Layers className="h-4 w-4" /> },
    {
      id: 'compressed',
      label: t('sidebar.category.compressed'),
      icon: <FolderArchive className="h-4 w-4" />,
    },
    { id: 'programs', label: t('sidebar.category.programs'), icon: <Terminal className="h-4 w-4" /> },
    { id: 'video', label: t('sidebar.category.video'), icon: <Video className="h-4 w-4" /> },
    {
      id: 'documents',
      label: t('sidebar.category.documents'),
      icon: <FileText className="h-4 w-4" />,
    },
    { id: 'audio', label: t('sidebar.category.audio'), icon: <Music className="h-4 w-4" /> },
  ];

  const statuses = [
    {
      id: 'all',
      label: t('sidebar.status.all'),
      icon: <Layers className="h-3.5 w-3.5 text-slate-500 dark:text-slate-400" />,
    },
    {
      id: 'downloading',
      label: t('sidebar.status.downloading'),
      icon: <ArrowDownCircle className="h-3.5 w-3.5 text-sky-600 dark:text-cyan-400" />,
    },
    {
      id: 'completed',
      label: t('sidebar.status.completed'),
      icon: <CheckCircle2 className="h-3.5 w-3.5 text-emerald-600 dark:text-emerald-400" />,
    },
    {
      id: 'paused',
      label: t('sidebar.status.paused'),
      icon: <PauseCircle className="h-3.5 w-3.5 text-amber-600 dark:text-amber-400" />,
    },
  ];

  const safeTasks = Array.isArray(tasks) ? tasks : [];

  const getCategoryCount = (cat: DownloadCategory) => {
    if (cat === 'all') return safeTasks.length;
    return safeTasks.filter((tk) => tk.category === cat).length;
  };

  const getStatusCount = (st: string) => {
    if (st === 'all') return safeTasks.length;
    return safeTasks.filter((tk) => tk.status === st).length;
  };

  return (
    <aside className="card-surface w-56 bg-slate-100/90 dark:bg-slate-900/90 backdrop-blur-md border-e border-slate-200/80 dark:border-slate-800 p-3.5 flex flex-col gap-3 min-h-0 shrink-0 select-none">
      <div className="flex-1 min-h-0 overflow-y-auto space-y-4 pe-0.5">
        {/* Brand Header */}
        <div className="flex items-center gap-2.5 px-2">
          <img
            src="./octoheft-logo.png"
            alt="OctoHeft"
            width={34}
            height={34}
            className="h-8 w-8 rounded-lg shadow-sm border border-slate-200 dark:border-slate-700 object-cover"
          />
          <div>
            <span className="font-bold text-sm tracking-tight text-slate-900 dark:text-slate-100 flex items-center gap-1.5">
              OctoHeft
              <span className="text-[10px] font-mono px-1.5 py-0.5 rounded bg-sky-100 dark:bg-sky-950/90 text-sky-800 dark:text-cyan-300 border border-sky-200 dark:border-cyan-500/40 font-semibold">
                v2.0
              </span>
            </span>
            <p className="text-[11px] text-slate-500 dark:text-slate-400">{t('sidebar.subtitle')}</p>
          </div>
        </div>

        {/* Workspace View Switcher */}
        <div>
          <span className="px-2 text-[10px] font-bold uppercase tracking-wider text-slate-500 dark:text-slate-400 mb-2 block">
            {t('sidebar.workspace')}
          </span>
          <nav aria-label={t('sidebar.workspace')} className="space-y-1">
            <button
              type="button"
              aria-current={activeView === 'downloads' ? 'page' : undefined}
              onClick={() => onSelectView('downloads')}
              className={`w-full flex items-center justify-between px-2.5 py-2 rounded-lg text-xs transition-colors focus-visible:outline focus-visible:outline-2 focus-visible:outline-sky-600 focus-visible:outline-offset-2 ${
                activeView === 'downloads'
                  ? 'bg-sky-600/12 dark:bg-sky-500/20 text-sky-900 dark:text-cyan-200 font-semibold ring-1 ring-sky-500/25 dark:ring-cyan-400/40 shadow-sm'
                  : 'text-slate-700 dark:text-slate-300 font-medium hover:bg-white/80 dark:hover:bg-slate-800/80 hover:text-slate-900 dark:hover:text-white'
              }`}
            >
              <span className="flex items-center gap-2.5">
                <ListChecks
                  className={`h-4 w-4 ${
                    activeView === 'downloads'
                      ? 'text-sky-600 dark:text-cyan-400'
                      : 'text-slate-500 dark:text-slate-400'
                  }`}
                  aria-hidden="true"
                />
                <span>{t('sidebar.viewDownloads')}</span>
              </span>
              {safeTasks.length > 0 && (
                <span className="text-[10px] font-mono px-1.5 py-0.5 rounded-full bg-slate-200/80 dark:bg-slate-800 text-slate-700 dark:text-slate-300">
                  {safeTasks.length}
                </span>
              )}
            </button>

            <button
              type="button"
              aria-current={activeView === 'octomedia' ? 'page' : undefined}
              onClick={() => onSelectView('octomedia')}
              className={`w-full flex items-center justify-between px-2.5 py-2 rounded-lg text-xs transition-colors focus-visible:outline focus-visible:outline-2 focus-visible:outline-sky-600 focus-visible:outline-offset-2 ${
                activeView === 'octomedia'
                  ? 'bg-sky-700 text-white font-semibold shadow-sm dark:ring-1 dark:ring-cyan-400/40'
                  : 'text-slate-700 dark:text-slate-300 font-medium hover:bg-white/80 dark:hover:bg-slate-800/80 hover:text-slate-900 dark:hover:text-white'
              }`}
            >
              <span className="flex items-center gap-2.5">
                <Clapperboard
                  className={`h-4 w-4 ${
                    activeView === 'octomedia' ? 'text-white' : 'text-sky-600 dark:text-cyan-400'
                  }`}
                  aria-hidden="true"
                />
                <span>{t('sidebar.viewOctoMedia')}</span>
              </span>
              <span
                className={`text-[10px] font-mono px-1.5 py-0.5 rounded font-bold ${
                  activeView === 'octomedia'
                    ? 'bg-white/20 text-white'
                    : 'bg-sky-100 dark:bg-sky-950/90 text-sky-800 dark:text-cyan-300 border border-sky-200 dark:border-cyan-500/40'
                }`}
              >
                {t('sidebar.octoMediaBadge')}
              </span>
            </button>
          </nav>
        </div>

        {/* Categories Section */}
        <div>
          <span className="px-2 text-[10px] font-bold uppercase tracking-wider text-slate-500 dark:text-slate-400 mb-2 block">
            {t('sidebar.categories')}
          </span>
          <nav aria-label={t('sidebar.categories')} className="space-y-0.5">
            {categories.map((cat) => {
              const count = getCategoryCount(cat.id);
              const isActive = activeView === 'downloads' && currentCategory === cat.id;

              return (
                <button
                  type="button"
                  key={cat.id}
                  onClick={() => {
                    onSelectCategory(cat.id);
                    onSelectView('downloads');
                  }}
                  className={`w-full flex items-center justify-between px-2.5 py-1.5 rounded-lg text-xs transition-colors focus-visible:outline focus-visible:outline-2 focus-visible:outline-sky-600 focus-visible:outline-offset-2 ${
                    isActive
                      ? 'bg-sky-600/10 dark:bg-sky-500/20 text-sky-800 dark:text-cyan-200 font-semibold shadow-2xs ring-1 ring-sky-500/20 dark:ring-cyan-400/35'
                      : 'text-slate-600 dark:text-slate-300 font-medium hover:bg-white/80 dark:hover:bg-slate-800/80 hover:text-slate-900 dark:hover:text-white'
                  }`}
                >
                  <div className="flex items-center gap-2.5">
                    <span
                      className={
                        isActive
                          ? 'text-sky-600 dark:text-cyan-400'
                          : 'text-slate-500 dark:text-slate-400'
                      }
                    >
                      {cat.icon}
                    </span>
                    <span>{cat.label}</span>
                  </div>
                  {count > 0 && (
                    <span
                      className={`text-[10px] font-mono px-1.5 py-0.5 rounded-full ${
                        isActive
                          ? 'bg-sky-700 text-white font-semibold'
                          : 'bg-slate-200/80 dark:bg-slate-800 text-slate-700 dark:text-slate-300'
                      }`}
                    >
                      {count}
                    </span>
                  )}
                </button>
              );
            })}
          </nav>
        </div>

        {/* Status Filters */}
        <div>
          <span className="px-2 text-[10px] font-bold uppercase tracking-wider text-slate-500 dark:text-slate-400 mb-2 block">
            {t('sidebar.status')}
          </span>
          <nav aria-label={t('sidebar.status')} className="space-y-0.5">
            {statuses.map((st) => {
              const count = getStatusCount(st.id);
              const isActive = activeView === 'downloads' && currentStatus === st.id;

              return (
                <button
                  type="button"
                  key={st.id}
                  onClick={() => {
                    onSelectStatus(st.id);
                    onSelectView('downloads');
                  }}
                  className={`w-full flex items-center justify-between px-2.5 py-1.5 rounded-lg text-xs transition-colors focus-visible:outline focus-visible:outline-2 focus-visible:outline-sky-600 focus-visible:outline-offset-2 ${
                    isActive
                      ? 'octoheft-subcard bg-white dark:bg-slate-800 text-slate-900 dark:text-slate-100 font-semibold shadow-sm border border-slate-200/90 dark:border-slate-700'
                      : 'text-slate-600 dark:text-slate-300 font-medium hover:bg-white/70 dark:hover:bg-slate-800/70 hover:text-slate-900 dark:hover:text-white'
                  }`}
                >
                  <div className="flex items-center gap-2.5">
                    {st.icon}
                    <span>{st.label}</span>
                  </div>
                  {count > 0 && (
                    <span className="text-[10px] font-mono px-1.5 py-0.5 rounded-full bg-slate-200/80 dark:bg-slate-800 text-slate-700 dark:text-slate-300">
                      {count}
                    </span>
                  )}
                </button>
              );
            })}
          </nav>
        </div>
      </div>

      {/* Live Storage & Session Stats Widget */}
      {(() => {
        const usedPercent = diskStats
          ? Math.min(100, Math.max(0, Math.round(diskStats.used_percent * 10) / 10))
          : null;
        const roundedPercent = usedPercent !== null ? Math.round(usedPercent) : 0;
        const barFill =
          usedPercent === null
            ? undefined
            : usedPercent >= 92
            ? '#e11d48'
            : usedPercent >= 80
            ? '#d97706'
            : undefined;

        return (
          <div
            title={diskStats?.target_path || undefined}
            className="octoheft-subcard shrink-0 p-3 rounded-xl bg-white/85 dark:bg-slate-800/85 border border-slate-200/90 dark:border-slate-700 shadow-sm text-[11px] text-slate-700 dark:text-slate-200 space-y-2"
          >
            <div className="flex items-center gap-1.5 min-w-0">
              <HardDrive
                className="h-3.5 w-3.5 text-sky-600 dark:text-cyan-400 shrink-0"
                aria-hidden="true"
              />
              <span className="font-semibold text-slate-800 dark:text-slate-100 truncate">
                {diskStats
                  ? t('sidebar.diskFree', {
                      drive: diskStats.drive_label,
                      free: formatBytes(diskStats.available_bytes, 1, language),
                    })
                  : t('sidebar.diskChecking')}
              </span>
            </div>

            {diskStats ? (
              <progress
                className="octoheft-disk-progress w-full"
                max={100}
                value={usedPercent ?? 0}
                aria-label={t('sidebar.diskUsageLabel', {
                  drive: diskStats.drive_label,
                  percent: roundedPercent,
                })}
                style={barFill ? ({ '--progress-bar-fill': barFill } as React.CSSProperties) : undefined}
              />
            ) : (
              <progress
                className="octoheft-disk-progress w-full"
                aria-label={t('sidebar.storageTitle')}
              />
            )}

            <div className="flex items-center justify-between gap-2 text-[10px] text-slate-600 dark:text-slate-300 pt-0.5">
              <span className="truncate font-medium">
                {t('sidebar.sessionLabel')}{' '}
                <span className="accent-neon font-mono font-semibold text-sky-700 dark:text-cyan-300">
                  {formatBytes(sessionDownloadedBytes, 1, language)}
                </span>
              </span>
              {usedPercent !== null && (
                <span className="font-mono text-slate-500 dark:text-slate-400 shrink-0">
                  {roundedPercent}%
                </span>
              )}
            </div>
          </div>
        );
      })()}
    </aside>
  );
};
