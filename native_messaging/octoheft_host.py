#!/usr/bin/env python3
"""
OctoHeft Native Messaging Host Bridge for Chromium & Firefox
Reads 32-bit length-prefixed JSON from standard input,
appends the download request to OctoHeft's state store,
and replies with a status confirmation.
"""

import sys
import json
import struct
import os
import uuid
import time


def read_message():
    raw_length = sys.stdin.buffer.read(4)
    if len(raw_length) == 0:
        return None
    message_length = struct.unpack("<I", raw_length)[0]
    message = sys.stdin.buffer.read(message_length).decode("utf-8")
    return json.loads(message)


def send_message(message):
    encoded = json.dumps(message).encode("utf-8")
    sys.stdout.buffer.write(struct.pack("<I", len(encoded)))
    sys.stdout.buffer.write(encoded)
    sys.stdout.buffer.flush()


def main():
    while True:
        try:
            msg = read_message()
            if msg is None:
                break

            action = msg.get("action")
            url = msg.get("video_url") or msg.get("videoUrl") or msg.get("url")

            if action in ("download", "download_media") and url:
                if str(url).lower().startswith("blob:"):
                    send_message({"status": "error", "error": "Cannot queue browser blob: URL"})
                    continue

                appdata = os.environ.get("APPDATA", ".")
                store_dir = os.path.join(appdata, "octoheft")
                os.makedirs(store_dir, exist_ok=True)
                tasks_path = os.path.join(store_dir, "tasks_state.json")

                tasks = []
                if os.path.exists(tasks_path):
                    try:
                        with open(tasks_path, "r", encoding="utf-8") as f:
                            tasks = json.load(f)
                    except Exception:
                        tasks = []

                file_name = (
                    msg.get("file_name")
                    or msg.get("filename")
                    or msg.get("fileName")
                    or msg.get("title")
                    or url.split("/")[-1].split("?")[0]
                    or "download.bin"
                )
                downloads_dir = os.path.join(os.environ.get("USERPROFILE", "."), "Downloads")
                save_path = os.path.join(downloads_dir, file_name)
                total_size = msg.get("total_bytes")
                if total_size is None:
                    total_size = msg.get("totalBytes") or 0

                headers = {}
                if isinstance(msg.get("headers"), dict):
                    for k, v in msg["headers"].items():
                        if k and v:
                            headers[str(k)] = str(v)
                if msg.get("cookies"):
                    headers["Cookie"] = str(msg["cookies"])
                if msg.get("referrer"):
                    headers["Referer"] = str(msg["referrer"])
                if msg.get("user_agent") or msg.get("userAgent"):
                    headers["User-Agent"] = str(msg.get("user_agent") or msg.get("userAgent"))

                mime_val = str(msg.get("mime") or "")
                task_id = str(uuid.uuid4())
                new_task = {
                    "id": task_id,
                    "url": url,
                    "file_name": file_name,
                    "save_path": save_path,
                    "total_size": total_size,
                    "downloaded_bytes": 0,
                    "speed": 0,
                    "eta_seconds": None,
                    "status": "queued",
                    "category": "video" if "video" in mime_val else "other",
                    "supports_range": True,
                    "connections": 8,
                    "segments": [],
                    "error_message": None,
                    "order_index": int(time.time() * 1000),
                    "headers": headers or None,
                    "created_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                    "updated_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                }

                tasks.insert(0, new_task)
                with open(tasks_path, "w", encoding="utf-8") as f:
                    json.dump(tasks, f, indent=2)

                send_message({"status": "success", "taskId": task_id, "fileName": file_name})
            else:
                send_message({"status": "ignored", "reason": "Unsupported action"})
        except Exception as e:
            send_message({"status": "error", "error": str(e)})
            break


if __name__ == "__main__":
    main()

