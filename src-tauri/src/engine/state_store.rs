use crate::engine::types::{DownloadStatus, DownloadTask, MediaStreamInfo, SegmentProgress};
use chrono::Utc;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub struct StateStore {
    tasks: RwLock<HashMap<String, DownloadTask>>,
    storage_path: PathBuf,
}

impl StateStore {
    pub fn new(storage_dir: &Path) -> Self {
        let storage_path = storage_dir.join("tasks_state.json");
        let store = Self {
            tasks: RwLock::new(HashMap::new()),
            storage_path,
        };

        store.load_from_disk();
        store
    }

    fn load_from_disk(&self) {
        if !self.storage_path.exists() {
            return;
        }

        if let Ok(data) = fs::read_to_string(&self.storage_path) {
            if let Ok(tasks_list) = serde_json::from_str::<Vec<DownloadTask>>(&data) {
                let mut map = self.tasks.write();
                for mut task in tasks_list {
                    // If task was downloading when app closed, set status to Paused
                    if task.status == DownloadStatus::Downloading || task.status == DownloadStatus::Connecting {
                        task.status = DownloadStatus::Paused;
                    }
                    map.insert(task.id.clone(), task);
                }
            }
        }
    }

    pub fn save_to_disk(&self) {
        if let Some(parent) = self.storage_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let tasks: Vec<DownloadTask> = self.tasks.read().values().cloned().collect();
        if let Ok(json) = serde_json::to_string_pretty(&tasks) {
            let _ = fs::write(&self.storage_path, json);
        }
    }

    pub fn add_task(&self, task: DownloadTask) {
        {
            let mut map = self.tasks.write();
            map.insert(task.id.clone(), task);
        }
        self.save_to_disk();
    }

    pub fn get_task(&self, id: &str) -> Option<DownloadTask> {
        self.tasks.read().get(id).cloned()
    }

    pub fn get_all_tasks(&self) -> Vec<DownloadTask> {
        let mut list: Vec<DownloadTask> = self.tasks.read().values().cloned().collect();
        list.sort_by(|a, b| {
            if a.order_index != b.order_index {
                a.order_index.cmp(&b.order_index)
            } else {
                b.created_at.cmp(&a.created_at)
            }
        });
        list
    }

    pub fn reorder_task(&self, task_id: &str, new_index: usize) -> Result<(), String> {
        let mut list = self.get_all_tasks();
        let current_pos = list.iter().position(|t| t.id == task_id)
            .ok_or_else(|| format!("Task {} not found", task_id))?;

        let task = list.remove(current_pos);
        let target_idx = new_index.min(list.len());
        list.insert(target_idx, task);

        {
            let mut map = self.tasks.write();
            for (idx, t) in list.iter().enumerate() {
                if let Some(stored) = map.get_mut(&t.id) {
                    stored.order_index = idx as i64;
                    stored.updated_at = Utc::now();
                }
            }
        }
        self.save_to_disk();
        Ok(())
    }

    pub fn move_task_up(&self, task_id: &str) -> Result<(), String> {
        let list = self.get_all_tasks();
        let current_pos = list.iter().position(|t| t.id == task_id)
            .ok_or_else(|| format!("Task {} not found", task_id))?;

        if current_pos > 0 {
            self.reorder_task(task_id, current_pos - 1)
        } else {
            Ok(())
        }
    }

    pub fn move_task_down(&self, task_id: &str) -> Result<(), String> {
        let list = self.get_all_tasks();
        let current_pos = list.iter().position(|t| t.id == task_id)
            .ok_or_else(|| format!("Task {} not found", task_id))?;

        if current_pos + 1 < list.len() {
            self.reorder_task(task_id, current_pos + 1)
        } else {
            Ok(())
        }
    }

    pub fn update_task_media_info(&self, id: &str, media_info: MediaStreamInfo) {
        {
            let mut map = self.tasks.write();
            if let Some(task) = map.get_mut(id) {
                task.media_info = Some(media_info);
                task.updated_at = Utc::now();
            }
        }
        self.save_to_disk();
    }

    pub fn update_task_status(&self, id: &str, status: DownloadStatus, error: Option<String>) {
        {
            let mut map = self.tasks.write();
            if let Some(task) = map.get_mut(id) {
                if !matches!(status, DownloadStatus::Downloading | DownloadStatus::Connecting) {
                    task.speed = 0;
                    if status == DownloadStatus::Completed {
                        task.eta_seconds = Some(0);
                    } else {
                        task.eta_seconds = None;
                    }
                }
                task.status = status;
                task.error_message = error;
                task.updated_at = Utc::now();
            }
        }
        self.save_to_disk();
    }

    pub fn update_progress(
        &self,
        id: &str,
        downloaded: u64,
        speed: u64,
        eta: Option<u64>,
        segments: Vec<SegmentProgress>,
    ) {
        let mut map = self.tasks.write();
        if let Some(task) = map.get_mut(id) {
            task.downloaded_bytes = downloaded;
            task.speed = speed;
            task.eta_seconds = eta;
            task.segments = segments;
            task.updated_at = Utc::now();
        }
    }

    pub fn update_total_and_connections(&self, id: &str, total_size: u64, connections: usize) {
        let mut map = self.tasks.write();
        if let Some(task) = map.get_mut(id) {
            if total_size > 0 {
                task.total_size = total_size;
            }
            if connections > 0 {
                task.connections = connections;
            }
            task.updated_at = Utc::now();
        }
    }

    pub fn delete_task(&self, id: &str, delete_file: bool) -> Option<DownloadTask> {
        let task = {
            let mut map = self.tasks.write();
            map.remove(id)
        };

        if let Some(ref t) = task {
            if delete_file {
                let path = Path::new(&t.save_path);
                if path.exists() {
                    let _ = fs::remove_file(path);
                }
            }
        }

        self.save_to_disk();
        task
    }

    pub fn import_tasks(&self, incoming: Vec<DownloadTask>) -> usize {
        let mut count = 0;
        {
            let mut map = self.tasks.write();
            for mut task in incoming {
                if task.status == DownloadStatus::Downloading || task.status == DownloadStatus::Connecting {
                    task.status = DownloadStatus::Paused;
                }
                map.insert(task.id.clone(), task);
                count += 1;
            }
        }
        self.save_to_disk();
        count
    }
}
