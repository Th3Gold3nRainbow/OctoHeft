// OctoHeft Manifest V3 Background Service Worker, Generic Media Sniffer & Global Download Interceptor

const NATIVE_HOST_NAME = "com.antigravity.octoheft";
const STORAGE_EXTENSIONS_KEY = "octoheftInterceptExtensions";

// In-memory generic non-YouTube media store indexed by tabId
const mediaByTab = new Map();

// Known media MIME patterns for generic direct stream detection
const MEDIA_MIMES = [
  "video/",
  "audio/",
  "application/vnd.apple.mpegurl",
  "application/x-mpegurl",
  "application/dash+xml",
  "video/mp2t",
  "application/octet-stream-m3u8"
];

// File extension regex for generic network sniffer URL inspection
const MEDIA_EXT_REGEX = /\.(mp4|mkv|webm|m3u8|mpd|ts|flv|avi|mov|mp3|aac|flac|wav|m4a)(\?|$)/i;

// Default 53 IDM extension tokens (dynamically synchronized with OctoHeft SettingsStore via get_settings)
const DEFAULT_IDM_EXTENSIONS = [
  "3GP", "7Z", "AAC", "ACE", "AIF", "APK", "ARJ", "ASF", "AVI", "BIN", "BZ2",
  "EXE", "GZ", "GZIP", "IMG", "ISO", "LZH", "M4A", "M4V", "MKV", "MOV", "MP3",
  "MP4", "MPA", "MPE", "MPEG", "MPG", "MSI", "MSU", "OGG", "OGV", "PLJ", "PPS",
  "PPT", "QT", "R0*", "R1*", "RA", "RAR", "RM", "RMVB", "SEA", "SIT", "SITX",
  "TAR", "TIF", "TIFF", "WAV", "WMA", "WMV", "Z", "ZIP", "PKG"
];

let activeExactExtensions = new Set();
let activeWildcardRegexes = [];
let lastSettingsSyncAt = 0;

function applyInterceptExtensions(input) {
  const rawTokens = Array.isArray(input)
    ? input
    : typeof input === "string"
      ? input.split(/[\s,;]+/)
      : [];

  const nextExact = new Set();
  const nextWildcards = [];
  const seen = new Set();

  for (const raw of rawTokens) {
    let token = String(raw || "")
      .trim()
      .replace(/^\.+/, "")
      .toUpperCase();
    if (token.startsWith("*.")) {
      token = token.slice(2);
    }
    token = token.replace(/[^A-Z0-9*]/g, "");
    if (!token || seen.has(token)) continue;
    seen.add(token);

    if (token.includes("*")) {
      const escaped = token.replace(/[.+^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*");
      nextWildcards.push(new RegExp(`^${escaped}$`, "i"));
    } else {
      nextExact.add(token);
    }
  }

  activeExactExtensions = nextExact;
  activeWildcardRegexes = nextWildcards;
}

applyInterceptExtensions(DEFAULT_IDM_EXTENSIONS);

function loadCachedExtensionsFromStorage() {
  try {
    if (chrome.storage && chrome.storage.local && typeof chrome.storage.local.get === "function") {
      chrome.storage.local.get(STORAGE_EXTENSIONS_KEY, (data) => {
        void chrome.runtime.lastError;
        if (data && data[STORAGE_EXTENSIONS_KEY]) {
          applyInterceptExtensions(data[STORAGE_EXTENSIONS_KEY]);
        }
      });
    }
  } catch (_) {}
}

function syncInterceptExtensionsFromHost(force = false) {
  const now = Date.now();
  if (!force && now - lastSettingsSyncAt < 1500) return;
  lastSettingsSyncAt = now;

  try {
    if (!chrome.runtime || typeof chrome.runtime.sendNativeMessage !== "function") return;
    chrome.runtime.sendNativeMessage(NATIVE_HOST_NAME, { action: "get_settings" }, (resp) => {
      if (chrome.runtime.lastError || !resp || resp.status === "error") {
        return;
      }
      const list = resp.interceptExtensions ?? resp.intercept_extensions;
      const hasContent = Array.isArray(list)
        ? list.length > 0
        : typeof list === "string" && list.trim().length > 0;
      if (hasContent) {
        applyInterceptExtensions(list);
        try {
          if (chrome.storage && chrome.storage.local && typeof chrome.storage.local.set === "function") {
            chrome.storage.local.set({ [STORAGE_EXTENSIONS_KEY]: list });
          }
        } catch (_) {}
      }
    });
  } catch (_) {}
}

loadCachedExtensionsFromStorage();
syncInterceptExtensionsFromHost(true);

function isHttpUrl(value) {
  return typeof value === "string" && /^https?:\/\//i.test(value.trim());
}

function isYouTubeHost(url) {
  if (!isHttpUrl(url)) return false;
  try {
    const h = new URL(url).hostname.toLowerCase();
    return (
      h === "youtube.com" ||
      h.endsWith(".youtube.com") ||
      h === "youtube-nocookie.com" ||
      h.endsWith(".youtube-nocookie.com") ||
      h === "youtu.be" ||
      h === "googlevideo.com" ||
      h.endsWith(".googlevideo.com")
    );
  } catch (_) {
    return false;
  }
}

function canonicalizeGrabUrl(rawUrl) {
  if (!isHttpUrl(rawUrl)) return "";
  try {
    const parsed = new URL(rawUrl.trim());
    const h = parsed.hostname.toLowerCase();
    if (
      h === "youtube.com" ||
      h.endsWith(".youtube.com") ||
      h === "youtube-nocookie.com" ||
      h.endsWith(".youtube-nocookie.com") ||
      h === "youtu.be"
    ) {
      const parts = parsed.pathname.split("/").filter(Boolean);
      let videoId = parsed.searchParams.get("v") || "";
      if (!videoId && (parts[0] === "shorts" || parts[0] === "embed" || parts[0] === "live")) {
        videoId = parts[1] || "";
      }
      if (!videoId && h === "youtu.be") {
        videoId = parts[0] || "";
      }
      if (videoId) {
        return `https://www.youtube.com/watch?v=${encodeURIComponent(videoId)}`;
      }
    }
    return parsed.toString();
  } catch (_) {
    return rawUrl.trim();
  }
}

function sanitizeFileName(raw, fallback = "video") {
  const cleaned = String(raw || "")
    .replace(/\s*-\s*YouTube\s*$/i, "")
    .replace(/[\\/:*?"<>|]/g, "_")
    .replace(/\s+/g, " ")
    .trim();
  return cleaned || fallback;
}

function extractExtensionFromNameOrUrl(input) {
  if (!input) return "";
  let candidate = String(input);
  try {
    if (isHttpUrl(candidate)) {
      candidate = new URL(candidate).pathname;
    }
  } catch (_) {}
  const base = (candidate.split(/[/\\]/).pop() || "").split("?")[0].split("#")[0];
  const dotIdx = base.lastIndexOf(".");
  if (dotIdx <= 0 || dotIdx === base.length - 1) return "";
  return base.slice(dotIdx + 1).trim().toUpperCase();
}

function shouldInterceptExtension(ext) {
  if (!ext) return false;
  const normalized = String(ext).trim().replace(/^\.+/, "").toUpperCase();
  if (!normalized) return false;
  if (activeExactExtensions.has(normalized)) return true;
  return activeWildcardRegexes.some((rx) => rx.test(normalized));
}

function extractFileName(url, contentDisposition) {
  if (contentDisposition) {
    const fnMatch = contentDisposition.match(/filename\*?=(?:UTF-8'')?"?([^";]+)"?/i);
    if (fnMatch && fnMatch[1]) {
      try {
        return decodeURIComponent(fnMatch[1].trim());
      } catch (_) {
        return fnMatch[1].trim();
      }
    }
  }
  try {
    const parsed = new URL(url);
    const pathname = parsed.pathname;
    const lastSeg = pathname.substring(pathname.lastIndexOf("/") + 1);
    if (lastSeg && lastSeg.includes(".")) {
      return decodeURIComponent(lastSeg);
    }
  } catch (_) {}
  return "media_stream.mp4";
}

function formatBytes(bytes) {
  if (!bytes || bytes === 0) return "Stream";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${parseFloat((bytes / Math.pow(k, i)).toFixed(1))} ${sizes[i]}`;
}

function getOriginFromUrl(rawUrl) {
  if (!isHttpUrl(rawUrl)) return "";
  try {
    return new URL(rawUrl).origin;
  } catch (_) {
    return "";
  }
}

function updateTabBadge(tabId) {
  const regular = mediaByTab.get(tabId) || [];
  const total = regular.length;
  chrome.action.setBadgeText({ text: total > 0 ? String(total) : "", tabId }).catch(() => {});
  if (total > 0) {
    chrome.action.setBadgeBackgroundColor({ color: "#0284c7", tabId }).catch(() => {});
  }
}

// Collect full cookie attributes + HTTP Cookie header using chrome.cookies.getAll
async function gatherCookieContext(urls, storeId = undefined) {
  const targets = new Set();
  for (const rawUrl of urls) {
    if (!isHttpUrl(rawUrl)) continue;
    targets.add(rawUrl);
    if (isYouTubeHost(rawUrl)) {
      targets.add("https://www.youtube.com/");
      targets.add("https://youtube.com/");
      targets.add("https://accounts.google.com/");
    }
  }

  const byKey = new Map();
  const headerPairs = new Map();

  for (const target of targets) {
    try {
      const query = storeId ? { url: target, storeId } : { url: target };
      const list = await chrome.cookies.getAll(query);
      for (const c of list || []) {
        if (!c || !c.name) continue;
        const uniqueKey = `${c.domain}|${c.path || "/"}|${c.name}`;
        if (!byKey.has(uniqueKey)) {
          byKey.set(uniqueKey, {
            domain: c.domain || "",
            path: c.path || "/",
            secure: Boolean(c.secure),
            expires: typeof c.expirationDate === "number" ? Math.floor(c.expirationDate) : 0,
            http_only: Boolean(c.httpOnly),
            name: c.name,
            value: c.value || ""
          });
        }
        headerPairs.set(c.name, c.value || "");
      }
    } catch (_) {}
  }

  const cookieHeader = Array.from(headerPairs.entries())
    .map(([k, v]) => `${k}=${v}`)
    .join("; ");

  return {
    cookieHeader,
    cookieEntries: Array.from(byKey.values())
  };
}

// 1. Generic Network Sniffer for direct non-YouTube HTTP streams (.mp4/.m3u8/.mpd)
chrome.webRequest.onHeadersReceived.addListener(
  (details) => {
    if (!details.responseHeaders || details.tabId <= 0) return;

    // Skip raw MSE chunk traffic (e.g. googlevideo.com) — YouTube & major platforms use OctoMedia directly
    if (isYouTubeHost(details.url) || isYouTubeHost(details.initiator || details.documentUrl || "")) {
      return;
    }

    let contentType = "";
    let contentLength = 0;
    let contentDisposition = "";

    for (const h of details.responseHeaders) {
      const name = h.name.toLowerCase();
      if (name === "content-type" && h.value) {
        contentType = h.value.toLowerCase().split(";")[0].trim();
      } else if (name === "content-length" && h.value) {
        contentLength = parseInt(h.value, 10) || 0;
      } else if (name === "content-disposition" && h.value) {
        contentDisposition = h.value;
      }
    }

    const isMediaMime = MEDIA_MIMES.some((m) => contentType.startsWith(m));
    const isMediaExt = MEDIA_EXT_REGEX.test(details.url);

    if (
      (isMediaMime || isMediaExt) &&
      (contentLength > 512 * 1024 ||
        isMediaMime ||
        details.url.includes(".m3u8") ||
        details.url.includes(".mpd"))
    ) {
      const fileName = extractFileName(details.url, contentDisposition);
      const tabId = details.tabId;

      if (!mediaByTab.has(tabId)) {
        mediaByTab.set(tabId, []);
      }

      const list = mediaByTab.get(tabId);
      if (!list.some((item) => item.url === details.url)) {
        const item = {
          id: `${tabId}_${Date.now()}_${Math.random().toString(36).substring(2, 7)}`,
          url: details.url,
          file_name: fileName,
          mime: contentType || "video/mp4",
          total_bytes: contentLength,
          formatted_size: formatBytes(contentLength),
          initiator: details.initiator || "",
          timestamp: Date.now()
        };

        list.unshift(item);
        if (list.length > 50) list.pop();

        updateTabBadge(tabId);
        chrome.tabs.sendMessage(tabId, { type: "MEDIA_SNIFFED", media: item }).catch(() => {});
      }
    }
  },
  { urls: ["<all_urls>"] },
  ["responseHeaders"]
);

chrome.tabs.onUpdated.addListener((tabId, changeInfo) => {
  if (changeInfo.status === "loading") {
    mediaByTab.delete(tabId);
    chrome.action.setBadgeText({ text: "", tabId }).catch(() => {});
  }
});

chrome.tabs.onRemoved.addListener((tabId) => {
  mediaByTab.delete(tabId);
});

if (chrome.tabs.onActivated && typeof chrome.tabs.onActivated.addListener === "function") {
  chrome.tabs.onActivated.addListener(() => {
    syncInterceptExtensionsFromHost(false);
  });
}
if (chrome.runtime.onStartup && typeof chrome.runtime.onStartup.addListener === "function") {
  chrome.runtime.onStartup.addListener(() => {
    loadCachedExtensionsFromStorage();
    syncInterceptExtensionsFromHost(true);
  });
}
if (chrome.runtime.onInstalled && typeof chrome.runtime.onInstalled.addListener === "function") {
  chrome.runtime.onInstalled.addListener(() => {
    syncInterceptExtensionsFromHost(true);
  });
}
if (chrome.downloads.onCreated && typeof chrome.downloads.onCreated.addListener === "function") {
  chrome.downloads.onCreated.addListener(() => {
    syncInterceptExtensionsFromHost(false);
  });
}

// 2. Canonical Snake-Case Native Messaging Payload Builders (zero duplicate JSON keys)
function buildDirectDownloadPayload(payload, cookieHeader) {
  const primaryUrl = String(payload.url || "").trim();
  const referrerUrl = String(payload.referrer || "").trim();
  const resolvedFileName = sanitizeFileName(payload.file_name || "", "");
  const resolvedTitle = sanitizeFileName(
    payload.title ||
      (resolvedFileName ? resolvedFileName.replace(/\.[a-z0-9]{2,5}$/i, "") : ""),
    ""
  );

  const ua = (navigator && navigator.userAgent) || "";
  const headers = {};
  if (ua) headers["User-Agent"] = ua;
  if (referrerUrl) {
    headers.Referer = referrerUrl;
    const origin = getOriginFromUrl(referrerUrl);
    if (origin) headers.Origin = origin;
  }
  if (cookieHeader) {
    headers.Cookie = cookieHeader;
  }

  const nativeMessage = {
    action: payload.action || "download",
    url: primaryUrl
  };

  if (resolvedFileName) nativeMessage.file_name = resolvedFileName;
  if (resolvedTitle) nativeMessage.title = resolvedTitle;
  if (cookieHeader) nativeMessage.cookies = cookieHeader;
  if (referrerUrl) nativeMessage.referrer = referrerUrl;
  if (ua) nativeMessage.user_agent = ua;
  if (Object.keys(headers).length > 0) nativeMessage.headers = headers;
  if (payload.mime) nativeMessage.mime = payload.mime;
  if (typeof payload.total_bytes === "number" && payload.total_bytes > 0) {
    nativeMessage.total_bytes = payload.total_bytes;
  }

  return nativeMessage;
}

function sendToNativeHost(nativeMessage) {
  return new Promise((resolve) => {
    chrome.runtime.sendNativeMessage(NATIVE_HOST_NAME, nativeMessage, (resp) => {
      if (chrome.runtime.lastError) {
        console.warn("[OctoHeft Native Host]", chrome.runtime.lastError.message);
        resolve({ success: false, error: chrome.runtime.lastError.message });
      } else if (!resp || resp.status === "error") {
        const errText = (resp && resp.error) || "Native host returned an error.";
        console.warn("[OctoHeft Native Host Rejected]", errText);
        resolve({ success: false, code: "NATIVE_ERROR", error: errText, response: resp });
      } else {
        console.log("[OctoHeft Native Host Response]", resp);
        resolve({ success: true, response: resp });
      }
    });
  });
}

async function dispatchDirectDownloadToNativeHost(payload, storeId = undefined) {
  try {
    const primaryUrl = String(payload.url || "").trim();
    if (!isHttpUrl(primaryUrl)) {
      return {
        success: false,
        code: "INVALID_URL",
        error: "Direct HTTP/HTTPS URL required (blob: URLs cannot be downloaded directly)."
      };
    }

    const { cookieHeader } = await gatherCookieContext([primaryUrl, payload.referrer], storeId);
    const nativeMessage = buildDirectDownloadPayload(payload, cookieHeader);
    return await sendToNativeHost(nativeMessage);
  } catch (err) {
    console.error("[OctoHeft Extension Error]", err);
    return { success: false, error: err.message };
  }
}

async function dispatchGrabVideoToNativeHost(request, sender) {
  try {
    const rawPageUrl = request.pageUrl || (sender.tab && sender.tab.url) || "";
    const targetUrl = canonicalizeGrabUrl(rawPageUrl);
    const fallbackDirect = isHttpUrl(request.candidateSrc) ? request.candidateSrc.trim() : "";

    const effectiveUrl = targetUrl || fallbackDirect;
    if (!isHttpUrl(effectiveUrl)) {
      return {
        success: false,
        code: "INVALID_URL",
        error: "Active tab URL is not a supported HTTP/HTTPS media page."
      };
    }

    const storeId = sender.tab ? sender.tab.cookieStoreId : undefined;
    const { cookieHeader, cookieEntries } = await gatherCookieContext(
      [effectiveUrl, rawPageUrl, request.referrer, fallbackDirect],
      storeId
    );

    const ua = (navigator && navigator.userAgent) || "";
    const cleanTitle = sanitizeFileName(
      request.pageTitle || (sender.tab && sender.tab.title) || "",
      ""
    );

    const nativeMessage = {
      action: "grab_video",
      url: effectiveUrl
    };

    if (cleanTitle) nativeMessage.title = cleanTitle;
    if (fallbackDirect && fallbackDirect !== effectiveUrl) {
      nativeMessage.video_url = fallbackDirect;
    }
    if (cookieHeader) nativeMessage.cookies = cookieHeader;
    if (cookieEntries.length > 0) nativeMessage.cookie_entries = cookieEntries;
    if (isHttpUrl(request.referrer || rawPageUrl)) {
      nativeMessage.referrer = request.referrer || rawPageUrl;
    }
    if (ua) nativeMessage.user_agent = ua;

    return await sendToNativeHost(nativeMessage);
  } catch (err) {
    console.error("[OctoHeft Grab Error]", err);
    return { success: false, error: err.message };
  }
}

// 3. Global Browser Download Interception (IDM onDeterminingFilename Hook)
chrome.downloads.onDeterminingFilename.addListener((downloadItem, suggest) => {
  const targetUrl = downloadItem.finalUrl || downloadItem.url || "";

  // Leave blob:, data:, file:, and extension-initiated transfers to Chrome natively
  if (!isHttpUrl(targetUrl) || downloadItem.byExtensionId === chrome.runtime.id) {
    suggest();
    return;
  }

  const rawFilename = (downloadItem.filename || "").split(/[/\\]/).pop() || "";
  const extFromName = extractExtensionFromNameOrUrl(rawFilename);
  const extFromUrl = extractExtensionFromNameOrUrl(targetUrl);
  const effectiveExt = extFromName || extFromUrl;

  if (!shouldInterceptExtension(effectiveExt)) {
    suggest();
    return;
  }

  const resolvedFilename = rawFilename || extractFileName(targetUrl, "");

  chrome.downloads.cancel(downloadItem.id, () => {
    void chrome.runtime.lastError;
    chrome.downloads.erase({ id: downloadItem.id }, () => {
      void chrome.runtime.lastError;
    });
  });

  (async () => {
    const { cookieHeader } = await gatherCookieContext([downloadItem.url, targetUrl]);
    const referrer =
      downloadItem.referrer ||
      (downloadItem.url && downloadItem.url !== targetUrl ? downloadItem.url : undefined);

    const payload = buildDirectDownloadPayload(
      {
        action: "download",
        url: targetUrl,
        file_name: resolvedFilename,
        referrer,
        mime: downloadItem.mime || undefined,
        total_bytes: downloadItem.totalBytes > 0 ? downloadItem.totalBytes : undefined
      },
      cookieHeader
    );

    chrome.runtime.sendNativeMessage(NATIVE_HOST_NAME, payload, () => {
      void chrome.runtime.lastError;
    });
  })();
});

// 4. Message Listener for Content Scripts & Popup
chrome.runtime.onMessage.addListener((request, sender, sendResponse) => {
  if (request.type === "SYNC_SETTINGS") {
    syncInterceptExtensionsFromHost(true);
    sendResponse({ ok: true });
    return false;
  }

  if (request.type === "GRAB_VIDEO") {
    dispatchGrabVideoToNativeHost(request, sender).then(sendResponse);
    return true;
  }

  if (request.type === "GET_TAB_MEDIA") {
    const tabId = request.tabId || (sender.tab && sender.tab.id);
    const baseMedia = [...(mediaByTab.get(tabId) || [])];
    sendResponse({ media: baseMedia });
    return false;
  }

  if (request.type === "DOWNLOAD_MEDIA") {
    const storeId = request.cookieStoreId || (sender.tab ? sender.tab.cookieStoreId : undefined);
    const payload = {
      action: "download_media",
      title: request.title,
      url: request.url,
      file_name: request.file_name,
      mime: request.mime,
      total_bytes: request.total_bytes,
      referrer: request.referrer || (sender.tab ? sender.tab.url : undefined)
    };
    dispatchDirectDownloadToNativeHost(payload, storeId).then(sendResponse);
    return true;
  }
});
