import React, { useState, useRef, useEffect, useCallback } from 'react';
import ReactDOM from 'react-dom';
import {
  Plus,
  Play,
  Pause,
  Gauge,
  Activity,
  Search,
  Layers,
  Calendar,
  FolderArchive,
  Settings,
  Sun,
  Moon,
  Zap,
  Clipboard,
  ClipboardCheck,
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { formatSpeed } from '../utils';
import { useI18n } from '../i18n';
import { useTheme, ThemeMode } from '../hooks/useTheme';

interface HeaderProps {
  totalSpeed: number;
  activeCount: number;
  theme?: ThemeMode;
  onCycleTheme?: () => void;
  clipboardMonitorActive?: boolean;
  onToggleClipboardMonitor?: () => void;
  onClipboardMonitorChanged?: (active: boolean) => void;
  onOpenAddModal: () => void;
  onOpenBatchModal: () => void;
  onOpenSchedulerModal: () => void;
  onOpenZipPreviewModal: () => void;
  onOpenSettingsModal: () => void;
  onResumeAll: () => void;
  onPauseAll: () => void;
  onSetGlobalSpeedLimit: (limit: number) => void;
  searchQuery: string;
  onSearchChange: (q: string) => void;
}

const POPOVER_WIDTH = 256;
const POPOVER_ESTIMATED_HEIGHT = 210;

export const Header: React.FC<HeaderProps> = ({
  totalSpeed,
  activeCount,
  theme: propTheme,
  onCycleTheme,
  clipboardMonitorActive: propClipboardActive,
  onToggleClipboardMonitor,
  onClipboardMonitorChanged,
  onOpenAddModal,
  onOpenBatchModal,
  onOpenSchedulerModal,
  onOpenZipPreviewModal,
  onOpenSettingsModal,
  onResumeAll,
  onPauseAll,
  onSetGlobalSpeedLimit,
  searchQuery,
  onSearchChange,
}) => {
  const { t, dir, language } = useI18n();
  const { theme: hookTheme, cycleTheme: hookCycleTheme } = useTheme();
  const theme = propTheme ?? hookTheme;
  const cycleTheme = onCycleTheme ?? hookCycleTheme;
  const [internalClipboardActive, setInternalClipboardActive] = useState<boolean>(
    Boolean(propClipboardActive)
  );
  const clipboardMonitorActive = internalClipboardActive;
  const [speedLimit, setSpeedLimit] = useState<number>(0);

  useEffect(() => {
    if (propClipboardActive !== undefined) {
      setInternalClipboardActive(propClipboardActive);
    }
  }, [propClipboardActive]);

  useEffect(() => {
    let mounted = true;
    invoke<boolean>('get_clipboard_monitor_status')
      .then((status) => {
        if (!mounted) return;
        const active = Boolean(status);
        setInternalClipboardActive(active);
        onClipboardMonitorChanged?.(active);
      })
      .catch(() => {});
    return () => {
      mounted = false;
    };
  }, [onClipboardMonitorChanged]);

  const handleClipboardButtonClick = async () => {
    if (onToggleClipboardMonitor) {
      onToggleClipboardMonitor();
      return;
    }
    const next = !clipboardMonitorActive;
    try {
      const updated = await invoke<boolean>('toggle_clipboard_monitor', {
        enable: next,
      });
      const active = Boolean(updated);
      setInternalClipboardActive(active);
      onClipboardMonitorChanged?.(active);
    } catch (err) {
      console.error('Failed to toggle clipboard monitor:', err);
      if (!('__TAURI_INTERNALS__' in window)) {
        setInternalClipboardActive(next);
        onClipboardMonitorChanged?.(next);
      }
    }
  };
  const [isSpeedMenuOpen, setIsSpeedMenuOpen] = useState(false);
  const [customSliderMb, setCustomSliderMb] = useState<number>(0);
  const [popoverCoords, setPopoverCoords] = useState<{ top: number; left: number }>({
    top: 64,
    left: 12,
  });

  const toolbarRef = useRef<HTMLDivElement | null>(null);
  const speedBtnRef = useRef<HTMLButtonElement | null>(null);
  const speedPopoverRef = useRef<HTMLDivElement | null>(null);

  const speedOptions = [
    { label: t('header.unlimited'), bytes: 0 },
    { label: t('header.mbPerSec', { val: 1 }), bytes: 1024 * 1024 },
    { label: t('header.mbPerSec', { val: 5 }), bytes: 5 * 1024 * 1024 },
    { label: t('header.mbPerSec', { val: 10 }), bytes: 10 * 1024 * 1024 },
    { label: t('header.mbPerSec', { val: 25 }), bytes: 25 * 1024 * 1024 },
    { label: t('header.mbPerSec', { val: 50 }), bytes: 50 * 1024 * 1024 },
  ];

  const updatePopoverPosition = useCallback(() => {
    const btn = speedBtnRef.current;
    if (!btn) return;
    const rect = btn.getBoundingClientRect();
    const preferredLeft = dir === 'rtl' ? rect.left : rect.right - POPOVER_WIDTH;
    const clampedLeft = Math.max(
      12,
      Math.min(window.innerWidth - POPOVER_WIDTH - 12, preferredLeft)
    );
    let top = rect.bottom + 8;
    if (top + POPOVER_ESTIMATED_HEIGHT > window.innerHeight - 12) {
      top = Math.max(12, rect.top - POPOVER_ESTIMATED_HEIGHT - 8);
    }
    setPopoverCoords({ top, left: clampedLeft });
  }, [dir]);

  useEffect(() => {
    const bar = toolbarRef.current;
    if (!bar || 'focusGroup' in HTMLElement.prototype) return;

    const getItems = () =>
      Array.from(bar.querySelectorAll<HTMLButtonElement>('button')).filter((el) => !el.disabled);

    const initialItems = getItems();
    if (initialItems.length > 0) {
      const currentActive = initialItems.find((b) => b.tabIndex === 0) ?? initialItems[0];
      initialItems.forEach((b) => {
        b.tabIndex = b === currentActive ? 0 : -1;
      });
    }

    const handleFocusIn = (e: FocusEvent) => {
      const target = e.target as HTMLElement | null;
      if (!target) return;
      const items = getItems();
      const matched = items.find((b) => b === target || b.contains(target));
      if (!matched) return;
      items.forEach((b) => {
        b.tabIndex = b === matched ? 0 : -1;
      });
    };

    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.altKey || e.ctrlKey || e.metaKey || e.shiftKey) return;
      const items = getItems();
      if (items.length === 0) return;

      const active = document.activeElement as HTMLElement | null;
      const idx = items.findIndex((b) => b === active);
      if (idx < 0) return;

      const rtl = getComputedStyle(bar).direction === 'rtl';
      const steps: Record<string, number> = {
        ArrowRight: rtl ? -1 : 1,
        ArrowLeft: rtl ? 1 : -1,
      };

      let nextIdx: number | null = null;
      if (e.key in steps) {
        nextIdx = (idx + steps[e.key] + items.length) % items.length;
      } else if (e.key === 'Home') {
        nextIdx = 0;
      } else if (e.key === 'End') {
        nextIdx = items.length - 1;
      }

      if (nextIdx === null) return;
      e.preventDefault();
      const nextBtn = items[nextIdx];
      items.forEach((b) => {
        b.tabIndex = b === nextBtn ? 0 : -1;
      });
      nextBtn.focus();
    };

    bar.addEventListener('focusin', handleFocusIn);
    bar.addEventListener('keydown', handleKeyDown);
    return () => {
      bar.removeEventListener('focusin', handleFocusIn);
      bar.removeEventListener('keydown', handleKeyDown);
    };
  }, []);

  useEffect(() => {
    if (!isSpeedMenuOpen) return;
    updatePopoverPosition();

    const handleMouseDown = (e: MouseEvent) => {
      const target = e.target as Node;
      if (
        speedBtnRef.current?.contains(target) ||
        speedPopoverRef.current?.contains(target)
      ) {
        return;
      }
      setIsSpeedMenuOpen(false);
    };

    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        setIsSpeedMenuOpen(false);
        speedBtnRef.current?.focus();
      }
    };

    const handleResize = () => updatePopoverPosition();

    document.addEventListener('mousedown', handleMouseDown);
    document.addEventListener('keydown', handleKeyDown);
    window.addEventListener('resize', handleResize);
    window.addEventListener('scroll', handleResize, true);

    return () => {
      document.removeEventListener('mousedown', handleMouseDown);
      document.removeEventListener('keydown', handleKeyDown);
      window.removeEventListener('resize', handleResize);
      window.removeEventListener('scroll', handleResize, true);
    };
  }, [isSpeedMenuOpen, updatePopoverPosition]);

  const handleSelectLimit = (bytes: number) => {
    setSpeedLimit(bytes);
    setCustomSliderMb(Math.round(bytes / (1024 * 1024)));
    onSetGlobalSpeedLimit(bytes);
  };

  const handleSliderChange = (mb: number) => {
    setCustomSliderMb(mb);
    const bytes = mb * 1024 * 1024;
    setSpeedLimit(bytes);
    onSetGlobalSpeedLimit(bytes);
  };

  const themeTooltip = t('header.themeTitle', { theme: theme.toUpperCase() });

  return (
    <header className="octoheft-header card-surface border-b border-slate-200/80 dark:border-slate-700/80 bg-white/80 dark:bg-slate-900/85 backdrop-blur-md px-5 py-2.5 shrink-0">
      <div className="octoheft-header-grid">
        {/* Left Zone: Search & Filter */}
        <search className="octoheft-header-search relative w-full max-w-full">
          <Search
            className="absolute start-3 top-1/2 -translate-y-1/2 h-4 w-4 text-slate-400 dark:text-slate-400 pointer-events-none"
            aria-hidden="true"
          />
          <input
            type="search"
            aria-label={t('header.searchPlaceholder')}
            placeholder={t('header.searchPlaceholder')}
            value={searchQuery}
            onChange={(e) => onSearchChange(e.target.value)}
            spellCheck={false}
            className="h-9 w-full rounded-lg bg-slate-50/90 dark:bg-slate-800/90 border border-slate-200/90 dark:border-slate-700 ps-9 pe-3 text-xs text-slate-900 dark:text-slate-100 placeholder-slate-400 focus:bg-white dark:focus:bg-slate-900 focus:border-sky-600 dark:focus:border-cyan-400 focus:outline-none focus:ring-2 focus:ring-sky-500/20 dark:focus:ring-cyan-400/25 transition-colors"
          />
        </search>

        {/* Composite Windows 11 Toolbar (Center + Right Utility Zones) */}
        <div
          ref={toolbarRef}
          role="toolbar"
          aria-label={t('header.toolbarLabel')}
          className="octoheft-header-toolbar"
          {...({ focusgroup: 'toolbar wrap' } as Record<string, string>)}
        >
          {/* Center Zone: Live Speed Pill + Primary Action + Global Controls */}
          <div
            role="group"
            aria-label={t('header.transferGroupLabel')}
            className="octoheft-header-center flex items-center gap-2 whitespace-nowrap"
          >
            <div
              role="status"
              aria-live="off"
              className={`inline-flex items-center gap-2 h-9 px-3 rounded-lg border text-xs whitespace-nowrap transition-colors ${
                totalSpeed > 0
                  ? 'bg-sky-50/90 dark:bg-sky-950/70 border-sky-200 dark:border-cyan-500/40 text-sky-900 dark:text-cyan-100'
                  : 'octoheft-subcard bg-slate-50/90 dark:bg-slate-800/85 border-slate-200/90 dark:border-slate-700 text-slate-600 dark:text-slate-300'
              }`}
            >
              <Activity
                className={`h-3.5 w-3.5 shrink-0 ${
                  totalSpeed > 0
                    ? 'text-sky-600 dark:text-cyan-400 animate-pulse'
                    : 'text-slate-400 dark:text-slate-400'
                }`}
                aria-hidden="true"
              />
              <span className="text-[11px] font-semibold uppercase tracking-wide text-slate-600 dark:text-slate-300">
                {t('header.activeCount', { count: activeCount })}
              </span>
              <span className="text-slate-300 dark:text-slate-600" aria-hidden="true">
                -
              </span>
              <span
                className={`font-mono tabular-nums font-bold text-sky-700 dark:text-sky-300 inline-block min-w-[82px] text-end ${
                  theme === 'neon' ? 'accent-neon' : ''
                }`}
              >
                {formatSpeed(totalSpeed, language)}
              </span>
            </div>

            <button
              type="button"
              onClick={onOpenAddModal}
              className="octoheft-toolbar-btn inline-flex items-center gap-1.5 h-9 px-3.5 rounded-lg text-xs font-semibold bg-sky-700 text-white hover:bg-sky-800 dark:hover:bg-sky-600 border border-transparent dark:border-cyan-400/30 shadow-sm shadow-sky-700/20 whitespace-nowrap active:scale-[0.98]"
            >
              <Plus className="h-4 w-4 stroke-[2.5]" aria-hidden="true" />
              <span>{t('header.addUrl')}</span>
            </button>

            <div className="inline-flex items-center gap-1">
              <button
                type="button"
                onClick={onResumeAll}
                title={t('header.resumeAll')}
                aria-label={t('header.resumeAll')}
                className="octoheft-toolbar-btn octoheft-subcard inline-flex items-center justify-center h-9 w-9 rounded-lg bg-white/90 dark:bg-slate-800/90 border border-slate-200/90 dark:border-slate-700 text-slate-600 dark:text-slate-300 hover:bg-sky-50 dark:hover:bg-sky-950/60 hover:text-sky-700 dark:hover:text-cyan-300 hover:border-sky-300 dark:hover:border-cyan-500/40 shadow-sm"
              >
                <Play className="h-4 w-4" aria-hidden="true" />
              </button>
              <button
                type="button"
                onClick={onPauseAll}
                title={t('header.pauseAll')}
                aria-label={t('header.pauseAll')}
                className="octoheft-toolbar-btn octoheft-subcard inline-flex items-center justify-center h-9 w-9 rounded-lg bg-white/90 dark:bg-slate-800/90 border border-slate-200/90 dark:border-slate-700 text-slate-600 dark:text-slate-300 hover:bg-amber-50 dark:hover:bg-amber-950/50 hover:text-amber-700 dark:hover:text-amber-300 hover:border-amber-300 dark:hover:border-amber-500/40 shadow-sm"
              >
                <Pause className="h-4 w-4" aria-hidden="true" />
              </button>
            </div>
          </div>

          {/* Right Zone: Separator + Cohesive Frosted Utility Pill */}
          <div className="octoheft-header-right-wrap">
            <span role="separator" aria-orientation="vertical" className="octoheft-toolbar-sep" />

            <div
              role="group"
              aria-label={t('header.utilitiesGroupLabel')}
              className="card-surface inline-flex items-center flex-nowrap h-9 rounded-lg bg-white/85 dark:bg-slate-800/90 backdrop-blur-md border border-slate-200/90 dark:border-slate-700 shadow-sm whitespace-nowrap"
            >
              {/* Speed Limiter Portal Popover Trigger */}
              <button
                ref={speedBtnRef}
                type="button"
                aria-haspopup="dialog"
                aria-expanded={isSpeedMenuOpen}
                aria-label={`${t('header.speedLimiter')}: ${
                  speedLimit === 0 ? t('header.unlimited') : formatSpeed(speedLimit, language)
                }`}
                title={t('header.speedLimiter')}
                onClick={() => setIsSpeedMenuOpen((prev) => !prev)}
                className={`octoheft-toolbar-btn rounded-s-lg inline-flex items-center gap-1.5 h-9 px-3 text-xs font-medium whitespace-nowrap border-e border-slate-200/80 dark:border-slate-700/80 focus-visible:relative focus-visible:z-10 ${
                  isSpeedMenuOpen || speedLimit > 0
                    ? 'bg-sky-50 dark:bg-sky-950/80 text-sky-700 dark:text-cyan-300'
                    : 'text-slate-700 dark:text-slate-200 hover:bg-slate-100/80 dark:hover:bg-slate-700/70 hover:text-slate-900 dark:hover:text-white'
                }`}
              >
                <Gauge className="h-3.5 w-3.5 text-sky-600 dark:text-cyan-400 shrink-0" aria-hidden="true" />
                <span className="tabular-nums">
                  {speedLimit === 0 ? t('header.unlimited') : formatSpeed(speedLimit, language)}
                </span>
              </button>

              {isSpeedMenuOpen &&
                ReactDOM.createPortal(
                  <div
                    ref={speedPopoverRef}
                    role="dialog"
                    aria-label={t('header.speedLimiter')}
                    style={{
                      position: 'fixed',
                      top: `${popoverCoords.top}px`,
                      left: `${popoverCoords.left}px`,
                      width: `${POPOVER_WIDTH}px`,
                      zIndex: 99999,
                    }}
                    className="card-surface rounded-xl bg-white/95 dark:bg-slate-900/95 backdrop-blur-xl border border-slate-200 dark:border-slate-700 shadow-2xl shadow-slate-900/15 dark:shadow-black/50 p-3.5 text-xs space-y-3"
                  >
                    <div className="flex items-center justify-between border-b border-slate-100 dark:border-slate-700/80 pb-2">
                      <span className="font-semibold text-slate-800 dark:text-slate-100">
                        {t('header.speedLimiter')}
                      </span>
                      <span
                        className={`font-mono text-sky-700 dark:text-cyan-300 font-bold ${
                          theme === 'neon' ? 'accent-neon' : ''
                        }`}
                      >
                        {speedLimit === 0
                          ? t('header.unlimited')
                          : t('header.mbPerSec', { val: customSliderMb })}
                      </span>
                    </div>

                    <div>
                      <input
                        type="range"
                        min="0"
                        max="50"
                        step="1"
                        aria-label={t('header.speedLimiter')}
                        value={customSliderMb}
                        onChange={(e) => handleSliderChange(parseInt(e.target.value, 10))}
                        className="w-full h-1.5 bg-slate-200 dark:bg-slate-700 rounded-lg appearance-none cursor-pointer accent-sky-600 dark:accent-cyan-400"
                      />
                      <div className="flex justify-between text-[10px] text-slate-500 dark:text-slate-400 mt-1">
                        <span>{t('header.zeroUnlimited')}</span>
                        <span>{t('header.mbPerSec', { val: 25 })}</span>
                        <span>{t('header.mbPerSec', { val: 50 })}</span>
                      </div>
                    </div>

                    <div className="grid grid-cols-3 gap-1.5 pt-1">
                      {speedOptions.map((opt) => (
                        <button
                          type="button"
                          key={opt.bytes}
                          onClick={() => handleSelectLimit(opt.bytes)}
                          className={`px-1.5 py-1.5 rounded-lg text-[11px] font-medium transition-colors border ${
                            speedLimit === opt.bytes
                              ? 'bg-sky-700 text-white border-sky-700 dark:border-cyan-400/50 font-semibold shadow-sm'
                              : 'octoheft-subcard bg-slate-50 dark:bg-slate-800 text-slate-700 dark:text-slate-200 border-slate-200 dark:border-slate-700 hover:bg-slate-100 dark:hover:bg-slate-700'
                          }`}
                        >
                          {opt.label}
                        </button>
                      ))}
                    </div>
                  </div>,
                  document.body
                )}

              <button
                type="button"
                onClick={onOpenBatchModal}
                title={t('header.batchTitle')}
                aria-label={t('header.batch')}
                className="octoheft-toolbar-btn inline-flex items-center justify-center h-9 w-9 text-sky-600 dark:text-sky-400 hover:bg-sky-50 dark:hover:bg-sky-950/60 hover:text-sky-700 dark:hover:text-cyan-300 border-e border-slate-200/80 dark:border-slate-700/80 focus-visible:relative focus-visible:z-10"
              >
                <Layers className="h-4 w-4" aria-hidden="true" />
              </button>

              <button
                type="button"
                onClick={onOpenSchedulerModal}
                title={t('header.schedulerTitle')}
                aria-label={t('header.scheduler')}
                className="octoheft-toolbar-btn inline-flex items-center justify-center h-9 w-9 text-indigo-600 dark:text-indigo-400 hover:bg-indigo-50 dark:hover:bg-indigo-950/60 hover:text-indigo-700 dark:hover:text-indigo-300 border-e border-slate-200/80 dark:border-slate-700/80 focus-visible:relative focus-visible:z-10"
              >
                <Calendar className="h-4 w-4" aria-hidden="true" />
              </button>

              <button
                type="button"
                onClick={onOpenZipPreviewModal}
                title={t('header.zipPreviewTitle')}
                aria-label={t('header.zipPreview')}
                className="octoheft-toolbar-btn inline-flex items-center justify-center h-9 w-9 text-amber-600 dark:text-amber-400 hover:bg-amber-50 dark:hover:bg-amber-950/50 hover:text-amber-700 dark:hover:text-amber-300 border-e border-slate-200/80 dark:border-slate-700/80 focus-visible:relative focus-visible:z-10"
              >
                <FolderArchive className="h-4 w-4" aria-hidden="true" />
              </button>

              {/* IDM-Signature Clipboard Monitor Toggle */}
              <button
                type="button"
                aria-pressed={clipboardMonitorActive}
                onClick={handleClipboardButtonClick}
                title={t('header.clipboardMonitorTitle')}
                aria-label={t('header.clipboardMonitor')}
                data-clipboard-monitor={clipboardMonitorActive ? 'active' : 'inactive'}
                className="octoheft-toolbar-btn relative inline-flex items-center justify-center h-9 w-9 text-slate-600 dark:text-slate-300 hover:bg-sky-50/80 dark:hover:bg-sky-950/60 hover:text-sky-700 dark:hover:text-cyan-300 border-e border-slate-200/80 dark:border-slate-700/80 focus-visible:relative focus-visible:z-10 shrink-0"
              >
                <span className="relative inline-flex items-center justify-center">
                  {clipboardMonitorActive ? (
                    <ClipboardCheck
                      className="h-4 w-4 drop-shadow-[0_0_6px_rgba(0,240,255,0.65)]"
                      aria-hidden="true"
                    />
                  ) : (
                    <Clipboard className="h-4 w-4" aria-hidden="true" />
                  )}
                  {clipboardMonitorActive && (
                    <span
                      className="absolute -top-1 -end-1 h-2 w-2 rounded-full bg-cyan-500 dark:bg-cyan-400 ring-2 ring-white dark:ring-slate-800 shadow-[0_0_6px_rgba(0,240,255,0.95)]"
                      aria-hidden="true"
                    />
                  )}
                </span>
              </button>

              {/* 3-Way Theme Toggle Button (Light -> Dark -> Neon), directly adjacent to Settings */}
              <button
                type="button"
                onClick={cycleTheme}
                title={themeTooltip}
                aria-label={themeTooltip}
                data-theme-toggle={theme}
                className="octoheft-toolbar-btn inline-flex items-center justify-center h-9 w-9 border-e border-slate-200/80 dark:border-slate-700/80 neon:border-cyan-500/30 neon:bg-[#021a42]/80 text-slate-700 dark:text-slate-200 hover:bg-slate-100 dark:hover:bg-slate-700/70 neon:hover:bg-[#03265e] transition-colors focus-visible:relative focus-visible:z-10 shrink-0"
              >
                {theme === 'light' && (
                  <Sun className="h-4 w-4 text-amber-500" aria-hidden="true" />
                )}
                {theme === 'dark' && (
                  <Moon className="h-4 w-4 text-blue-400" aria-hidden="true" />
                )}
                {theme === 'neon' && (
                  <Zap
                    className="h-4 w-4 text-cyan-400 drop-shadow-[0_0_6px_rgba(0,240,255,0.8)]"
                    aria-hidden="true"
                  />
                )}
              </button>

              <button
                type="button"
                onClick={onOpenSettingsModal}
                title={t('header.settingsTitle')}
                aria-label={t('header.settings')}
                className="octoheft-toolbar-btn rounded-e-lg inline-flex items-center justify-center h-9 w-9 text-slate-600 dark:text-slate-300 hover:bg-slate-100 dark:hover:bg-slate-700/70 hover:text-slate-900 dark:hover:text-white focus-visible:relative focus-visible:z-10 shrink-0"
              >
                <Settings className="h-4 w-4" aria-hidden="true" />
              </button>
            </div>
          </div>
        </div>
      </div>
    </header>
  );
};
