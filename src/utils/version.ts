import { getVersion } from '@tauri-apps/api/app';
import pkg from '../../package.json';

/**
 * Static build-time version from package.json (e.g. "2.0.3").
 * Guarantees immediate zero-flicker rendering on initial frame.
 */
export const APP_VERSION: string = pkg.version;

/**
 * Asynchronously queries the native runtime version from Tauri's core app metadata,
 * falling back cleanly to the package.json version if running in a web/preview context.
 */
export async function fetchAppVersion(): Promise<string> {
  try {
    const v = await getVersion();
    if (v && typeof v === 'string' && v.trim().length > 0) {
      return v.trim();
    }
  } catch {
    // Non-Tauri runtime or restricted environment fallback
  }
  return APP_VERSION;
}
