use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Runtime spec (YAML file)
#[derive(Debug, Deserialize, Serialize)]
pub struct RuntimeSpec {
    pub metadata: Metadata,
    pub spec: Spec,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Metadata {
    pub name: String,
    pub region: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Spec {
    pub image: String,
    pub role: String,
    #[serde(default = "default_network")]
    pub network: String,
    pub filesystem: Option<FilesystemConfig>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FilesystemConfig {
    #[serde(rename = "sessionStorage")]
    pub session_storage: Option<String>,
}

fn default_network() -> String {
    "PUBLIC".into()
}

impl RuntimeSpec {
    pub fn from_file(path: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {path}"))?;
        serde_yaml::from_str(&content)
            .with_context(|| format!("failed to parse {path}"))
    }

    pub fn region(&self) -> String {
        self.metadata.region.clone().unwrap_or_else(|| "us-east-1".into())
    }
}

/// Alias store (~/.config/agctl/aliases.json)
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AliasStore {
    pub aliases: HashMap<String, String>,
}

impl AliasStore {
    fn path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("agctl")
            .join("aliases.json")
    }

    pub fn load() -> Self {
        let path = Self::path();
        if path.exists() {
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            Self::default()
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    pub fn resolve(&self, name: &str) -> String {
        if name.starts_with("arn:") {
            name.to_string()
        } else {
            self.aliases.get(name).cloned().unwrap_or_else(|| name.to_string())
        }
    }
}
