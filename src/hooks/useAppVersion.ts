import { useState, useEffect } from 'react';
import { APP_VERSION, fetchAppVersion } from '../utils/version';

/**
 * React hook returning the dynamic application version (e.g. "2.0.3").
 * Initializes synchronously with APP_VERSION from package.json for zero-flicker paint,
 * then checks runtime metadata from Tauri core.
 */
export function useAppVersion(): string {
  const [version, setVersion] = useState<string>(APP_VERSION);

  useEffect(() => {
    let active = true;
    fetchAppVersion()
      .then((ver) => {
        if (active && ver) {
          setVersion(ver);
        }
      })
      .catch(() => {});
    return () => {
      active = false;
    };
  }, []);

  return version;
}
