import React, { createContext, useContext, useEffect, useState, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import en from './en.json';
import fr from './fr.json';
import es from './es.json';
import de from './de.json';
import ar from './ar.json';
import ru from './ru.json';
import zh from './zh.json';
import type { AppSettings } from '../types';

export type LanguageCode = 'en' | 'fr' | 'es' | 'de' | 'ar' | 'ru' | 'zh';
export type TranslationKey = keyof typeof en;

export interface LanguageOption {
  code: LanguageCode;
  nativeName: string;
  englishName: string;
  dir: 'ltr' | 'rtl';
}

export const SUPPORTED_LANGUAGES: LanguageOption[] = [
  { code: 'en', nativeName: 'English', englishName: 'English', dir: 'ltr' },
  { code: 'fr', nativeName: 'Français', englishName: 'French', dir: 'ltr' },
  { code: 'es', nativeName: 'Español', englishName: 'Spanish', dir: 'ltr' },
  { code: 'de', nativeName: 'Deutsch', englishName: 'German', dir: 'ltr' },
  { code: 'ar', nativeName: 'العربية', englishName: 'Arabic', dir: 'rtl' },
  { code: 'ru', nativeName: 'Русский', englishName: 'Russian', dir: 'ltr' },
  { code: 'zh', nativeName: '简体中文', englishName: 'Simplified Chinese', dir: 'ltr' },
];

const TRANSLATIONS: Record<LanguageCode, Record<TranslationKey, string>> = {
  en,
  fr,
  es,
  de,
  ar,
  ru,
  zh,
};

export function normalizeLanguageCode(raw?: string | null): LanguageCode {
  if (!raw) return 'en';
  const primary = raw.trim().toLowerCase().split(/[-_]/)[0];
  switch (primary) {
    case 'fr':
    case 'es':
    case 'de':
    case 'ar':
    case 'ru':
    case 'zh':
      return primary;
    default:
      return 'en';
  }
}

export function getLanguageDirection(code: LanguageCode): 'ltr' | 'rtl' {
  return code === 'ar' ? 'rtl' : 'ltr';
}

interface I18nContextValue {
  language: LanguageCode;
  dir: 'ltr' | 'rtl';
  setLanguage: (code: LanguageCode) => Promise<void>;
  t: (key: TranslationKey, params?: Record<string, string | number>) => string;
}

const I18nContext = createContext<I18nContextValue | null>(null);

export const I18nProvider: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const [language, setLanguageState] = useState<LanguageCode>(() =>
    normalizeLanguageCode(typeof navigator !== 'undefined' ? navigator.language : 'en')
  );

  const dir = getLanguageDirection(language);

  useEffect(() => {
    document.documentElement.lang = language;
    document.documentElement.dir = dir;
  }, [language, dir]);

  useEffect(() => {
    let active = true;
    let unlistenSettings: (() => void) | null = null;

    invoke<AppSettings>('get_app_settings')
      .then((settings) => {
        if (active && settings?.language) {
          setLanguageState(normalizeLanguageCode(settings.language));
        }
      })
      .catch(() => {
        invoke<string>('detect_system_locale')
          .then((loc) => {
            if (active && loc) {
              setLanguageState(normalizeLanguageCode(loc));
            }
          })
          .catch(() => {});
      });

    listen<AppSettings>('settings-updated', (event) => {
      if (active && event.payload?.language) {
        setLanguageState(normalizeLanguageCode(event.payload.language));
      }
    })
      .then((unlisten) => {
        unlistenSettings = unlisten;
      })
      .catch(() => {});

    return () => {
      active = false;
      if (unlistenSettings) unlistenSettings();
    };
  }, []);

  const setLanguage = useCallback(async (next: LanguageCode) => {
    const normalized = normalizeLanguageCode(next);
    setLanguageState(normalized);
    try {
      await invoke<AppSettings>('set_app_language', {
        language: normalized,
      });
    } catch (err) {
      console.error('Failed to persist language setting:', err);
    }
  }, []);

  const t = useCallback(
    (key: TranslationKey, params?: Record<string, string | number>): string => {
      const dict = TRANSLATIONS[language] || TRANSLATIONS.en;
      let text = dict[key] ?? TRANSLATIONS.en[key] ?? String(key);
      if (params) {
        for (const [k, v] of Object.entries(params)) {
          text = text.replace(new RegExp(`\\{\\{${k}\\}\\}`, 'g'), String(v));
        }
      }
      return text;
    },
    [language]
  );

  return (
    <I18nContext.Provider value={{ language, dir, setLanguage, t }}>
      {children}
    </I18nContext.Provider>
  );
};

export function useI18n(): I18nContextValue {
  const ctx = useContext(I18nContext);
  if (!ctx) {
    throw new Error('useI18n must be used within an I18nProvider');
  }
  return ctx;
}
