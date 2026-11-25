//! Turns a failed model call into a sentence that says what to do about it.

use genai::adapter::AdapterKind;
use genai::chat::StopReason;

use super::client::provider_name;
use crate::{Error, retry};

/// Turns a model-call failure into a sentence that says what went wrong and
/// where to fix it.
fn status(error: &genai::Error) -> Option<u16> {
    use genai::Error as G;
    match error {
        G::WebModelCall { webc_error, .. } | G::WebAdapterCall { webc_error, .. } => {
            webc_error.status().map(|s| s.as_u16())
        }
        G::HttpError { status, .. } => Some(status.as_u16()),
        G::ChatResponse { body, .. } => body["code"].as_u64().and_then(|c| u16::try_from(c).ok()),
        G::WebStream { error, .. } => error.downcast_ref::<genai::Error>().and_then(status),
        _ => None,
    }
}

/// A provider's own reason for refusing a request. A status alone can mislead:
/// OpenRouter answers 403 to a moderation flag or a spent budget, and some
/// providers answer 400 to a model name they don't know.
fn refusal_reason(error: &genai::Error) -> Option<String> {
    use genai::Error as G;
    use genai::webc::Error::ResponseFailedStatus as Failed;
    let body = match error {
        G::ChatResponse { body, .. } => {
            return body["message"]
                .as_str()
                .or_else(|| body["error"]["message"].as_str())
                .map(str::to_string);
        }
        G::HttpError { body, .. }
        | G::WebModelCall {
            webc_error: Failed { body, .. },
            ..
        }
        | G::WebAdapterCall {
            webc_error: Failed { body, .. },
            ..
        } => body,
        G::WebStream { error, .. } => return refusal_reason(error.downcast_ref()?),
        _ => return None,
    };
    let reply: serde_json::Value = serde_json::from_str(body).ok()?;
    reply["error"]["message"].as_str().map(str::to_string)
}

/// A model call that failed on the way, which trying again can fix.
pub(super) fn transient(error: &genai::Error) -> bool {
    use genai::Error as G;
    matches!(
        error,
        G::WebModelCall { .. } | G::WebAdapterCall { .. } | G::HttpError { .. }
    ) && retry::transient_status(status(error))
}

/// `server` is the user's own server when the model went there.
pub(crate) fn model_error(model: &str, server: Option<&str>, error: genai::Error) -> Error {
    use genai::Error as G;
    let provider = server.unwrap_or(provider_name(model));
    let status = status(&error);
    let reason = refusal_reason(&error);
    let message = match (&error, status) {
        (G::Resolver { .. } | G::RequiresApiKey { .. } | G::NoAuthData { .. }, _) => {
            format!("No API key for {provider}. Add one in Settings.")
        }
        (_, Some(401)) => format!("{provider} rejected the API key. Check it in Settings."),
        (_, Some(404)) if reason.is_none() => {
            format!("{provider} has no model called “{model}”. Check the model name in Settings.")
        }
        (_, Some(429)) => {
            format!("{provider} is limiting requests right now. Wait a minute, then try again.")
        }
        (_, Some(code)) if code >= 500 => {
            format!("{provider} had a problem ({code}). Try again in a moment.")
        }
        (_, Some(code)) => match reason {
            Some(reason) => format!("{provider} refused the request ({code}): {reason}"),
            None => format!("{provider} refused the request ({code})."),
        },
        (G::WebModelCall { .. } | G::WebAdapterCall { .. } | G::WebStream { .. }, None) => {
            if server.is_some() {
                format!(
                    "Nothing answers at {provider}. Check the server address in Settings, and that the server is running."
                )
            } else if provider == AdapterKind::Ollama.as_str() {
                format!(
                    "Ollama isn't answering on this device. “{model}” goes to Ollama because no other provider is named in front of it. To use another provider, put its name in front in Settings, as in groq::model-name. For a server on another computer, add its address in Settings."
                )
            } else {
                format!("Bowerbird couldn't reach {provider}. Check your internet connection.")
            }
        }
        (G::ChatResponse { .. }, None) => match reason {
            Some(reason) => format!("{provider} returned an error: {reason}"),
            None => format!("The request to {provider} failed: {error}"),
        },
        _ => format!("The request to {provider} failed: {error}"),
    };
    Error::Model(message)
}

pub(super) fn empty_reply_error(provider: &str, stop: Option<StopReason>) -> Error {
    let message = match stop {
        Some(StopReason::ContentFilter(_)) => format!(
            "{provider} ended the answer before writing any of it: a content filter stopped it."
        ),
        Some(StopReason::MaxTokens(_)) => format!(
            "{provider} ended the answer before writing any of it: the model hit its length limit."
        ),
        _ => format!("{provider} sent an empty answer."),
    };
    Error::Model(format!(
        "{message} Try again, or choose another model in Settings."
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures::StreamExt;

    use super::*;
    use crate::ask::client::client;
    use crate::ask::testing::*;

    /// The failure of a streamed request, however far it got.
    async fn stream_failure(target: genai::ServiceTarget) -> genai::Error {
        let client = client(Arc::default()).unwrap();
        let mut stream = match client.exec_chat_stream(target, question(), None).await {
            Ok(response) => response.stream,
            Err(error) => return error,
        };
        while let Some(event) = stream.next().await {
            if let Err(error) = event {
                return error;
            }
        }
        panic!("the request should have failed");
    }

    #[tokio::test]
    async fn a_refused_stream_says_what_the_provider_refused() {
        for (status, body, message) in [
            (
                "401 Unauthorized",
                r#"{"error":{"message":"User not found.","code":401}}"#,
                "OpenRouter rejected the API key. Check it in Settings.",
            ),
            (
                "402 Payment Required",
                r#"{"error":{"message":"Insufficient credits","code":402}}"#,
                "OpenRouter refused the request (402): Insufficient credits",
            ),
            (
                "403 Forbidden",
                r#"{"error":{"message":"Your input was flagged","code":403}}"#,
                "OpenRouter refused the request (403): Your input was flagged",
            ),
            (
                "403 Forbidden",
                "<html>Blocked</html>",
                "OpenRouter refused the request (403).",
            ),
            (
                "404 Not Found",
                r#"{"error":{"message":"No endpoints found.","code":404}}"#,
                "OpenRouter refused the request (404): No endpoints found.",
            ),
            (
                "404 Not Found",
                r#"{"error":"model not found"}"#,
                "OpenRouter has no model called “open_router::x”. Check the model name in Settings.",
            ),
            (
                "400 Bad Request",
                r#"{"error":{"message":"x is not a valid model ID","code":400}}"#,
                "OpenRouter refused the request (400): x is not a valid model ID",
            ),
            (
                "400 Bad Request",
                "<html>Bad Request</html>",
                "OpenRouter refused the request (400).",
            ),
        ] {
            let target = provider(AdapterKind::OpenRouter, Some((status, body))).await;
            let error = model_error("open_router::x", None, stream_failure(target).await);
            assert_eq!(error.to_string(), message);
        }
    }

    #[tokio::test]
    async fn a_refused_request_without_a_stream_gives_the_same_reason() {
        let body = r#"{"error":{"message":"x is not a valid model ID","code":400}}"#;
        let target = provider(AdapterKind::OpenRouter, Some(("400 Bad Request", body))).await;
        let client = client(Arc::default()).unwrap();
        let failure = client
            .exec_chat(target, question(), None)
            .await
            .unwrap_err();
        assert_eq!(
            model_error("open_router::x", None, failure).to_string(),
            "OpenRouter refused the request (400): x is not a valid model ID"
        );
    }

    #[tokio::test]
    async fn an_unanswered_request_says_where_to_look() {
        let target = provider(AdapterKind::OpenRouter, None).await;
        assert_eq!(
            model_error("open_router::x", None, stream_failure(target).await).to_string(),
            "Bowerbird couldn't reach OpenRouter. Check your internet connection."
        );
        let target = provider(AdapterKind::Ollama, None).await;
        assert_eq!(
            model_error("vendor/model", None, stream_failure(target).await).to_string(),
            "Ollama isn't answering on this device. “vendor/model” goes to Ollama because no other provider is named in front of it. To use another provider, put its name in front in Settings, as in groq::model-name. For a server on another computer, add its address in Settings."
        );
    }
}
