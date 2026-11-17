document.addEventListener("DOMContentLoaded", async () => {
  const container = document.getElementById("media-container");
  const countEl = document.getElementById("media-count");
  const tabTitleEl = document.getElementById("tab-title");
  const grabBtn = document.getElementById("btn-grab-page");

  chrome.runtime.sendMessage({ type: "SYNC_SETTINGS" }).catch(() => {});

  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  if (!tab) return;

  const pageUrl = tab.url || "";
  const pageTitle = tab.title || pageUrl || "Current Tab";
  const isHttpPage = /^https?:\/\//i.test(pageUrl);

  tabTitleEl.textContent = pageTitle;
  tabTitleEl.title = pageUrl;

  if (!isHttpPage) {
    grabBtn.disabled = true;
    grabBtn.textContent = "Open an HTTP/HTTPS video page";
  } else {
    grabBtn.addEventListener("click", () => {
      grabBtn.disabled = true;
      grabBtn.textContent = "Opening OctoMedia...";

      chrome.runtime.sendMessage(
        {
          type: "GRAB_VIDEO",
          tabId: tab.id,
          pageUrl,
          referrer: pageUrl,
          pageTitle
        },
        (res) => {
          if (chrome.runtime.lastError || !res || res.success === false) {
            grabBtn.textContent = "! Native Host Missing";
            grabBtn.title =
              (res && res.error) ||
              (chrome.runtime.lastError && chrome.runtime.lastError.message) ||
              "Repair registry keys in OctoHeft Settings";
          } else {
            grabBtn.textContent = "✓ Opened in OctoMedia";
          }
          setTimeout(() => {
            grabBtn.disabled = false;
            grabBtn.textContent = "Open Page in OctoMedia";
          }, 2200);
        }
      );
    });
  }

  chrome.runtime.sendMessage(
    {
      type: "GET_TAB_MEDIA",
      tabId: tab.id,
      pageUrl,
      pageTitle
    },
    (response) => {
      const list = (response && response.media) || [];
      countEl.textContent = `${list.length} stream${list.length !== 1 ? "s" : ""}`;

      if (list.length === 0) return;

      container.innerHTML = "";

      list.forEach((item) => {
        const fileName = item.file_name || "video.mp4";
        const formattedSize = item.formatted_size || "Stream";
        const totalBytes = item.total_bytes || 0;

        const card = document.createElement("div");
        card.className = "media-card";

        card.innerHTML = `
          <div class="media-name">${escapeHtml(fileName)}</div>
          <div class="media-meta">
            <span class="quality-badge">${escapeHtml(item.mime || "video/mp4")}</span>
            <span>${escapeHtml(formattedSize)}</span>
          </div>
          <button type="button" class="btn-download" data-name="${escapeHtml(fileName)}">
            Download with OctoHeft
          </button>
        `;

        card.querySelector(".btn-download").addEventListener("click", (e) => {
          const btn = e.currentTarget;
          btn.disabled = true;
          btn.textContent = "Sending to OctoHeft...";

          const downloadMessage = {
            type: "DOWNLOAD_MEDIA",
            cookieStoreId: tab.cookieStoreId,
            url: item.url,
            file_name: fileName,
            mime: item.mime || "video/mp4",
            referrer: pageUrl
          };
          if (item.title) downloadMessage.title = item.title;
          if (totalBytes > 0) downloadMessage.total_bytes = totalBytes;

          chrome.runtime.sendMessage(downloadMessage, (res) => {
            if (chrome.runtime.lastError || !res || res.success === false) {
              btn.textContent = "! Native Host Missing";
              btn.title =
                (res && res.error) ||
                (chrome.runtime.lastError && chrome.runtime.lastError.message) ||
                "Repair registry keys in OctoHeft Settings";
            } else {
              btn.textContent = "✓ Queued in OctoHeft";
            }
            setTimeout(() => {
              btn.disabled = false;
              btn.textContent = "Download with OctoHeft";
            }, 2200);
          });
        });

        container.appendChild(card);
      });
    }
  );

  function escapeHtml(text) {
    if (!text) return "";
    return text.replace(/[&<>"']/g, (m) => ({
      "&": "&amp;",
      "<": "&lt;",
      ">": "&gt;",
      '"': "&quot;",
      "'": "&#39;"
    })[m]);
  }
});
