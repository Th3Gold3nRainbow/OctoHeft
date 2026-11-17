import React, { useEffect, useState } from 'react';
import { PostDownloadAction, QueueConfig } from '../types';
import { X, Calendar, Clock, Moon, Power, Plus, Trash2, CheckCircle2 } from 'lucide-react';
import { useI18n, TranslationKey } from '../i18n';

interface SchedulerModalProps {
  isOpen: boolean;
  onClose: () => void;
  queues: QueueConfig[];
  onSaveQueue: (queue: QueueConfig) => void;
  onDeleteQueue: (id: string) => void;
}

const DAY_KEYS: TranslationKey[] = [
  'scheduler.day.0',
  'scheduler.day.1',
  'scheduler.day.2',
  'scheduler.day.3',
  'scheduler.day.4',
  'scheduler.day.5',
  'scheduler.day.6',
];

export const SchedulerModal: React.FC<SchedulerModalProps> = ({
  isOpen,
  onClose,
  queues,
  onSaveQueue,
  onDeleteQueue,
}) => {
  const { t } = useI18n();
  const safeQueues = Array.isArray(queues) ? queues : [];
  const [selectedQueueId, setSelectedQueueId] = useState<string>(
    safeQueues[0]?.id || 'main_queue'
  );

  // Form state
  const currentQueue = safeQueues.find((q) => q.id === selectedQueueId) || safeQueues[0] || {
    id: 'new_queue',
    name: t('scheduler.newQueueName'),
    max_concurrent_downloads: 2,
    start_time: null,
    stop_time: null,
    days_of_week: 127,
    post_action: 'none' as PostDownloadAction,
    is_enabled: false,
    task_ids: [],
  };

  const [name, setName] = useState(currentQueue.name);
  const [maxConcurrent, setMaxConcurrent] = useState(currentQueue.max_concurrent_downloads);
  const [enableStart, setEnableStart] = useState(!!currentQueue.start_time);
  const [startTime, setStartTime] = useState(currentQueue.start_time || '02:00');
  const [enableStop, setEnableStop] = useState(!!currentQueue.stop_time);
  const [stopTime, setStopTime] = useState(currentQueue.stop_time || '06:00');
  const [daysMask, setDaysMask] = useState(currentQueue.days_of_week);
  const [postAction, setPostAction] = useState<PostDownloadAction>(currentQueue.post_action);
  const [isEnabled, setIsEnabled] = useState(currentQueue.is_enabled);

  // Sync state when switching queues
  const handleSelectQueue = (q: QueueConfig) => {
    setSelectedQueueId(q.id);
    setName(q.name);
    setMaxConcurrent(q.max_concurrent_downloads);
    setEnableStart(!!q.start_time);
    setStartTime(q.start_time || '02:00');
    setEnableStop(!!q.stop_time);
    setStopTime(q.stop_time || '06:00');
    setDaysMask(q.days_of_week);
    setPostAction(q.post_action);
    setIsEnabled(q.is_enabled);
  };

  const handleAddNewQueue = () => {
    const newId = `queue_${Date.now()}`;
    const newQ: QueueConfig = {
      id: newId,
      name: t('scheduler.secondaryQueueName'),
      max_concurrent_downloads: 1,
      start_time: null,
      stop_time: null,
      days_of_week: 127,
      post_action: 'none',
      is_enabled: false,
      task_ids: [],
    };
    onSaveQueue(newQ);
    handleSelectQueue(newQ);
  };

  const toggleDay = (dayIndex: number) => {
    setDaysMask((prev) => prev ^ (1 << dayIndex));
  };

  const handleSave = (e: React.FormEvent) => {
    e.preventDefault();
    const updated: QueueConfig = {
      ...currentQueue,
      id: selectedQueueId,
      name,
      max_concurrent_downloads: maxConcurrent,
      start_time: enableStart ? startTime : null,
      stop_time: enableStop ? stopTime : null,
      days_of_week: daysMask,
      post_action: postAction,
      is_enabled: isEnabled,
    };
    onSaveQueue(updated);
    onClose();
  };

  useEffect(() => {
    if (!isOpen) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [isOpen, onClose]);

  if (!isOpen) return null;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/35 dark:bg-black/60 backdrop-blur-sm p-4 animate-in fade-in duration-150"
      role="dialog"
      aria-modal="true"
      aria-labelledby="scheduler-modal-title"
    >
      <div className="card-surface w-full max-w-2xl rounded-2xl border border-slate-200/90 dark:border-slate-700 bg-white/95 dark:bg-slate-900/95 backdrop-blur-xl shadow-2xl shadow-slate-900/15 dark:shadow-black/50 flex flex-col max-h-[90vh] overflow-hidden">
        {/* Header */}
        <div className="octoheft-subcard flex items-center justify-between border-b border-slate-200 dark:border-slate-700 px-6 py-4 bg-slate-50/90 dark:bg-slate-900/90">
          <div className="flex items-center gap-2.5">
            <div className="flex h-8 w-8 items-center justify-center rounded-xl bg-sky-50 dark:bg-sky-950/80 text-sky-700 dark:text-cyan-300 border border-sky-200 dark:border-cyan-500/40">
              <Calendar className="h-4 w-4" />
            </div>
            <div>
              <h2 id="scheduler-modal-title" className="text-base font-semibold text-slate-900 dark:text-slate-100">
                {t('scheduler.title')}
              </h2>
              <p className="text-[11px] text-slate-500 dark:text-slate-400">{t('scheduler.subtitle')}</p>
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

        <div className="flex flex-1 overflow-hidden">
          {/* Queues List Sidebar */}
          <div className="octoheft-subcard w-48 border-e border-slate-200 dark:border-slate-700 p-3 bg-slate-50/80 dark:bg-slate-900/70 flex flex-col justify-between">
            <div className="space-y-1">
              <span className="text-[10px] font-bold uppercase tracking-wider text-slate-500 dark:text-slate-400 px-2 block mb-2">
                {t('scheduler.queuesTitle')}
              </span>
              {safeQueues.map((q) => {
                const isSelected = q.id === selectedQueueId;
                return (
                  <button
                    type="button"
                    key={q.id}
                    onClick={() => handleSelectQueue(q)}
                    className={`w-full text-start px-2.5 py-1.5 rounded-lg text-xs font-medium transition-colors flex items-center justify-between ${
                      isSelected
                        ? 'bg-sky-50 dark:bg-sky-950/80 text-sky-800 dark:text-cyan-300 border border-sky-200 dark:border-cyan-500/40 font-semibold'
                        : 'text-slate-600 dark:text-slate-300 hover:bg-white dark:hover:bg-slate-800 hover:text-slate-900 dark:hover:text-white'
                    }`}
                  >
                    <span className="truncate">{q.name}</span>
                    {q.is_enabled && (
                      <span className="h-2 w-2 rounded-full bg-emerald-500 shrink-0" />
                    )}
                  </button>
                );
              })}
            </div>

            <button
              type="button"
              onClick={handleAddNewQueue}
              className="flex items-center justify-center gap-1.5 w-full py-1.5 rounded-lg border border-dashed border-slate-300 dark:border-slate-700 text-xs text-slate-600 dark:text-slate-300 hover:text-sky-700 dark:hover:text-cyan-300 hover:border-sky-400 dark:hover:border-cyan-400 hover:bg-sky-50/50 dark:hover:bg-sky-950/50 transition-colors"
            >
              <Plus className="h-3.5 w-3.5" /> {t('scheduler.addQueue')}
            </button>
          </div>

          {/* Queue Settings Form */}
          <form onSubmit={handleSave} className="flex-1 p-6 overflow-y-auto space-y-4">
            <div className="flex items-center justify-between pb-3 border-b border-slate-200 dark:border-slate-700 gap-3">
              <div>
                <label htmlFor="scheduler-queue-name" className="block text-xs font-semibold text-slate-800 dark:text-slate-200 mb-1">
                  {t('scheduler.queueNameLabel')}
                </label>
                <input
                  id="scheduler-queue-name"
                  type="text"
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  className="rounded-lg bg-white dark:bg-slate-800 border border-slate-300 dark:border-slate-700 px-3 py-1.5 text-xs text-slate-900 dark:text-slate-100 font-medium focus:border-sky-600 dark:focus:border-cyan-400 focus:outline-none focus:ring-2 focus:ring-sky-500/20"
                />
              </div>

              {/* Master Queue Enable Toggle */}
              <div className="flex items-center gap-2 text-xs font-semibold text-slate-800 dark:text-slate-200">
                <input
                  id="scheduler-enable"
                  type="checkbox"
                  checked={isEnabled}
                  onChange={(e) => setIsEnabled(e.target.checked)}
                  className="h-4 w-4 rounded border-slate-300 dark:border-slate-600 text-sky-600 accent-sky-600 dark:accent-cyan-400 focus:ring-sky-500 cursor-pointer"
                />
                <label htmlFor="scheduler-enable" className="cursor-pointer select-none">
                  {t('scheduler.enableScheduler')}
                </label>
              </div>
            </div>

            {/* Concurrency Limit */}
            <div>
              <div className="flex justify-between items-center text-xs mb-1">
                <label htmlFor="scheduler-concurrency" className="font-semibold text-slate-800 dark:text-slate-200">
                  {t('scheduler.concurrencyLabel')}
                </label>
                <span className="accent-neon font-mono text-sky-700 dark:text-cyan-300 font-bold">
                  {maxConcurrent > 1
                    ? t('scheduler.concurrencyMany', { count: maxConcurrent })
                    : t('scheduler.concurrencyOne', { count: maxConcurrent })}
                </span>
              </div>
              <input
                id="scheduler-concurrency"
                type="range"
                min="1"
                max="8"
                step="1"
                value={maxConcurrent}
                onChange={(e) => setMaxConcurrent(parseInt(e.target.value, 10))}
                className="w-full h-1.5 bg-slate-200 dark:bg-slate-700 rounded-lg appearance-none cursor-pointer accent-sky-600 dark:accent-cyan-400"
              />
              <div className="flex justify-between text-[10px] text-slate-500 dark:text-slate-400 mt-0.5">
                <span>{t('scheduler.scale1')}</span>
                <span>{t('scheduler.scale2')}</span>
                <span>4</span>
                <span>8</span>
              </div>
            </div>

            {/* Schedule Timers */}
            <div className="octoheft-subcard grid grid-cols-2 gap-4 p-3.5 rounded-xl bg-slate-50 dark:bg-slate-800/80 border border-slate-200 dark:border-slate-700">
              <div>
                <div className="flex items-center gap-2 text-xs font-semibold text-slate-800 dark:text-slate-200 mb-2">
                  <input
                    id="scheduler-enable-start"
                    type="checkbox"
                    checked={enableStart}
                    onChange={(e) => setEnableStart(e.target.checked)}
                    className="h-4 w-4 rounded border-slate-300 dark:border-slate-600 text-sky-600 accent-sky-600 dark:accent-cyan-400 focus:ring-sky-500 cursor-pointer"
                  />
                  <label htmlFor="scheduler-enable-start" className="cursor-pointer select-none">
                    {t('scheduler.startAt')}
                  </label>
                </div>
                <div className="flex items-center gap-2">
                  <Clock className="h-4 w-4 text-sky-600 dark:text-cyan-400" />
                  <input
                    type="time"
                    aria-label={t('scheduler.startAt')}
                    disabled={!enableStart}
                    value={startTime}
                    onChange={(e) => setStartTime(e.target.value)}
                    className="rounded-lg bg-white dark:bg-slate-900 border border-slate-300 dark:border-slate-700 px-3 py-1 text-xs text-slate-900 dark:text-slate-100 disabled:opacity-45 focus:border-sky-600 dark:focus:border-cyan-400 focus:outline-none focus:ring-2 focus:ring-sky-500/20"
                  />
                </div>
              </div>

              <div>
                <div className="flex items-center gap-2 text-xs font-semibold text-slate-800 dark:text-slate-200 mb-2">
                  <input
                    id="scheduler-enable-stop"
                    type="checkbox"
                    checked={enableStop}
                    onChange={(e) => setEnableStop(e.target.checked)}
                    className="h-4 w-4 rounded border-slate-300 dark:border-slate-600 text-sky-600 accent-sky-600 dark:accent-cyan-400 focus:ring-sky-500 cursor-pointer"
                  />
                  <label htmlFor="scheduler-enable-stop" className="cursor-pointer select-none">
                    {t('scheduler.stopAt')}
                  </label>
                </div>
                <div className="flex items-center gap-2">
                  <Clock className="h-4 w-4 text-amber-600 dark:text-amber-400" />
                  <input
                    type="time"
                    aria-label={t('scheduler.stopAt')}
                    disabled={!enableStop}
                    value={stopTime}
                    onChange={(e) => setStopTime(e.target.value)}
                    className="rounded-lg bg-white dark:bg-slate-900 border border-slate-300 dark:border-slate-700 px-3 py-1 text-xs text-slate-900 dark:text-slate-100 disabled:opacity-45 focus:border-sky-600 dark:focus:border-cyan-400 focus:outline-none focus:ring-2 focus:ring-sky-500/20"
                  />
                </div>
              </div>
            </div>

            {/* Days of Week */}
            <div>
              <span className="block text-xs font-semibold text-slate-800 dark:text-slate-200 mb-1.5">
                {t('scheduler.activeDays')}
              </span>
              <div className="flex gap-1.5">
                {DAY_KEYS.map((dayKey, idx) => {
                  const isActive = (daysMask & (1 << idx)) !== 0;
                  return (
                    <button
                      type="button"
                      key={dayKey}
                      onClick={() => toggleDay(idx)}
                      className={`flex-1 py-1.5 text-[11px] font-semibold rounded-lg border transition-colors ${
                        isActive
                          ? 'bg-sky-50 dark:bg-sky-950/80 text-sky-800 dark:text-cyan-300 border-sky-300 dark:border-cyan-500/40'
                          : 'octoheft-subcard bg-white dark:bg-slate-800 text-slate-600 dark:text-slate-300 border-slate-200 dark:border-slate-700 hover:bg-slate-50 dark:hover:bg-slate-700 hover:text-slate-900 dark:hover:text-white'
                      }`}
                    >
                      {t(dayKey)}
                    </button>
                  );
                })}
              </div>
            </div>

            {/* Post-Download Action */}
            <div>
              <span className="block text-xs font-semibold text-slate-800 dark:text-slate-200 mb-1.5">
                {t('scheduler.postActionLabel')}
              </span>
              <div className="grid grid-cols-2 gap-2 text-xs">
                <label
                  className={`p-2.5 rounded-xl border cursor-pointer flex items-center gap-2 transition-colors ${
                    postAction === 'none'
                      ? 'border-sky-300 dark:border-cyan-500/40 bg-sky-50 dark:bg-sky-950/80 text-sky-900 dark:text-cyan-200 font-semibold'
                      : 'octoheft-subcard border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-800 text-slate-700 dark:text-slate-300 hover:border-slate-300 dark:hover:border-slate-600 hover:bg-slate-50 dark:hover:bg-slate-700'
                  }`}
                >
                  <input
                    type="radio"
                    name="postAction"
                    value="none"
                    checked={postAction === 'none'}
                    onChange={() => setPostAction('none')}
                    className="sr-only"
                  />
                  <CheckCircle2 className="h-4 w-4 shrink-0 text-sky-600 dark:text-cyan-400" />
                  <span>{t('scheduler.actionNone')}</span>
                </label>

                <label
                  className={`p-2.5 rounded-xl border cursor-pointer flex items-center gap-2 transition-colors ${
                    postAction === 'sleeppc'
                      ? 'border-sky-300 dark:border-cyan-500/40 bg-sky-50 dark:bg-sky-950/80 text-sky-900 dark:text-cyan-200 font-semibold'
                      : 'octoheft-subcard border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-800 text-slate-700 dark:text-slate-300 hover:border-slate-300 dark:hover:border-slate-600 hover:bg-slate-50 dark:hover:bg-slate-700'
                  }`}
                >
                  <input
                    type="radio"
                    name="postAction"
                    value="sleeppc"
                    checked={postAction === 'sleeppc'}
                    onChange={() => setPostAction('sleeppc')}
                    className="sr-only"
                  />
                  <Moon className="h-4 w-4 shrink-0 text-indigo-600 dark:text-indigo-400" />
                  <span>{t('scheduler.actionSleep')}</span>
                </label>

                <label
                  className={`p-2.5 rounded-xl border cursor-pointer flex items-center gap-2 transition-colors ${
                    postAction === 'shutdownpc'
                      ? 'border-sky-300 dark:border-cyan-500/40 bg-sky-50 dark:bg-sky-950/80 text-sky-900 dark:text-cyan-200 font-semibold'
                      : 'octoheft-subcard border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-800 text-slate-700 dark:text-slate-300 hover:border-slate-300 dark:hover:border-slate-600 hover:bg-slate-50 dark:hover:bg-slate-700'
                  }`}
                >
                  <input
                    type="radio"
                    name="postAction"
                    value="shutdownpc"
                    checked={postAction === 'shutdownpc'}
                    onChange={() => setPostAction('shutdownpc')}
                    className="sr-only"
                  />
                  <Power className="h-4 w-4 shrink-0 text-rose-600 dark:text-rose-400" />
                  <span>{t('scheduler.actionShutdown')}</span>
                </label>

                <label
                  className={`p-2.5 rounded-xl border cursor-pointer flex items-center gap-2 transition-colors ${
                    postAction === 'exitapp'
                      ? 'border-sky-300 dark:border-cyan-500/40 bg-sky-50 dark:bg-sky-950/80 text-sky-900 dark:text-cyan-200 font-semibold'
                      : 'octoheft-subcard border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-800 text-slate-700 dark:text-slate-300 hover:border-slate-300 dark:hover:border-slate-600 hover:bg-slate-50 dark:hover:bg-slate-700'
                  }`}
                >
                  <input
                    type="radio"
                    name="postAction"
                    value="exitapp"
                    checked={postAction === 'exitapp'}
                    onChange={() => setPostAction('exitapp')}
                    className="sr-only"
                  />
                  <X className="h-4 w-4 shrink-0 text-amber-600 dark:text-amber-400" />
                  <span>{t('scheduler.actionExit')}</span>
                </label>
              </div>
            </div>

            {/* Actions */}
            <div className="pt-3 flex items-center justify-between border-t border-slate-200 dark:border-slate-700">
              {safeQueues.length > 1 && selectedQueueId !== 'main_queue' ? (
                <button
                  type="button"
                  onClick={() => {
                    onDeleteQueue(selectedQueueId);
                    setSelectedQueueId('main_queue');
                  }}
                  className="flex items-center gap-1.5 text-xs font-medium text-rose-600 dark:text-rose-400 hover:text-rose-700 dark:hover:text-rose-300"
                >
                  <Trash2 className="h-3.5 w-3.5" /> {t('scheduler.deleteQueue')}
                </button>
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
    </div>
  );
};
