//! The OpenRouter client: one chat completion, and the list of models that
//! can use tools. Every failure becomes a sentence the conversation can show.

use crate::Model;
use serde_json::Value;
use std::time::Duration;

/// How long one model turn may take before it counts as a network failure.
const CHAT_TIMEOUT: Duration = Duration::from_secs(600);
const MODELS_TIMEOUT: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// The largest reply read, so a runaway answer cannot fill memory.
const BODY_LIMIT: u64 = 64 * 1024 * 1024;
/// The most pages of models followed.
const MODEL_PAGES: usize = 50;

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .http_status_as_error(false)
        .build()
        .into()
}

/// `POST {base}/chat/completions`, streamed. A JSON reply is accepted too, so
/// a stand-in may answer either way. Returns the reply's first choice's message.
#[cfg(test)]
pub fn chat(base_url: &str, key: &str, body: &Value) -> Result<Value, String> {
    chat_streamed(base_url, key, body, |_| {})
}

/// As [`chat`], and each piece of the reply's text goes to `text` as soon as
/// its line is read, so the conversation can show the reply arriving.
pub fn chat_streamed(base_url: &str, key: &str, body: &Value, mut text: impl FnMut(&str)) -> Result<Value, String> {
    use std::io::BufRead;
    let mut body = body.clone();
    body["stream"] = Value::Bool(true);
    let mut response = send(base_url, key, &body)
        .map_err(|error| format!("Could not reach the endpoint: {error}."))?;
    if !response.status().is_success() {
        return Err(failure(response.status().as_u16(), &body_text(&mut response)?));
    }
    let reader = response.body_mut().with_config().limit(BODY_LIMIT).reader();
    let mut whole = String::new();
    for line in std::io::BufReader::new(reader).lines() {
        let line = line.map_err(|error| format!("The endpoint's reply could not be read: {error}."))?;
        if let Some(data) = line.trim().strip_prefix("data:") {
            let data = data.trim();
            if !data.is_empty() && data != "[DONE]" {
                for choice in sse_choices(&format!("data: {data}"))? {
                    if let Some(piece) = choice.pointer("/delta/content").and_then(Value::as_str)
                        && !piece.is_empty()
                    {
                        text(piece);
                    }
                }
            }
        }
        whole.push_str(&line);
        whole.push('\n');
    }
    assemble(&whole)
}

/// One user message, no system prompt and no tools, streamed. Each piece of
/// text goes to `piece` as soon as its line is read; when `piece` returns
/// false the stream is dropped, so nothing more is read. A JSON reply, as a
/// stand-in may send, is one piece.
pub fn stream_prompt(
    base_url: &str,
    key: &str,
    model: &str,
    prompt: &str,
    extra: &serde_json::Map<String, Value>,
    mut piece: impl FnMut(&str) -> bool,
) -> Result<(), String> {
    use std::io::BufRead;
    let mut body = serde_json::json!({
        "model": model,
        "stream": true,
        "messages": [{"role": "user", "content": prompt}],
    });
    // A level's effort, in the endpoint's own field.
    for (field, value) in extra {
        body[field] = value.clone();
    }
    let mut response = send(base_url, key, &body)
        .map_err(|error| format!("Could not reach the endpoint: {error}."))?;
    if !response.status().is_success() {
        return Err(failure(response.status().as_u16(), &body_text(&mut response)?));
    }
    let reader = response.body_mut().with_config().limit(BODY_LIMIT).reader();
    let mut lines = std::io::BufReader::new(reader).lines();
    let mut first = String::new();
    while let Some(line) = lines.next() {
        let line = line.map_err(|error| format!("The endpoint's reply could not be read: {error}."))?;
        let Some(data) = line.trim().strip_prefix("data:") else {
            // Not server-sent events: keep it, in case the whole body is JSON.
            first.push_str(&line);
            first.push('\n');
            continue;
        };
        let data = data.trim();
        if data.is_empty() {
            continue;
        }
        if data == "[DONE]" {
            return Ok(());
        }
        for choice in sse_choices(&format!("data: {data}"))? {
            let text = choice
                .pointer("/delta/content")
                .or_else(|| choice.pointer("/message/content"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if !text.is_empty() && !piece(text) {
                return Ok(());
            }
        }
    }
    if !first.trim().is_empty() {
        let reply: Value = serde_json::from_str(first.trim())
            .map_err(|_| String::from("The endpoint's reply was not JSON."))?;
        let full = message(&reply)?;
        let text = full.get("content").and_then(Value::as_str).unwrap_or("");
        if !text.is_empty() {
            piece(text);
        }
    }
    Ok(())
}

fn send(
    base_url: &str,
    key: &str,
    body: &Value,
) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let mut request = agent(CHAT_TIMEOUT).post(&url);
    // A local server, such as Ollama, takes no key.
    if !key.is_empty() {
        request = request.header("Authorization", &format!("Bearer {key}"));
    }
    request
        .header("HTTP-Referer", "https://github.com/Spumcake/Inpainter")
        .header("X-Title", "Inpainter")
        .header("Content-Type", "application/json")
        .send(body.to_string())
}

/// A chat body, JSON or server-sent events, as the assistant's message.
pub fn assemble(text: &str) -> Result<Value, String> {
    if !text.contains("data:") {
        let reply: Value = serde_json::from_str(text.trim())
            .map_err(|_| String::from("The endpoint's reply was not JSON."))?;
        return message(&reply);
    }
    let mut content = String::new();
    let mut calls: Vec<Value> = Vec::new();
    let mut role = String::from("assistant");
    let mut saw = false;
    for event in sse_choices(text)? {
        saw = true;
        if let Some(message) = event.get("message").filter(|message| message.is_object()) {
            return Ok(message.clone());
        }
        let Some(delta) = event.get("delta") else {
            continue;
        };
        if let Some(next) = delta.get("role").and_then(Value::as_str) {
            role = next.to_string();
        }
        if let Some(piece) = delta.get("content").and_then(Value::as_str) {
            content.push_str(piece);
        }
        if let Some(list) = delta.get("tool_calls").and_then(Value::as_array) {
            merge_calls(&mut calls, list);
        }
    }
    if !saw {
        return Err(String::from("The model's reply held no message."));
    }
    let mut assembled = serde_json::json!({"role": role, "content": content});
    if !calls.is_empty() {
        assembled["tool_calls"] = Value::Array(calls);
    }
    Ok(assembled)
}

fn sse_choices(text: &str) -> Result<Vec<Value>, String> {
    let mut found = Vec::new();
    for line in text.lines() {
        let Some(data) = line.trim().strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() {
            continue;
        }
        if data == "[DONE]" {
            break;
        }
        let value: Value = serde_json::from_str(data)
            .map_err(|_| String::from("The endpoint's reply was not JSON."))?;
        if let Some(error) = value.get("error") {
            return Err(format!("The model failed: {}.", error_text(error)));
        }
        let Some(choice) = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        else {
            continue;
        };
        if let Some(error) = choice.get("error") {
            return Err(format!("The model failed: {}.", error_text(error)));
        }
        found.push(choice.clone());
    }
    Ok(found)
}

fn merge_calls(calls: &mut Vec<Value>, deltas: &[Value]) {
    for delta in deltas {
        let index = delta
            .get("index")
            .and_then(Value::as_u64)
            .unwrap_or(calls.len() as u64) as usize;
        while calls.len() <= index {
            calls.push(serde_json::json!({
                "id": "",
                "type": "function",
                "function": {"name": "", "arguments": ""},
            }));
        }
        let call = &mut calls[index];
        if let Some(id) = delta.get("id").and_then(Value::as_str) {
            call["id"] = Value::String(id.to_string());
        }
        if let Some(kind) = delta.get("type").and_then(Value::as_str) {
            call["type"] = Value::String(kind.to_string());
        }
        if let Some(function) = delta.get("function") {
            if let Some(name) = function.get("name").and_then(Value::as_str) {
                call["function"]["name"] = Value::String(name.to_string());
            }
            if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                let so_far = call["function"]["arguments"].as_str().unwrap_or("");
                call["function"]["arguments"] = Value::String(format!("{so_far}{arguments}"));
            }
        }
    }
}

/// The first choice's message of a chat reply, or why there is none.
pub fn message(reply: &Value) -> Result<Value, String> {
    if let Some(error) = reply.get("error") {
        return Err(format!("The model failed: {}.", error_text(error)));
    }
    let choice = reply
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .ok_or_else(|| String::from("The model's reply held no answer."))?;
    if let Some(error) = choice.get("error") {
        return Err(format!("The model failed: {}.", error_text(error)));
    }
    match choice.get("message") {
        Some(message) if message.is_object() => Ok(message.clone()),
        _ => Err(String::from("The model's reply held no message.")),
    }
}

/// `GET {base}/models`, every page, sorted by name. With `tools_only`, as
/// for OpenRouter, it keeps the models that can use tools; an
/// OpenAI-compatible server does not say, so all of its models are kept.
pub fn models(base_url: &str, key: Option<&str>, tools_only: bool) -> Result<Vec<Model>, String> {
    let base = base_url.trim_end_matches('/');
    let agent = agent(MODELS_TIMEOUT);
    let mut url = if tools_only {
        format!("{base}/models?supported_parameters=tools")
    } else {
        format!("{base}/models")
    };
    let mut found: Vec<Model> = Vec::new();
    for _ in 0..MODEL_PAGES {
        let mut request = agent.get(&url);
        if let Some(key) = key.filter(|key| !key.is_empty()) {
            request = request.header("Authorization", &format!("Bearer {key}"));
        }
        let page = read(request.call())?;
        if let Some(error) = page.get("error") {
            return Err(format!("The endpoint could not list the models: {}.", error_text(error)));
        }
        let Some(data) = page.get("data").and_then(Value::as_array) else {
            return Err(String::from("The endpoint's model list could not be read."));
        };
        let listed = data.iter().filter_map(|entry| if tools_only { tool_model(entry) } else { any_model(entry) });
        for model in listed {
            if !found.iter().any(|known| known.id == model.id) {
                found.push(model);
            }
        }
        match page.pointer("/links/next").and_then(Value::as_str) {
            Some(next) if !next.is_empty() => url = resolve(base, next),
            _ => break,
        }
    }
    found.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(found)
}

/// One entry of the model list, when it supports tools.
/// A model of any kind, named by its `name` or else its `id`.
/// `GET url`, with the key when there is one, within `limit`. The answer's
/// status, or the reason nothing answered.
pub fn status(url: &str, key: Option<&str>, limit: Duration) -> Result<u16, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(limit))
        .http_status_as_error(false)
        .build()
        .into();
    let mut request = agent.get(url);
    if let Some(key) = key.filter(|key| !key.is_empty()) {
        request = request.header("Authorization", &format!("Bearer {key}"));
    }
    request.call().map(|response| response.status().as_u16()).map_err(|error| error.to_string())
}

pub fn any_model(entry: &Value) -> Option<Model> {
    let id = entry.get("id")?.as_str()?.to_string();
    let name = entry
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .map(String::from)
        .unwrap_or_else(|| id.clone());
    Some(Model { id, name, images: false })
}

pub fn tool_model(entry: &Value) -> Option<Model> {
    let id = entry.get("id")?.as_str()?.to_string();
    let tools = entry
        .get("supported_parameters")
        .and_then(Value::as_array)
        .is_some_and(|parameters| parameters.iter().any(|p| p.as_str() == Some("tools")));
    if !tools {
        return None;
    }
    let name = entry
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .map(String::from)
        .unwrap_or_else(|| id.clone());
    let images = entry
        .pointer("/architecture/input_modalities")
        .and_then(Value::as_array)
        .is_some_and(|modalities| modalities.iter().any(|m| m.as_str() == Some("image")));
    Some(Model { id, name, images })
}

/// A next-page link: absolute, or a path on the API's own host.
fn resolve(base: &str, next: &str) -> String {
    if next.starts_with("http://") || next.starts_with("https://") {
        return next.to_string();
    }
    let origin = match base.find("://") {
        Some(scheme) => match base[scheme + 3..].find('/') {
            Some(path) => &base[..scheme + 3 + path],
            None => base,
        },
        None => base,
    };
    if next.starts_with('/') {
        format!("{origin}{next}")
    } else {
        format!("{base}/{next}")
    }
}

/// The body of a reply, or a sentence saying why not, with OpenRouter's own
/// message for a status that is not a success.
fn read_text(response: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<String, String> {
    let mut response = response.map_err(|error| format!("Could not reach the endpoint: {error}."))?;
    let status = response.status();
    let text = body_text(&mut response)?;
    if !status.is_success() {
        return Err(failure(status.as_u16(), &text));
    }
    Ok(text)
}

fn body_text(response: &mut ureq::http::Response<ureq::Body>) -> Result<String, String> {
    response
        .body_mut()
        .with_config()
        .limit(BODY_LIMIT)
        .read_to_string()
        .map_err(|error| format!("The endpoint's reply could not be read: {error}."))
}

/// A status that is not a success, with OpenRouter's own message.
fn failure(status: u16, text: &str) -> String {
    let detail = serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|body| body.get("error").map(error_text))
        .unwrap_or_else(|| {
            let text = text.trim();
            if text.is_empty() {
                String::from("no message")
            } else {
                text.chars().take(300).collect()
            }
        });
    format!("The endpoint answered {status}: {detail}.")
}

/// How long one picture may take to make.
const IMAGE_TIMEOUT: Duration = Duration::from_secs(600);

/// One picture from a prompt: `POST {base}/images`, always at high quality,
/// as PNG. Returns the PNG's bytes, or a sentence saying why not.
pub fn make_image(base_url: &str, key: &str, model: &str, prompt: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    let url = format!("{}/images", base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": model,
        "prompt": prompt,
        "n": 1,
        "quality": "high",
        "output_format": "png",
    });
    let mut request = agent(IMAGE_TIMEOUT).post(&url);
    if !key.is_empty() {
        request = request.header("Authorization", &format!("Bearer {key}"));
    }
    let response = request
        .header("HTTP-Referer", "https://github.com/Spumcake/Inpainter")
        .header("X-Title", "Inpainter")
        .header("Content-Type", "application/json")
        .send(body.to_string());
    let reply = read(response)?;
    let data = reply
        .pointer("/data/0/b64_json")
        .and_then(Value::as_str)
        .ok_or_else(|| String::from("The endpoint's reply held no picture."))?;
    let png = base64::engine::general_purpose::STANDARD
        .decode(data.trim())
        .map_err(|_| String::from("The endpoint's picture could not be read."))?;
    if !png.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(String::from("The endpoint's picture was not a PNG."));
    }
    Ok(png)
}

/// The image models an endpoint offers: `GET {base}/images/models`.
pub fn image_models(base_url: &str, key: Option<&str>) -> Result<Vec<Model>, String> {
    let url = format!("{}/images/models", base_url.trim_end_matches('/'));
    let mut request = agent(MODELS_TIMEOUT).get(&url);
    if let Some(key) = key.filter(|key| !key.is_empty()) {
        request = request.header("Authorization", &format!("Bearer {key}"));
    }
    let page = read(request.call())?;
    let data = page
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| String::from("The endpoint's image models could not be read."))?;
    Ok(data.iter().filter_map(any_model).collect())
}

/// The body of a reply as JSON, or a sentence saying why not.
fn read(response: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<Value, String> {
    let text = read_text(response)?;
    serde_json::from_str(&text).map_err(|_| String::from("The endpoint's reply was not JSON."))
}

/// OpenRouter's `{code, message, metadata?}`, as one line, with the
/// provider's own words when it passed them on.
fn error_text(error: &Value) -> String {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .map(String::from)
        .unwrap_or_else(|| error.to_string());
    let raw = error
        .pointer("/metadata/raw")
        .map(|raw| match raw.as_str() {
            Some(text) => text.to_string(),
            None => raw.to_string(),
        })
        .filter(|raw| !raw.trim().is_empty() && !message.contains(raw.as_str()));
    let line = match raw {
        Some(raw) => format!("{message} ({})", raw.chars().take(300).collect::<String>()),
        None => message,
    };
    line.trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::resolve;

    #[test]
    fn next_page_links_resolve_on_the_api_host() {
        let base = "https://openrouter.ai/api/v1";
        assert_eq!(
            resolve(base, "/api/v1/models?offset=500"),
            "https://openrouter.ai/api/v1/models?offset=500"
        );
        assert_eq!(resolve(base, "https://x.test/m"), "https://x.test/m");
    }
}
