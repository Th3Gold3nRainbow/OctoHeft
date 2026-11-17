// OctoHeft Floating Video Grabber Content Script (Clean OctoMedia Handoff)

(function () {
  const OVERLAY_CLASS = "octoheft-video-overlay-btn";
  const VISIBLE_CLASS = "octoheft-visible";
  const HOVER_LOCK_CLASS = "octoheft-hover-lock";
  const SENDING_CLASS = "octoheft-sending";
  const SUCCESS_CLASS = "octoheft-success";
  const ERROR_CLASS = "octoheft-error";
  const PROCESSED_ATTR = "data-octoheft-attached";
  const HIDE_GRACE_MS = 800;

  const attachedState = new WeakMap();

  function cleanPageTitle() {
    return (
      String(document.title || "")
        .replace(/\s*-\s*YouTube\s*$/i, "")
        .replace(/[\\/:*?"<>|]/g, "_")
        .replace(/\s+/g, " ")
        .trim() || "video"
    );
  }

  function canonicalizePageUrl() {
    const rawHref = String(window.location.href || "").trim();
    try {
      const parsed = new URL(rawHref);
      const host = parsed.hostname.toLowerCase();
      if (
        host === "youtube.com" ||
        host.endsWith(".youtube.com") ||
        host === "youtube-nocookie.com" ||
        host.endsWith(".youtube-nocookie.com") ||
        host === "youtu.be"
      ) {
        const parts = parsed.pathname.split("/").filter(Boolean);
        let videoId = parsed.searchParams.get("v") || "";
        if (!videoId && (parts[0] === "shorts" || parts[0] === "embed" || parts[0] === "live")) {
          videoId = parts[1] || "";
        }
        if (!videoId && host === "youtu.be") {
          videoId = parts[0] || "";
        }
        if (videoId) {
          return `https://www.youtube.com/watch?v=${encodeURIComponent(videoId)}`;
        }
      }
    } catch (_) {}
    return rawHref;
  }

  function defaultButtonContent() {
    return `
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2"></polygon>
      </svg>
      <span>Download Video</span>
    `;
  }

  function resolvePlayerContainer(mediaElement) {
    return (
      mediaElement.closest("#movie_player") ||
      mediaElement.closest("#shorts-player") ||
      mediaElement.closest(".html5-video-player") ||
      mediaElement.parentElement
    );
  }

  function detachExistingState(state) {
    if (!state) return;
    if (state.hideTimer !== null) {
      clearTimeout(state.hideTimer);
      state.hideTimer = null;
    }
    if (state.cleanupListeners) {
      state.cleanupListeners();
    }
    if (state.btn && state.btn.parentElement) {
      state.btn.remove();
    }
  }

  function createOverlayButton(mediaElement) {
    const targetContainer = resolvePlayerContainer(mediaElement);
    if (!targetContainer) return;

    const existing = attachedState.get(mediaElement);
    if (existing) {
      if (
        existing.container === targetContainer &&
        existing.btn.isConnected &&
        existing.btn.parentElement === targetContainer
      ) {
        return;
      }
      detachExistingState(existing);
    }

    // Avoid duplicate overlay widgets inside the same player container
    const staleBtn = targetContainer.querySelector(`:scope > .${OVERLAY_CLASS}`);
    if (staleBtn && (!existing || existing.btn !== staleBtn)) {
      staleBtn.remove();
    }

    mediaElement.setAttribute(PROCESSED_ATTR, "true");

    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = OVERLAY_CLASS;
    btn.innerHTML = defaultButtonContent();
    btn.title = "Open in OctoMedia Video Grabber";

    let hideTimer = null;
    let buttonHovered = false;

    const enforceVisible = () => {
      if (hideTimer !== null) {
        clearTimeout(hideTimer);
        hideTimer = null;
      }
      btn.classList.add(VISIBLE_CLASS);
      btn.style.opacity = "1";
      btn.style.pointerEvents = "auto";
    };

    const scheduleHide = () => {
      if (buttonHovered) return;
      if (hideTimer !== null) {
        clearTimeout(hideTimer);
      }
      hideTimer = setTimeout(() => {
        hideTimer = null;
        if (buttonHovered) return;
        btn.classList.remove(VISIBLE_CLASS);
        btn.style.opacity = "";
        btn.style.pointerEvents = "";
      }, HIDE_GRACE_MS);
    };

    const onContainerEnter = () => enforceVisible();
    const onContainerMove = () => enforceVisible();
    const onContainerLeave = () => scheduleHide();

    const onBtnMouseEnter = (e) => {
      e.stopPropagation();
      buttonHovered = true;
      btn.classList.add(HOVER_LOCK_CLASS);
      enforceVisible();
    };

    const onBtnMouseLeave = (e) => {
      e.stopPropagation();
      buttonHovered = false;
      btn.classList.remove(HOVER_LOCK_CLASS);
      scheduleHide();
    };

    const stopBubble = (e) => {
      e.stopPropagation();
    };

    const showFeedback = (isError, label) => {
      btn.classList.remove(SENDING_CLASS);
      btn.classList.add(isError ? ERROR_CLASS : SUCCESS_CLASS);
      btn.textContent = label;
      setTimeout(() => {
        btn.classList.remove(ERROR_CLASS, SUCCESS_CLASS);
        btn.innerHTML = defaultButtonContent();
      }, 3000);
    };

    const onBtnClick = (e) => {
      e.preventDefault();
      e.stopPropagation();
      enforceVisible();

      const candidateSrc =
        mediaElement.currentSrc ||
        mediaElement.src ||
        (mediaElement.querySelector("source") && mediaElement.querySelector("source").src) ||
        "";

      btn.classList.add(SENDING_CLASS);
      btn.textContent = "Opening OctoMedia...";

      chrome.runtime.sendMessage(
        {
          type: "GRAB_VIDEO",
          pageUrl: canonicalizePageUrl(),
          referrer: window.location.href,
          pageTitle: cleanPageTitle(),
          candidateSrc: candidateSrc.startsWith("http") ? candidateSrc : ""
        },
        (res) => {
          setTimeout(() => {
            if (chrome.runtime.lastError || !res || res.success === false) {
              if (res && res.error) {
                btn.title = res.error;
              } else if (chrome.runtime.lastError && chrome.runtime.lastError.message) {
                btn.title = chrome.runtime.lastError.message;
              }
              showFeedback(true, "! Host Missing");
              return;
            }
            showFeedback(false, "✓ Opened in OctoMedia");
          }, 150);
        }
      );
    };

    // Anchor cleanly relative to #movie_player or direct parent container
    const computedPos = window.getComputedStyle(targetContainer).position;
    if (computedPos === "static") {
      targetContainer.style.position = "relative";
    }
    targetContainer.appendChild(btn);

    // Track hover across both container (#movie_player) and video canvas with 800ms grace period
    targetContainer.addEventListener("mouseenter", onContainerEnter, { passive: true });
    targetContainer.addEventListener("mousemove", onContainerMove, { passive: true });
    targetContainer.addEventListener("mouseleave", onContainerLeave, { passive: true });

    if (targetContainer !== mediaElement && targetContainer !== mediaElement.parentElement) {
      mediaElement.addEventListener("mouseenter", onContainerEnter, { passive: true });
      mediaElement.addEventListener("mousemove", onContainerMove, { passive: true });
      mediaElement.addEventListener("mouseleave", onContainerLeave, { passive: true });
    }

    // Lock hover + prevent player controls from swallowing clicks/hovers
    btn.addEventListener("mouseenter", onBtnMouseEnter);
    btn.addEventListener("mouseleave", onBtnMouseLeave);
    btn.addEventListener("mouseover", stopBubble);
    btn.addEventListener("mousedown", stopBubble);
    btn.addEventListener("pointerdown", stopBubble);
    btn.addEventListener("click", onBtnClick);

    const cleanupListeners = () => {
      targetContainer.removeEventListener("mouseenter", onContainerEnter);
      targetContainer.removeEventListener("mousemove", onContainerMove);
      targetContainer.removeEventListener("mouseleave", onContainerLeave);
      mediaElement.removeEventListener("mouseenter", onContainerEnter);
      mediaElement.removeEventListener("mousemove", onContainerMove);
      mediaElement.removeEventListener("mouseleave", onContainerLeave);
      btn.removeEventListener("mouseenter", onBtnMouseEnter);
      btn.removeEventListener("mouseleave", onBtnMouseLeave);
      btn.removeEventListener("mouseover", stopBubble);
      btn.removeEventListener("mousedown", stopBubble);
      btn.removeEventListener("pointerdown", stopBubble);
      btn.removeEventListener("click", onBtnClick);
    };

    attachedState.set(mediaElement, {
      btn,
      container: targetContainer,
      get hideTimer() {
        return hideTimer;
      },
      set hideTimer(val) {
        hideTimer = val;
      },
      cleanupListeners
    });
  }

  function scanMediaElements() {
    const videos = document.querySelectorAll("video");
    videos.forEach((v) => {
      const rect = v.getBoundingClientRect();
      if (rect.width > 150 && rect.height > 100) {
        createOverlayButton(v);
      }
    });
  }

  scanMediaElements();

  const observer = new MutationObserver(() => {
    scanMediaElements();
  });

  if (document.body) {
    observer.observe(document.body, { childList: true, subtree: true });
  } else {
    document.addEventListener("DOMContentLoaded", () => {
      if (document.body) {
        observer.observe(document.body, { childList: true, subtree: true });
        scanMediaElements();
      }
    });
  }

  // Re-attach overlay smoothly during YouTube SPA navigation transitions
  window.addEventListener("yt-navigate-finish", () => {
    setTimeout(scanMediaElements, 300);
  });
})();
