//! User settings: the chat model and provider API keys, stored as TOML in the
//! config folder. The file holds keys, so it is written readable by the user only.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::notebook::ChatStyle;
use crate::ocr::OcrScript;
use crate::search::SearchSettings;
use crate::{Error, Result};

/// Overrides where settings live, e.g. for tests.
pub const CONFIG_ENV: &str = "BOWERBIRD_CONFIG";

/// How much reasoning to ask a thinking model for. `Default` sends nothing,
/// leaving the model's own default alone and staying compatible with models
/// that reject reasoning options.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Thinking {
    #[default]
    Default,
    Off,
    Low,
    Medium,
    High,
}

impl Thinking {
    pub fn is_default(&self) -> bool {
        matches!(self, Thinking::Default)
    }

    pub fn into_effort(self) -> Option<genai::chat::ReasoningEffort> {
        use genai::chat::ReasoningEffort;
        match self {
            Thinking::Default => None,
            Thinking::Off => Some(ReasoningEffort::Zero),
            Thinking::Low => Some(ReasoningEffort::Low),
            Thinking::Medium => Some(ReasoningEffort::Medium),
            Thinking::High => Some(ReasoningEffort::High),
        }
    }
}

/// A named model + thinking preset that can be switched in one click.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelProfile {
    pub name: String,
    pub model: String,
    /// Always written, so a reader can tell a profile that asks for the
    /// model's own default from one that says nothing about thinking.
    #[serde(default)]
    pub thinking: Thinking,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    /// Model for answers. Unset means `ask::DEFAULT_MODEL`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_model: Option<String>,
    /// Faster model used for lightweight tasks (translating search queries,
    /// picking Deep search sections, writing web research queries). Unset uses
    /// the model that writes the answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub helper_model: Option<String>,
    /// How much reasoning to ask a thinking model for.
    #[serde(default, skip_serializing_if = "Thinking::is_default")]
    pub thinking: Thinking,
    /// Named model presets for quick switching in Settings and notebooks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<ModelProfile>,
    /// Script recognizer used when reading scanned PDFs with OCR.
    #[serde(default, skip_serializing_if = "OcrScript::is_default")]
    pub ocr_script: OcrScript,
    /// API keys by the environment variable each provider reads, e.g.
    /// `DEEPSEEK_API_KEY`, so saved keys and environment keys line up.
    #[serde(default)]
    pub keys: BTreeMap<String, String>,
    /// How web searches for new sources run.
    #[serde(default)]
    pub search: SearchSettings,
    /// The user's own model server, such as Ollama on another computer, LM
    /// Studio or llama.cpp. It gets the models that would go to Ollama on this
    /// device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_server: Option<String>,
}

/// `$BOWERBIRD_CONFIG`, else `bowerbird/settings.toml` in the user's config folder.
pub fn path() -> PathBuf {
    if let Some(path) = std::env::var_os(CONFIG_ENV) {
        return path.into();
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_default();
    base.join("bowerbird").join("settings.toml")
}

static MODELS_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Puts speech models in `dir`, for apps whose data lives in their own
/// folder. Takes effect only if set before the first model is used.
pub fn use_models_dir(dir: PathBuf) {
    let _ = MODELS_DIR.set(dir);
}

/// `bowerbird/models` in the user's local data folder: downloaded speech
/// models are too large for a config folder that roams between machines.
pub fn models_dir() -> PathBuf {
    if let Some(dir) = MODELS_DIR.get() {
        return dir.clone();
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("share"))
        })
        .unwrap_or_default();
    base.join("bowerbird").join("models")
}

/// Providers that take an API key, as (name, key variable), one row per
/// variable since some providers share one.
pub fn providers() -> Vec<(&'static str, &'static str)> {
    let mut out: Vec<(&'static str, &'static str)> = Vec::new();
    for kind in genai::adapter::AdapterKind::all() {
        if let Some(env) = kind.default_key_env_name()
            && !out.iter().any(|(_, e)| *e == env)
        {
            out.push((kind.as_str(), env));
        }
    }
    out
}

impl Settings {
    pub fn load() -> Result<Settings> {
        Settings::load_from(&path())
    }

    /// A missing file means default settings.
    pub fn load_from(path: &Path) -> Result<Settings> {
        match std::fs::read_to_string(path) {
            Ok(raw) => toml::from_str(&raw).map_err(|e| Error::Toml(e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&path())
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let raw = toml::to_string_pretty(self).map_err(|e| Error::Toml(e.to_string()))?;
        let tmp = path.with_extension("toml.tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        std::io::Write::write_all(&mut options.open(&tmp)?, raw.as_bytes())?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn chat_model(&self) -> &str {
        self.chat_model
            .as_deref()
            .unwrap_or(crate::ask::DEFAULT_MODEL)
    }

    /// The model and thinking level for work in a notebook: the notebook's
    /// own where its answer style names them, else these settings'.
    pub fn for_notebook(&self, style: &ChatStyle) -> (String, Thinking) {
        (
            style
                .model
                .clone()
                .unwrap_or_else(|| self.chat_model().to_string()),
            style.thinking.unwrap_or(self.thinking),
        )
    }

    /// The saved key for a provider, else its environment variable.
    /// OpenRouter also falls back to the common `OPENROUTER_API_KEY` name.
    pub fn key(&self, env_name: &str) -> Option<String> {
        let env_value = |name: &str| std::env::var(name).ok().filter(|k| !k.trim().is_empty());
        self.keys
            .get(env_name)
            .filter(|k| !k.trim().is_empty())
            .cloned()
            .or_else(|| env_value(env_name))
            .or_else(|| {
                (env_name == "OPEN_ROUTER_API_KEY")
                    .then(|| env_value("OPENROUTER_API_KEY"))
                    .flatten()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_settings_load_back_and_stay_private() {
        let path = std::env::temp_dir()
            .join(format!("bowerbird-settings-{}", std::process::id()))
            .join("settings.toml");
        let settings = Settings {
            chat_model: Some("claude-sonnet-5-5".into()),
            thinking: Thinking::Low,
            keys: BTreeMap::from([("ANTHROPIC_API_KEY".into(), "sk-test".into())]),
            ..Default::default()
        };
        settings.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path).unwrap(), settings);
        let default_path = path.with_file_name("default.toml");
        Settings::default().save_to(&default_path).unwrap();
        assert!(
            !std::fs::read_to_string(&default_path)
                .unwrap()
                .contains("thinking")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn saved_keys_win_over_the_environment_and_blank_keys_count_as_missing() {
        let mut settings = Settings::default();
        settings
            .keys
            .insert("BOWERBIRD_TEST_KEY_A".into(), "saved".into());
        settings
            .keys
            .insert("BOWERBIRD_TEST_KEY_B".into(), "  ".into());
        assert_eq!(
            settings.key("BOWERBIRD_TEST_KEY_A").as_deref(),
            Some("saved")
        );
        assert_eq!(settings.key("BOWERBIRD_TEST_KEY_B"), None);
    }
}
