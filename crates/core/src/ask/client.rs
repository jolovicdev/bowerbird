//! The client that calls a model, and which provider a model name goes to:
//! a cloud provider, Ollama on this device, or the user's own server.

use std::sync::PoisonError;
use std::time::{Duration, Instant};

use genai::ModelIden;
use genai::adapter::AdapterKind;
use genai::chat::{ChatMessage, ChatOptions, ChatRequest, ChatStreamEvent};
use genai::resolver::{AuthData, AuthResolver, Endpoint, ProviderConfig};

use super::SharedSettings;
use super::failure::model_error;
use super::reply::{next_event, silence_limit};
use crate::settings::Thinking;
use crate::{Error, Result};

pub fn client(settings: SharedSettings) -> Result<genai::Client> {
    let thinking = settings
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .thinking;
    client_with_thinking(settings, thinking)
}

/// The client and model for the small calls beside an answer: translating a
/// search, picking Deep search sections, writing web queries. They go to the
/// helper model in Settings, at its own default thinking so a fast model
/// stays fast. Without one they go to the model that answers, so a notebook
/// or question kept on one model sends nothing to another.
pub fn helper(
    settings: &SharedSettings,
    answer: &genai::Client,
    answer_model: &str,
) -> Result<(genai::Client, String)> {
    let helper_model = settings
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .helper_model
        .clone();
    match helper_model {
        Some(model) => Ok((
            client_with_thinking(settings.clone(), Thinking::Default)?,
            model,
        )),
        None => Ok((answer.clone(), answer_model.to_string())),
    }
}

pub fn client_with_thinking(settings: SharedSettings, thinking: Thinking) -> Result<genai::Client> {
    let servers = settings.clone();
    let chat_options = thinking
        .into_effort()
        .map(|effort| ChatOptions::default().with_reasoning_effort(effort));
    let resolver = AuthResolver::from_resolver_fn(move |iden: ModelIden| {
        // Providers without a key variable (local Ollama) need no key.
        let Some(env_name) = iden.adapter_kind.default_key_env_name() else {
            return Ok(None);
        };
        let settings = settings.read().unwrap_or_else(PoisonError::into_inner);
        match settings.key(env_name) {
            Some(key) => Ok(Some(AuthData::from_single(key))),
            None => Err(genai::resolver::Error::ApiKeyEnvNotFound {
                env_name: env_name.to_string(),
            }),
        }
    });
    let http = genai::WebConfig::default()
        .apply_to_builder(crate::http::client())
        // genai's adaptive window starts at 64 KiB, and h2 sizes its allowance
        // for unread small frames from the window: tokens arriving faster than
        // the stream is read would close the connection with ENHANCE_YOUR_CALM.
        .http2_adaptive_window(false)
        .build()
        .map_err(|e| Error::Model(e.to_string()))?;
    let mut builder = genai::Client::builder()
        .with_reqwest(http)
        .with_auth_resolver(resolver)
        .with_service_target_resolver_fn(move |mut target: genai::ServiceTarget| {
            let settings = servers.read().unwrap_or_else(PoisonError::into_inner);
            if target.model.adapter_kind == AdapterKind::Ollama
                && let Some(endpoint) = settings.model_server.as_deref().and_then(server_endpoint)
            {
                // Ollama, LM Studio and llama.cpp all speak OpenAI's protocol,
                // which always sends a key; they take any key.
                let name = target.model.model_name.namespace_and_name().1.to_string();
                target.model = ModelIden::new(AdapterKind::OpenAI, name);
                target.endpoint = Endpoint::from_owned(endpoint);
                target.auth = AuthData::from_single("none");
            }
            Ok(target)
        })
        .with_model_mapper_fn(|iden: ModelIden| {
            Ok(match provider_named(&iden.model_name) {
                Some((kind, name)) => {
                    ModelIden::new(kind, format!("{}::{name}", kind.as_lower_str()))
                }
                None => iden,
            })
        });
    if let Some(options) = chat_options {
        builder = builder.with_chat_options(options);
    }
    builder.build().map_err(|e| Error::Model(e.to_string()))
}

/// Resolves a provider display name or slug (e.g. `"OpenRouter"`, `"open_router"`,
/// `"Ollama"`, `"server"`) to its `AdapterKind`.
pub fn resolve_provider(provider: &str) -> Option<AdapterKind> {
    let letters = |text: &str| -> String {
        text.chars()
            .filter(char::is_ascii_alphanumeric)
            .map(|c| c.to_ascii_lowercase())
            .collect()
    };
    let needle = letters(provider);
    if needle == "server" || needle == "ownserver" || needle == "yourownserver" {
        return Some(AdapterKind::Ollama);
    }
    AdapterKind::all()
        .iter()
        .copied()
        .find(|kind| letters(kind.as_lower_str()) == needle || letters(kind.as_str()) == needle)
}

/// Formats a model name returned by `kind` so typing or clicking it in Settings
/// always routes back to `kind` without accidental provider misrouting.
pub fn format_listed_model(kind: AdapterKind, raw_name: &str) -> String {
    let raw_name = raw_name.trim();
    if raw_name.contains("::") {
        return raw_name.to_string();
    }
    let routed = provider_name(raw_name);
    if routed == kind.as_str()
        || (kind == AdapterKind::OpenAI && routed == AdapterKind::OpenAIResp.as_str())
    {
        raw_name.to_string()
    } else {
        format!("{}::{raw_name}", kind.as_lower_str())
    }
}

/// Where OpenRouter's API is, for the one call made to it without a key.
const OPEN_ROUTER_API: &str = "https://openrouter.ai/api/v1/";

/// Lists models available from a provider (or the user's own server when
/// `provider` is `"Ollama"` / `"server"` and `model_server` is configured).
/// Returns `(display_name, formatted_model_ids)`.
pub async fn list_models(
    settings: SharedSettings,
    provider: &str,
) -> Result<(String, Vec<String>)> {
    let kind = resolve_provider(provider)
        .ok_or_else(|| Error::Invalid(format!("Unknown model provider “{provider}”.")))?;
    let (model_server, has_key) = {
        let s = settings.read().unwrap_or_else(PoisonError::into_inner);
        let has_key = kind
            .default_key_env_name()
            .is_none_or(|env| s.key(env).is_some());
        (s.model_server.clone(), has_key)
    };
    let client = client_with_thinking(settings, Thinking::Default)?;
    let (query_kind, provider_config, display_name, server_label) = if kind == AdapterKind::Ollama
        && let Some(endpoint) = model_server.as_deref().and_then(server_endpoint)
    {
        let host = reqwest::Url::parse(&endpoint)
            .ok()
            .and_then(|u| {
                let host = u.host_str()?.to_string();
                Some(match u.port() {
                    Some(port) => format!("{host}:{port}"),
                    None => host,
                })
            })
            .unwrap_or_else(|| "Your own server".into());
        (
            AdapterKind::OpenAI,
            ProviderConfig {
                endpoint: Some(Endpoint::from_owned(endpoint)),
                auth: Some(AuthData::from_single("none")),
            },
            host.clone(),
            Some(host),
        )
    } else if kind == AdapterKind::OpenRouter && !has_key {
        // OpenRouter's model list is public, so it can be browsed before a
        // key is saved. The address is given along with the empty key: with
        // either left out, the client asks for the saved key to fill the gap
        // and fails without one.
        (
            kind,
            ProviderConfig {
                endpoint: Some(Endpoint::from_static(OPEN_ROUTER_API)),
                auth: Some(AuthData::None),
            },
            kind.as_str().to_string(),
            None,
        )
    } else {
        (
            kind,
            ProviderConfig::default(),
            kind.as_str().to_string(),
            None,
        )
    };

    let fetch = tokio::time::timeout(
        Duration::from_secs(15),
        client.all_model_names(query_kind, provider_config),
    )
    .await
    .map_err(|_| {
        Error::Model(format!(
            "{display_name} took too long to return its model list."
        ))
    })?;

    let raw = fetch.map_err(|e| {
        model_error(
            &format!("{}::", kind.as_lower_str()),
            server_label.as_deref(),
            e,
        )
    })?;

    let mut models: Vec<String> = raw
        .into_iter()
        .map(|name| format_listed_model(kind, &name))
        .filter(|name| !name.is_empty())
        .collect();
    models.sort();
    models.dedup();
    Ok((display_name, models))
}

/// The provider a model name with a colon asks for: `provider::name`,
/// `provider:name`, or Ollama for its own `name:tag`. genai reads only its own
/// spelling, `open_router::name`, and picks by how a name starts, which would
/// send `glm-5.3-flash:cloud`, which runs through Ollama, to Z.ai. So this also
/// takes a provider as Settings lists it, in any case, and keeps tagged names
/// with Ollama.
fn provider_named(model: &str) -> Option<(AdapterKind, &str)> {
    let two_colons = model.split_once("::");
    let (front, name) = two_colons.or_else(|| model.split_once(':'))?;
    let letters = |text: &str| -> String {
        text.chars()
            .filter(char::is_ascii_alphanumeric)
            .map(|c| c.to_ascii_lowercase())
            .collect()
    };
    let front = letters(front);
    let kind = AdapterKind::all()
        .iter()
        .find(|kind| letters(kind.as_lower_str()) == front);
    match kind {
        // Of the providers, only `mistral` is a model in Ollama's library; a
        // local model named after another provider needs `ollama::` in front.
        Some(AdapterKind::Mistral) if two_colons.is_none() => Some((AdapterKind::Ollama, model)),
        Some(kind) => Some((*kind, name.trim())),
        None if two_colons.is_none() => Some((AdapterKind::Ollama, model)),
        None => None,
    }
}

/// The base of the OpenAI-compatible API at a server address. Ollama, LM
/// Studio and llama.cpp all serve it under `/v1/`, so an address without a
/// path gets that; one typed without a scheme gets `http://`. None for an
/// address that can't be a web server.
pub fn server_endpoint(address: &str) -> Option<String> {
    let address = address.trim();
    let address = if address.contains("://") {
        address.to_string()
    } else {
        format!("http://{address}")
    };
    let mut url = reqwest::Url::parse(&address).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    let path = match url.path() {
        "/" => "/v1/".to_string(),
        path if path.ends_with('/') => path.to_string(),
        // A base without its last slash would lose its last segment when
        // `chat/completions` is joined on.
        path => format!("{path}/"),
    };
    url.set_path(&path);
    Some(url.to_string())
}

/// The address of the user's own server, as messages name it, when Settings
/// sends `model` there.
pub(super) async fn own_server(client: &genai::Client, model: &str) -> Option<String> {
    if provider_name(model) != AdapterKind::Ollama.as_str() {
        return None;
    }
    let target = client.resolve_service_target(model).await.ok()?;
    if target.model.adapter_kind == AdapterKind::Ollama {
        return None;
    }
    let url = reqwest::Url::parse(target.endpoint.base_url()).ok()?;
    Some(match url.port() {
        Some(port) => format!("{}:{port}", url.host_str()?),
        None => url.host_str()?.to_string(),
    })
}

/// The provider's display name for a model id, e.g. "DeepSeek".
pub fn provider_name(model: &str) -> &'static str {
    provider_named(model)
        .map(|(kind, _)| kind)
        .or_else(|| AdapterKind::from_model(model).ok())
        .map_or("the model provider", |k| k.as_str())
}

/// Sends one short message the way an answer goes out, to find out whether the
/// model name, the key and the account behind it all work. A provider's model
/// list can't tell, since several hand it to any key. Costs a few tokens.
/// Gives who answered: the provider, or the address of the user's own server.
pub async fn check_model(client: &genai::Client, model: &str) -> Result<String> {
    let server = own_server(client, model).await;
    let answerer = server.as_deref().unwrap_or(provider_name(model));
    match reply_starts(client, model, silence_limit(model)).await {
        Ok(true) => Ok(answerer.to_string()),
        Ok(false) => Err(Error::Model(format!("{answerer} sent no reply."))),
        Err(e) => Err(model_error(model, server.as_deref(), e)),
    }
}

/// A provider silent for `limit` counts as sending no reply.
pub(super) async fn reply_starts(
    client: &genai::Client,
    model: impl Into<genai::ModelSpec>,
    limit: Duration,
) -> genai::Result<bool> {
    let request = ChatRequest::new(vec![ChatMessage::user("Reply with the word OK.")]);
    let mut stream = client.exec_chat_stream(model, request, None).await?.stream;
    let started = Instant::now();
    loop {
        let Ok(Some(event)) = next_event(&mut stream, limit, started).await else {
            return Ok(false);
        };
        if matches!(
            event?,
            ChatStreamEvent::Chunk(_) | ChatStreamEvent::ReasoningChunk(_)
        ) {
            return Ok(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, RwLock};

    use super::*;
    use crate::ask::reply::SILENCE;
    use crate::ask::testing::*;
    use crate::settings::Settings;

    #[tokio::test]
    async fn a_model_name_reaches_the_provider_it_asks_for() {
        use AdapterKind::{BedrockApi, DeepSeek, Mistral, Ollama, OpenRouter, Zai};
        // Each key's value is its own variable, to show which key a provider gets.
        let keys = crate::settings::providers()
            .into_iter()
            .map(|(_, env)| (env.to_string(), env.to_string()))
            .collect();
        let settings = Settings {
            keys,
            ..Default::default()
        };
        let client = client(Arc::new(RwLock::new(settings))).unwrap();
        for (typed, kind, model) in [
            ("Openrouter:inkling", OpenRouter, "inkling"),
            ("Openrouter::inkling", OpenRouter, "inkling"),
            ("openrouter::inkling", OpenRouter, "inkling"),
            ("Open Router: inkling", OpenRouter, "inkling"),
            ("open_router:inkling", OpenRouter, "inkling"),
            ("open_router::inkling", OpenRouter, "inkling"),
            ("OpenRouter:org/model:free", OpenRouter, "org/model:free"),
            ("Deepseek:deepseek-flash", DeepSeek, "deepseek-flash"),
            ("Mistral::large", Mistral, "large"),
            // A model and its tag, in Ollama's naming.
            ("llama3:8b", Ollama, "llama3:8b"),
            ("mistral:7b", Ollama, "mistral:7b"),
            ("glm-5.3-flash:cloud", Ollama, "glm-5.3-flash:cloud"),
            ("command-r:35b", Ollama, "command-r:35b"),
            ("Mistral:7b", Ollama, "Mistral:7b"),
            ("hf.co/user/model:Q4", Ollama, "hf.co/user/model:Q4"),
            ("Ollama:llama3:8b", Ollama, "llama3:8b"),
            ("ollama::llama3:8b", Ollama, "llama3:8b"),
            // Names genai routes by itself.
            ("deepseek-flash", DeepSeek, "deepseek-flash"),
            ("mistral::large", Mistral, "large"),
            ("bedrock_api::claude-v1:0", BedrockApi, "claude-v1:0"),
            ("zai_coding::glm-4.6", Zai, "glm-4.6"),
        ] {
            let target = client.resolve_service_target(typed).await.unwrap();
            assert_eq!(target.model.adapter_kind, kind, "{typed}");
            assert_eq!(
                target.model.model_name.namespace_and_name().1,
                model,
                "{typed}"
            );
            if let Some(env) = kind.default_key_env_name() {
                assert_eq!(target.auth.single_key_value().unwrap(), env, "{typed}");
            }
            assert_eq!(provider_name(typed), kind.as_str(), "{typed}");
        }
    }

    #[test]
    fn every_provider_can_go_in_front_as_settings_lists_it() {
        for (name, _) in crate::settings::providers() {
            assert_eq!(provider_name(&format!("{name}::some-model")), name);
            if name != AdapterKind::Mistral.as_str() {
                assert_eq!(provider_name(&format!("{name}:some-model")), name);
            }
        }
    }

    #[tokio::test]
    async fn a_thinking_setting_is_sent_with_the_request() {
        use crate::settings::Thinking;
        let ok = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"OK\"}}]}\n\ndata: [DONE]\n\n";
        for (thinking, expected) in [
            (Thinking::Default, None),
            (Thinking::Off, Some("\"reasoning_effort\":\"none\"")),
            (Thinking::Low, Some("\"reasoning_effort\":\"low\"")),
        ] {
            let (address, request) = recording_server(ok).await;
            let settings = Settings {
                model_server: Some(address),
                thinking,
                ..Default::default()
            };
            let client = client(Arc::new(RwLock::new(settings))).unwrap();
            check_model(&client, "qwen/qwen3-8b").await.unwrap();
            let request = request.await.unwrap();
            match expected {
                Some(needle) => assert!(request.contains(needle), "{thinking:?}: {request}"),
                None => assert!(
                    !request.contains("reasoning_effort"),
                    "{thinking:?}: {request}"
                ),
            }
        }
    }

    #[test]
    fn a_server_address_becomes_the_base_of_its_openai_api() {
        for (typed, base) in [
            (
                "http://192.168.1.20:11434",
                Some("http://192.168.1.20:11434/v1/"),
            ),
            ("192.168.1.20:1234", Some("http://192.168.1.20:1234/v1/")),
            ("localhost:8080", Some("http://localhost:8080/v1/")),
            (
                "http://localhost:8080/v1",
                Some("http://localhost:8080/v1/"),
            ),
            (
                " https://models.example.org/api/v1/ ",
                Some("https://models.example.org/api/v1/"),
            ),
            ("ftp://192.168.1.20", None),
            ("http://", None),
            ("", None),
        ] {
            assert_eq!(server_endpoint(typed).as_deref(), base, "{typed}");
        }
    }

    #[test]
    fn helper_calls_go_to_the_answering_model_unless_settings_name_a_helper() {
        let settings = Arc::new(RwLock::new(Settings::default()));
        let answer = client(settings.clone()).unwrap();
        let (_, model) = helper(&settings, &answer, "ollama::qwen3").unwrap();
        assert_eq!(model, "ollama::qwen3");
        settings.write().unwrap().helper_model = Some("deepseek-flash".into());
        let (_, model) = helper(&settings, &answer, "ollama::qwen3").unwrap();
        assert_eq!(model, "deepseek-flash");
    }

    #[tokio::test]
    async fn models_that_would_go_to_ollama_go_to_the_server_in_settings() {
        let ok = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"OK\"}}]}\n\ndata: [DONE]\n\n";
        let (address, request) = recording_server(ok).await;
        let client = client_with_server(&address);
        assert_eq!(
            check_model(&client, "qwen/qwen3-8b").await.unwrap(),
            address.trim_start_matches("http://")
        );
        let request = request.await.unwrap();
        assert!(
            request.starts_with("POST /v1/chat/completions "),
            "{request}"
        );
        for (typed, name) in [
            ("qwen/qwen3-8b", "qwen/qwen3-8b"),
            ("llama3:8b", "llama3:8b"),
            ("ollama::llama3:8b", "llama3:8b"),
        ] {
            let target = client.resolve_service_target(typed).await.unwrap();
            assert_eq!(target.model.adapter_kind, AdapterKind::OpenAI, "{typed}");
            assert_eq!(target.model.model_name.namespace_and_name().1, name);
            assert_eq!(target.endpoint.base_url(), format!("{address}/v1/"));
        }
        let target = client
            .resolve_service_target("deepseek-flash")
            .await
            .unwrap();
        assert_eq!(target.model.adapter_kind, AdapterKind::DeepSeek);
    }

    #[tokio::test]
    async fn a_server_in_settings_that_doesnt_answer_is_named() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let host = listener.local_addr().unwrap().to_string();
        drop(listener);
        let client = client_with_server(&host);
        assert_eq!(
            check_model(&client, "llama3:8b")
                .await
                .unwrap_err()
                .to_string(),
            format!(
                "Nothing answers at {host}. Check the server address in Settings, and that the server is running."
            )
        );
    }

    #[tokio::test]
    async fn a_model_test_passes_only_when_a_reply_starts() {
        let client = client(Arc::default()).unwrap();
        let reply = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"OK\"}}]}\n\ndata: [DONE]\n\n";
        let target = provider(AdapterKind::OpenRouter, Some(("200 OK", reply))).await;
        assert!(reply_starts(&client, target, SILENCE).await.unwrap());
        let empty = "data: [DONE]\n\n";
        let target = provider(AdapterKind::OpenRouter, Some(("200 OK", empty))).await;
        assert!(!reply_starts(&client, target, SILENCE).await.unwrap());
        let refused = r#"{"error":{"message":"Insufficient credits","code":402}}"#;
        let target = provider(
            AdapterKind::OpenRouter,
            Some(("402 Payment Required", refused)),
        )
        .await;
        let failure = reply_starts(&client, target, SILENCE).await.unwrap_err();
        assert_eq!(
            model_error("open_router::x", None, failure).to_string(),
            "OpenRouter refused the request (402): Insufficient credits"
        );
    }

    #[test]
    fn listed_models_are_prefixed_whenever_bare_names_would_misroute() {
        assert_eq!(
            format_listed_model(AdapterKind::OpenRouter, "org/model:free"),
            "open_router::org/model:free"
        );
        assert_eq!(format_listed_model(AdapterKind::OpenAI, "gpt-5"), "gpt-5");
        assert_eq!(
            format_listed_model(AdapterKind::Groq, "fast-model"),
            "groq::fast-model"
        );
        assert_eq!(
            format_listed_model(AdapterKind::Ollama, "gpt-5"),
            "ollama::gpt-5"
        );
        assert_eq!(
            format_listed_model(AdapterKind::Ollama, "local-model:8b"),
            "local-model:8b"
        );
        assert_eq!(
            resolve_provider("OpenRouter"),
            Some(AdapterKind::OpenRouter)
        );
        assert_eq!(
            resolve_provider("open_router"),
            Some(AdapterKind::OpenRouter)
        );
        assert_eq!(resolve_provider("server"), Some(AdapterKind::Ollama));
    }

    #[tokio::test]
    async fn list_models_queries_own_server_openai_models_endpoint() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 2048];
            let _ = socket.read(&mut buf).await;
            let body = r#"{"data":[{"id":"local-model:7b"},{"id":"gpt-5"}]}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(resp.as_bytes()).await;
        });
        let settings = Arc::new(RwLock::new(Settings {
            model_server: Some(format!("http://{addr}")),
            ..Default::default()
        }));
        let (name, models) = list_models(settings, "server").await.unwrap();
        assert_eq!(name, addr.to_string());
        assert_eq!(models, ["local-model:7b", "ollama::gpt-5"]);
    }
}
