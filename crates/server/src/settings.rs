//! Settings: the chat model, provider keys and web search, with keys never sent back.

use std::collections::BTreeMap;
use std::sync::PoisonError;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use bowerbird_core::ask;
use bowerbird_core::search::{self, SearchSettings};
use bowerbird_core::settings::{self, Settings};
use serde::{Deserialize, Serialize};

use crate::{ApiError, ApiResult, AppState, Body, blocking};

#[derive(Serialize)]
struct ProviderView {
    name: &'static str,
    key_env: &'static str,
    /// A key is saved in Settings or set in the environment.
    has_key: bool,
    /// The key is saved in Settings, so Settings can remove it.
    saved: bool,
}

/// Settings as the UI sees them. Keys never leave the server.
#[derive(Serialize)]
pub(crate) struct SettingsView {
    chat_model: String,
    default_model: &'static str,
    providers: Vec<ProviderView>,
    search: SearchSettings,
    /// Search services that take a key.
    search_keys: Vec<ProviderView>,
}

fn settings_view(settings: &Settings) -> SettingsView {
    let view = |(name, key_env): (&'static str, &'static str)| ProviderView {
        name,
        key_env,
        has_key: settings.key(key_env).is_some(),
        saved: settings
            .keys
            .get(key_env)
            .is_some_and(|k| !k.trim().is_empty()),
    };
    SettingsView {
        chat_model: settings.chat_model().to_string(),
        default_model: ask::DEFAULT_MODEL,
        providers: settings::providers().into_iter().map(view).collect(),
        search: settings.search.clone(),
        search_keys: search::KEYS.into_iter().map(view).collect(),
    }
}

pub(crate) async fn get_settings(State(state): State<AppState>) -> Json<SettingsView> {
    Json(settings_view(&state.settings()))
}

#[derive(Deserialize)]
pub(crate) struct SettingsUpdate {
    /// An empty name means the default model.
    chat_model: Option<String>,
    /// By key variable. An empty key removes the saved one.
    #[serde(default)]
    keys: BTreeMap<String, String>,
    search: Option<SearchSettings>,
}

pub(crate) async fn put_settings(
    State(state): State<AppState>,
    Body(update): Body<SettingsUpdate>,
) -> ApiResult<Json<SettingsView>> {
    let mut known = settings::providers();
    known.extend(search::KEYS);
    if let Some(name) = update
        .keys
        .keys()
        .find(|k| !known.iter().any(|(_, env)| env == k))
    {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            format!("No provider takes a key named {name}."),
        ));
    }
    blocking(move || {
        let mut current = state
            .settings
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let mut next = current.clone();
        if let Some(model) = update.chat_model {
            let model = model.trim();
            next.chat_model = (!model.is_empty()).then(|| model.to_string());
        }
        if let Some(search) = update.search {
            next.search = search;
        }
        for (env, key) in update.keys {
            match key.trim() {
                "" => next.keys.remove(&env),
                key => next.keys.insert(env, key.to_string()),
            };
        }
        next.save_to(&state.settings_path)?;
        *current = next;
        Ok(Json(settings_view(&current)))
    })
    .await
}
