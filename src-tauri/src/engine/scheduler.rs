use chrono::{Datelike, Local, Timelike};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tauri::AppHandle;

use crate::engine::types::DownloadStatus;
use crate::engine::DownloadEngine;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PostDownloadAction {
    None,
    ExitApp,
    SleepPC,
    ShutdownPC,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueConfig {
    pub id: String,
    pub name: String,
    pub max_concurrent_downloads: usize,
    pub start_time: Option<String>, // "HH:MM" 24h format
    pub stop_time: Option<String>,  // "HH:MM" 24h format
    pub days_of_week: u8,           // Bitmask: bit 0 = Sun, bit 1 = Mon ... bit 6 = Sat (127 = All)
    pub post_action: PostDownloadAction,
    pub is_enabled: bool,
    pub task_ids: Vec<String>,
}

pub struct QueueScheduler {
    queues: RwLock<HashMap<String, QueueConfig>>,
    storage_path: PathBuf,
}

impl QueueScheduler {
    pub fn new(storage_dir: &Path) -> Self {
        let storage_path = storage_dir.join("queues_state.json");
        let scheduler = Self {
            queues: RwLock::new(HashMap::new()),
            storage_path,
        };

        scheduler.load_from_disk();
        scheduler
    }

    fn load_from_disk(&self) {
        if !self.storage_path.exists() {
            // Initialize default queue
            let default_queue = QueueConfig {
                id: "main_queue".to_string(),
                name: "Main Download Queue".to_string(),
                max_concurrent_downloads: 2,
                start_time: None,
                stop_time: None,
                days_of_week: 127,
                post_action: PostDownloadAction::None,
                is_enabled: false,
                task_ids: Vec::new(),
            };
            let mut map = self.queues.write();
            map.insert("main_queue".to_string(), default_queue);
            return;
        }

        if let Ok(data) = fs::read_to_string(&self.storage_path) {
            if let Ok(list) = serde_json::from_str::<Vec<QueueConfig>>(&data) {
                let mut map = self.queues.write();
                for q in list {
                    map.insert(q.id.clone(), q);
                }
            }
        }
    }

    pub fn save_to_disk(&self) {
        if let Some(parent) = self.storage_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let list: Vec<QueueConfig> = self.queues.read().values().cloned().collect();
        if let Ok(json) = serde_json::to_string_pretty(&list) {
            let _ = fs::write(&self.storage_path, json);
        }
    }

    pub fn get_all_queues(&self) -> Vec<QueueConfig> {
        let mut list: Vec<QueueConfig> = self.queues.read().values().cloned().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    pub fn get_queue(&self, id: &str) -> Option<QueueConfig> {
        self.queues.read().get(id).cloned()
    }

    pub fn save_queue(&self, config: QueueConfig) {
        {
            let mut map = self.queues.write();
            map.insert(config.id.clone(), config);
        }
        self.save_to_disk();
    }

    pub fn delete_queue(&self, id: &str) -> bool {
        let removed = {
            let mut map = self.queues.write();
            map.remove(id).is_some()
        };
        if removed {
            self.save_to_disk();
        }
        removed
    }

    pub fn add_task_to_queue(&self, queue_id: &str, task_id: &str) {
        {
            let mut map = self.queues.write();
            if let Some(q) = map.get_mut(queue_id) {
                if !q.task_ids.contains(&task_id.to_string()) {
                    q.task_ids.push(task_id.to_string());
                }
            }
        }
        self.save_to_disk();
    }

    pub fn remove_task_from_queue(&self, queue_id: &str, task_id: &str) {
        {
            let mut map = self.queues.write();
            if let Some(q) = map.get_mut(queue_id) {
                q.task_ids.retain(|id| id != task_id);
            }
        }
        self.save_to_disk();
    }

    pub fn reorder_task_in_queue(&self, queue_id: &str, task_id: &str, new_index: usize) -> bool {
        let changed = {
            let mut map = self.queues.write();
            if let Some(q) = map.get_mut(queue_id) {
                if let Some(pos) = q.task_ids.iter().position(|id| id == task_id) {
                    let id = q.task_ids.remove(pos);
                    let target = new_index.min(q.task_ids.len());
                    q.task_ids.insert(target, id);
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };
        if changed {
            self.save_to_disk();
        }
        changed
    }

    pub fn sync_task_order_across_queues(&self, task_id: &str, move_up: bool) {
        let mut changed = false;
        {
            let mut map = self.queues.write();
            for q in map.values_mut() {
                if let Some(pos) = q.task_ids.iter().position(|id| id == task_id) {
                    if move_up && pos > 0 {
                        q.task_ids.swap(pos, pos - 1);
                        changed = true;
                    } else if !move_up && pos + 1 < q.task_ids.len() {
                        q.task_ids.swap(pos, pos + 1);
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.save_to_disk();
        }
    }

    pub fn import_queues(&self, incoming: Vec<QueueConfig>) -> usize {
        let mut count = 0;
        {
            let mut map = self.queues.write();
            for mut q in incoming {
                q.is_enabled = false; // Require explicit manual or scheduled activation after import
                map.insert(q.id.clone(), q);
                count += 1;
            }
        }
        self.save_to_disk();
        count
    }

    /// Background scheduler loop polling every second
    pub fn start_scheduler_loop(
        self: Arc<Self>,
        engine: Arc<DownloadEngine>,
        app_handle: AppHandle,
    ) {
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));

            loop {
                interval.tick().await;

                let now = Local::now();
                let current_hm = format!("{:02}:{:02}", now.hour(), now.minute());
                let weekday_bit = 1u8 << now.weekday().num_days_from_sunday();

                let active_queues: Vec<QueueConfig> = {
                    let map = self.queues.read();
                    map.values().cloned().collect()
                };

                for mut q in active_queues {
                    // Check day of week filter
                    if (q.days_of_week & weekday_bit) == 0 {
                        continue;
                    }

                    // Check scheduled stop time
                    if let Some(ref stop_t) = q.stop_time {
                        if stop_t == &current_hm && now.second() == 0 && q.is_enabled {
                            q.is_enabled = false;
                            self.save_queue(q.clone());
                            // Pause active tasks in this queue
                            for tid in &q.task_ids {
                                let _ = engine.pause_download(tid);
                            }
                            continue;
                        }
                    }

                    // Check scheduled start time
                    if let Some(ref start_t) = q.start_time {
                        if start_t == &current_hm && now.second() == 0 && !q.is_enabled {
                            q.is_enabled = true;
                            self.save_queue(q.clone());
                        }
                    }

                    if !q.is_enabled {
                        continue;
                    }

                    // Count currently active downloading tasks in this queue
                    let mut running_count = 0;
                    let mut pending_ids = Vec::new();
                    let mut all_completed = true;

                    for tid in &q.task_ids {
                        if let Some(task) = engine.state_store.get_task(tid) {
                            match task.status {
                                DownloadStatus::Downloading | DownloadStatus::Connecting => {
                                    running_count += 1;
                                    all_completed = false;
                                }
                                DownloadStatus::Queued | DownloadStatus::Paused => {
                                    pending_ids.push(tid.clone());
                                    all_completed = false;
                                }
                                DownloadStatus::Error => {
                                    // Treat error as not completed, but don't auto-retry indefinitely
                                    all_completed = false;
                                }
                                DownloadStatus::Completed => {
                                    // Completed
                                }
                            }
                        }
                    }

                    // If concurrency cap allows, start next pending tasks
                    let slots_available = q.max_concurrent_downloads.saturating_sub(running_count);
                    for tid in pending_ids.into_iter().take(slots_available) {
                        let is_ytdlp = engine
                            .state_store
                            .get_task(&tid)
                            .and_then(|t| t.media_info)
                            .map(|mi| mi.protocol == "YT-DLP")
                            .unwrap_or(false);
                        if is_ytdlp {
                            let _ = crate::engine::omniget::downloader::resume_omniget_task(
                                &tid,
                                engine.clone(),
                                Some(app_handle.clone()),
                            );
                        } else {
                            let _ = engine.start_download(&tid, app_handle.clone());
                        }
                    }

                    // If all tasks in queue are finished and queue is active, trigger post action
                    if all_completed && !q.task_ids.is_empty() {
                        q.is_enabled = false;
                        self.save_queue(q.clone());

                        match q.post_action {
                            PostDownloadAction::None => {}
                            PostDownloadAction::ExitApp => {
                                std::process::exit(0);
                            }
                            PostDownloadAction::SleepPC => {
                                execute_system_sleep();
                            }
                            PostDownloadAction::ShutdownPC => {
                                execute_system_shutdown();
                            }
                        }
                    }
                }
            }
        });
    }
}

fn execute_system_sleep() {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("rundll32.exe")
            .args(["powrprof.dll,SetSuspendState", "0,1,0"])
            .spawn();
    }
}

fn execute_system_shutdown() {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("shutdown.exe")
            .args(["/s", "/t", "30", "/c", "OctoHeft download queue completed"])
            .spawn();
    }
}
