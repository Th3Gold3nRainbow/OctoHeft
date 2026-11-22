import { useState, useEffect, useCallback, useRef } from 'react';

export type ThemeMode = 'light' | 'dark';

export const THEME_STORAGE_KEY = 'octoheft_theme';
const THEME_CHANGE_EVENT = 'octoheft-theme-change';
const THEME_CYCLE_ORDER: readonly ThemeMode[] = ['light', 'dark'];

function isValidThemeMode(value: string | null): value is ThemeMode {
  return value === 'light' || value === 'dark';
}

function readStoredTheme(): ThemeMode | null {
  try {
    const raw = window.localStorage.getItem(THEME_STORAGE_KEY);
    if (raw === 'neon') {
      // Seamlessly migrate legacy neon theme to obsidian dark
      window.localStorage.setItem(THEME_STORAGE_KEY, 'dark');
      return 'dark';
    }
    return isValidThemeMode(raw) ? raw : null;
  } catch {
    return null;
  }
}

function getSystemTheme(): 'light' | 'dark' {
  if (
    typeof window !== 'undefined' &&
    typeof window.matchMedia === 'function' &&
    window.matchMedia('(prefers-color-scheme: dark)').matches
  ) {
    return 'dark';
  }
  return 'light';
}

export function applyThemeToDocument(mode: ThemeMode, hasExplicitOverride = true): void {
  if (typeof document === 'undefined') return;
  const root = document.documentElement;

  root.classList.remove('neon');
  if (mode === 'dark') {
    root.classList.add('dark');
  } else {
    root.classList.remove('dark');
  }

  root.style.colorScheme = mode;

  const meta = document.querySelector<HTMLMetaElement>('meta[name="color-scheme"]');
  if (meta) {
    meta.content = !hasExplicitOverride ? 'light dark' : mode;
  }
}

export function useTheme() {
  const [theme, setThemeState] = useState<ThemeMode>(() => {
    const stored = readStoredTheme();
    return stored ?? getSystemTheme();
  });

  const themeRef = useRef<ThemeMode>(theme);
  themeRef.current = theme;

  useEffect(() => {
    const hasOverride = readStoredTheme() !== null;
    applyThemeToDocument(theme, hasOverride);
  }, [theme]);

  useEffect(() => {
    if (typeof window === 'undefined') return;

    const handleStorage = (e: StorageEvent) => {
      if (e.key !== THEME_STORAGE_KEY) return;
      const val = e.newValue === 'neon' ? 'dark' : e.newValue;
      const next = isValidThemeMode(val) ? val : getSystemTheme();
      themeRef.current = next;
      setThemeState(next);
    };

    const mql =
      typeof window.matchMedia === 'function'
        ? window.matchMedia('(prefers-color-scheme: dark)')
        : null;

    const handleMediaChange = (e: MediaQueryListEvent) => {
      if (readStoredTheme() === null) {
        const next: ThemeMode = e.matches ? 'dark' : 'light';
        themeRef.current = next;
        setThemeState(next);
      }
    };

    const handleThemeSync = (e: Event) => {
      const next = (e as CustomEvent<ThemeMode>).detail;
      if (isValidThemeMode(next) && themeRef.current !== next) {
        themeRef.current = next;
        setThemeState(next);
      }
    };

    window.addEventListener('storage', handleStorage);
    window.addEventListener(THEME_CHANGE_EVENT, handleThemeSync);
    mql?.addEventListener('change', handleMediaChange);

    return () => {
      window.removeEventListener('storage', handleStorage);
      window.removeEventListener(THEME_CHANGE_EVENT, handleThemeSync);
      mql?.removeEventListener('change', handleMediaChange);
    };
  }, []);

  const setTheme = useCallback((next: ThemeMode) => {
    try {
      window.localStorage.setItem(THEME_STORAGE_KEY, next);
    } catch {
      // Ignore persistence errors (e.g. restricted storage) while still applying theme in memory
    }
    themeRef.current = next;
    applyThemeToDocument(next, true);
    setThemeState(next);
    if (typeof window !== 'undefined') {
      window.dispatchEvent(new CustomEvent<ThemeMode>(THEME_CHANGE_EVENT, { detail: next }));
    }
  }, []);

  const cycleTheme = useCallback(() => {
    const idx = THEME_CYCLE_ORDER.indexOf(themeRef.current);
    const next = THEME_CYCLE_ORDER[(idx + 1) % THEME_CYCLE_ORDER.length];
    setTheme(next);
  }, [setTheme]);

  return {
    theme,
    setTheme,
    cycleTheme,
  };
}

