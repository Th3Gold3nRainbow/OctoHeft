use parking_lot::RwLock;
use reqwest::Proxy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyMode {
    System,
    None,
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    pub mode: ProxyMode,
    pub server_url: Option<String>, // e.g. "http://127.0.0.1:8080" or "socks5://127.0.0.1:1080"
    pub username: Option<String>,
    pub password: Option<String>,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            mode: ProxyMode::System,
            server_url: None,
            username: None,
            password: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainAuth {
    pub domain: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub bearer_token: Option<String>,
    pub custom_headers: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProxyAuthSettings {
    pub proxy: ProxyConfig,
    pub domain_auths: Vec<DomainAuth>,
    pub custom_user_agent: Option<String>,
}

pub struct ProxyAuthManager {
    settings: RwLock<ProxyAuthSettings>,
    storage_path: PathBuf,
}

impl ProxyAuthManager {
    pub fn new(storage_dir: &Path) -> Self {
        let storage_path = storage_dir.join("proxy_auth_settings.json");
        let manager = Self {
            settings: RwLock::new(ProxyAuthSettings::default()),
            storage_path,
        };
        manager.load_from_disk();
        manager
    }

    fn load_from_disk(&self) {
        if !self.storage_path.exists() {
            return;
        }

        if let Ok(data) = fs::read_to_string(&self.storage_path) {
            if let Ok(s) = serde_json::from_str::<ProxyAuthSettings>(&data) {
                *self.settings.write() = s;
            }
        }
    }

    pub fn save_to_disk(&self) {
        if let Some(parent) = self.storage_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let s = self.settings.read().clone();
        if let Ok(json) = serde_json::to_string_pretty(&s) {
            let _ = fs::write(&self.storage_path, json);
        }
    }

    pub fn get_settings(&self) -> ProxyAuthSettings {
        self.settings.read().clone()
    }

    pub fn update_settings(&self, new_settings: ProxyAuthSettings) {
        {
            let mut s = self.settings.write();
            *s = new_settings;
        }
        self.save_to_disk();
    }

    /// Builds a configured `reqwest::Client` based on active proxy settings
    pub fn build_client(&self) -> reqwest::Client {
        let settings = self.settings.read().clone();
        let mut builder = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .pool_max_idle_per_host(32);

        let user_agent = settings.custom_user_agent.as_deref().unwrap_or(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36"
        );
        builder = builder.user_agent(user_agent);

        match settings.proxy.mode {
            ProxyMode::None => {
                builder = builder.no_proxy();
            }
            ProxyMode::System => {
                // Default reqwest behavior respects system / env proxies
            }
            ProxyMode::Manual => {
                if let Some(ref server) = settings.proxy.server_url {
                    if let Ok(mut p) = Proxy::all(server) {
                        if let (Some(u), Some(pw)) = (&settings.proxy.username, &settings.proxy.password) {
                            p = p.basic_auth(u, pw);
                        }
                        builder = builder.proxy(p);
                    }
                }
            }
        }

        builder.build().unwrap_or_default()
    }

    /// Injects domain-specific Basic Auth, Bearer token, or custom headers into an outgoing request
    pub fn apply_auth(&self, mut req: reqwest::RequestBuilder, target_url: &str) -> reqwest::RequestBuilder {
        if let Ok(parsed) = Url::parse(target_url) {
            if let Some(host) = parsed.host_str() {
                let settings = self.settings.read();
                if let Some(auth) = settings.domain_auths.iter().find(|a| a.domain.eq_ignore_ascii_case(host)) {
                    if let (Some(u), Some(p)) = (&auth.username, &auth.password) {
                        req = req.basic_auth(u, Some(p));
                    } else if let Some(token) = &auth.bearer_token {
                        req = req.bearer_auth(token);
                    }

                    for (k, v) in &auth.custom_headers {
                        req = req.header(k, v);
                    }
                }
            }
        }
        req
    }
}

