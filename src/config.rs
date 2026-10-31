//! Operator configuration, not model-supplied connection information.
use crate::domain::{valid_irc_target, valid_key, Failure, Result};
use serde::Deserialize;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::{
    collections::BTreeSet,
    net::SocketAddr,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Conversation {
    pub key: String,
    pub target: String,
    pub description: String,
    #[serde(default)]
    pub writable: bool,
}
impl Conversation {
    pub fn uri(&self) -> String {
        format!("irc://configured/conversations/{}/messages", self.key)
    }
    pub fn buffer(&self, server: &str) -> String {
        format!("irc.{server}.{}", self.target)
    }
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub bind: SocketAddr,
    pub public_base_url: String,
    pub issuer: String,
    pub subject: String,
    pub allowed_client_ids: Vec<String>,
    pub allowed_origins: Vec<String>,
    pub relay_socket: PathBuf,
    pub relay_password_file: PathBuf,
    pub state_dir: PathBuf,
    pub server: String,
    pub display_identity: String,
    pub relay_api_versions: Vec<String>,
    pub conversations: Vec<Conversation>,
    #[serde(default)]
    pub attention: crate::attention::AttentionConfig,
}
impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|_| Failure::new("configuration", "Unable to read configuration."))?;
        if content.len() > 64 * 1024 {
            return Err(Failure::new(
                "configuration",
                "Configuration exceeds size limit.",
            ));
        }
        let cfg: Self = toml::from_str(&content)
            .map_err(|_| Failure::new("configuration", "Invalid configuration shape."))?;
        cfg.validate()?;
        Ok(cfg)
    }
    pub fn validate(&self) -> Result<()> {
        let invalid = || Failure::new("configuration", "Invalid or unsafe hosted configuration.");
        if !self.bind.ip().is_loopback()
            || !valid_key(&self.server)
            || self.subject.trim().is_empty()
            || self.subject.len() > 2048
            || self.display_identity.is_empty()
            || self.allowed_client_ids.is_empty()
            || self.allowed_client_ids.iter().any(|s| s.is_empty())
            || !self.relay_socket.is_absolute()
            || !self.relay_password_file.is_absolute()
            || !self.state_dir.is_absolute()
            || self.relay_api_versions.is_empty()
            || self.conversations.is_empty()
            || self.conversations.len() > 32
        {
            return Err(invalid());
        }
        let base = url::Url::parse(&self.public_base_url).map_err(|_| invalid())?;
        let issuer = url::Url::parse(&self.issuer).map_err(|_| invalid())?;
        for u in [&base, &issuer] {
            if u.scheme() != "https"
                || u.host_str().is_none()
                || !u.username().is_empty()
                || u.password().is_some()
                || u.query().is_some()
                || u.fragment().is_some()
            {
                return Err(invalid());
            }
        }
        if base.path() != "/" || self.public_base_url.ends_with('/') {
            return Err(invalid());
        }
        for origin in &self.allowed_origins {
            let u = url::Url::parse(origin).map_err(|_| invalid())?;
            if u.scheme() != "https" || u.origin().ascii_serialization() != *origin {
                return Err(invalid());
            }
        }
        let mut keys = BTreeSet::new();
        let mut targets = BTreeSet::new();
        for c in &self.conversations {
            if !valid_key(&c.key)
                || !valid_irc_target(&c.target)
                || c.description.len() > 500
                || !keys.insert(&c.key)
                || !targets.insert(c.target.to_ascii_lowercase())
            {
                return Err(invalid());
            }
        }
        self.attention.validate(&self.conversations)?;
        Ok(())
    }
    pub fn resource_url(&self) -> String {
        format!("{}/mcp", self.public_base_url)
    }
    pub fn authority(&self) -> Result<String> {
        let u = url::Url::parse(&self.public_base_url)
            .map_err(|_| Failure::new("configuration", "Invalid public URL."))?;
        Ok(match u.port() {
            Some(p) => format!("{}:{p}", u.host_str().unwrap_or_default()),
            None => u.host_str().unwrap_or_default().to_owned(),
        })
    }
    pub fn conversation(&self, key: &str) -> Result<&Conversation> {
        self.conversations
            .iter()
            .find(|c| c.key == key)
            .ok_or_else(Failure::denied)
    }
    pub fn read_password(&self) -> Result<String> {
        let metadata = std::fs::symlink_metadata(&self.relay_password_file)
            .map_err(|_| Failure::new("secret_file", "Cannot inspect Relay secret file."))?;
        if !metadata.is_file()
            || metadata.permissions().mode() & 0o077 != 0
            || metadata.len() > 4096
        {
            return Err(Failure::new(
                "secret_file",
                "Relay secret must be a regular, owner-only file of bounded size.",
            ));
        }
        let raw = std::fs::read_to_string(&self.relay_password_file)
            .map_err(|_| Failure::new("secret_file", "Cannot read Relay secret."))?;
        let secret = raw.trim_end_matches(['\r', '\n']).to_string();
        if secret.is_empty() || secret.chars().any(char::is_control) {
            return Err(Failure::new("secret_file", "Invalid Relay secret."));
        }
        Ok(secret)
    }
    /// Existing state directory must already be owned by the service UID with mode 0700.
    pub fn validate_state_dir(&self) -> Result<()> {
        let m = std::fs::symlink_metadata(&self.state_dir).map_err(|_| {
            Failure::new(
                "state_directory",
                "Create an owner-only state directory before startup.",
            )
        })?;
        // The password file and state directory must share their owner; deployment sets this to the service UID.
        let s = std::fs::symlink_metadata(&self.relay_password_file)
            .map_err(|_| Failure::new("state_directory", "Cannot verify state owner."))?;
        if !m.is_dir() || m.permissions().mode() & 0o077 != 0 || m.uid() != s.uid() {
            return Err(Failure::new(
                "state_directory",
                "State directory must be owner-only and share the credential owner.",
            ));
        }
        Ok(())
    }
}
