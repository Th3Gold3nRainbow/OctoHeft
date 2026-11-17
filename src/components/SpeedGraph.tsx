import React from 'react';
import { formatSpeed } from '../utils';
import { useI18n } from '../i18n';

interface SpeedGraphProps {
  data: number[];
  height?: number;
  maxPoints?: number;
}

export const SpeedGraph: React.FC<SpeedGraphProps> = ({ data, height = 70, maxPoints = 30 }) => {
  const { t, language } = useI18n();
  const points = data.slice(-maxPoints);
  const maxVal = Math.max(...points, 1024 * 1024);

  const width = 360;
  const h = height;

  const coords = points.map((val, idx) => {
    const x = points.length <= 1 ? 0 : (idx / (points.length - 1)) * width;
    const y = h - (val / maxVal) * (h - 10) - 5;
    return `${x.toFixed(1)},${y.toFixed(1)}`;
  });

  const pathData = coords.length > 0 ? `M ${coords.join(' L ')}` : '';
  const areaData =
    coords.length > 0 ? `M 0,${h} L ${coords.join(' L ')} L ${width},${h} Z` : '';

  const currentSpeed = points[points.length - 1] || 0;

  return (
    <div className="octoheft-subcard relative w-full overflow-hidden rounded-xl bg-slate-50/90 dark:bg-slate-800/85 border border-slate-200 dark:border-slate-700 p-3">
      <div className="flex items-center justify-between mb-1.5">
        <span className="text-[11px] font-semibold uppercase tracking-wider text-slate-500 dark:text-slate-400">
          {t('speedGraph.title')}
        </span>
        <span className="accent-neon inline-block min-w-[84px] text-end text-sm font-bold font-mono tabular-nums text-sky-700 dark:text-cyan-300">
          {formatSpeed(currentSpeed, language)}
        </span>
      </div>
      <div className="relative w-full h-[70px]">
        <svg
          viewBox={`0 0 ${width} ${h}`}
          className="w-full h-full overflow-visible text-sky-600 dark:text-cyan-400"
          preserveAspectRatio="none"
          aria-hidden="true"
        >
          <defs>
            <linearGradient id="speedGrad" x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor="currentColor" stopOpacity="0.28" />
              <stop offset="100%" stopColor="currentColor" stopOpacity="0.02" />
            </linearGradient>
          </defs>
          {areaData && <path d={areaData} fill="url(#speedGrad)" />}
          {pathData && (
            <path
              d={pathData}
              fill="none"
              stroke="currentColor"
              strokeWidth="2.2"
              strokeLinecap="round"
              strokeLinejoin="round"
            />
          )}
        </svg>
      </div>
      <div className="flex justify-between items-center text-[10px] text-slate-500 dark:text-slate-400 font-mono mt-1">
        <span>{t('speedGraph.thirtySecAgo')}</span>
        <span className="tabular-nums">{t('speedGraph.peak', { speed: formatSpeed(maxVal, language) })}</span>
        <span>{t('speedGraph.now')}</span>
      </div>
    </div>
  );
};
