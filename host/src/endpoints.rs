//! Where a prompt goes, worked out from settings, and what the Endpoints
//! page asks of an endpoint: whether it answers, and which models it offers.
//! Both questions run on their own threads, so the window never waits.

use eframe::egui;
use crate::workspace::{Effort, EndpointKind, EndpointSetting, Level, Settings, SkillSetting};
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

/// How long a connection test waits for an answer.
const PROBE_LIMIT: Duration = Duration::from_secs(5);

/// Everything one run needs: which endpoint, how to reach it, and what to ask.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Route {
    /// The endpoint's id, recorded on the cell so a rerun goes back to it.
    pub endpoint: String,
    pub name: String,
    pub base: String,
    pub key: Option<String>,
    pub kind: EndpointKind,
    pub model: String,
    pub effort: Option<Effort>,
}

impl Route {
    pub(crate) fn new(endpoint: &EndpointSetting, key: Option<String>, model: &str, effort: Option<Effort>) -> Self {
        Self {
            endpoint: endpoint.id.clone(),
            name: endpoint.name.clone(),
            base: endpoint.address.trim_end_matches('/').to_string(),
            key,
            kind: endpoint.kind,
            model: model.to_string(),
            effort,
        }
    }

    pub(crate) fn reasoning(&self) -> assistant::Reasoning {
        match self.kind {
            EndpointKind::Openrouter => assistant::Reasoning::OpenRouter,
            _ => assistant::Reasoning::OpenAi,
        }
    }

    /// The effort, in the field this kind of endpoint reads.
    pub(crate) fn extra(&self) -> serde_json::Map<String, serde_json::Value> {
        assistant::effort_fields(self.reasoning(), self.effort.map(Effort::word))
    }

    /// Why this route cannot run, if it cannot. Only OpenRouter needs a
    /// key: a local server such as Ollama is sent none.
    pub(crate) fn blocked(&self) -> Option<String> {
        if !self.kind.chats() {
            return Some(format!(
                "“{}” is a ComfyUI endpoint, which does not answer prompts. Choose a chat endpoint in Settings.",
                self.name
            ));
        }
        if self.kind == EndpointKind::Openrouter && self.key.as_deref().is_none_or(|key| key.trim().is_empty()) {
            return Some(format!("No key is set for “{}”. Add one in Settings, under Endpoints.", self.name));
        }
        if self.model.trim().is_empty() {
            return Some(String::from("No model is chosen. Choose one in Settings."));
        }
        None
    }
}

/// Said when a level or skill names no endpoint that exists.
pub(crate) const NO_ENDPOINT: &str = "No endpoint is set for this. Choose one in Settings.";

/// A level's route: its model at its effort, on Discussion's endpoint.
/// Levels follow the Discussion skill's endpoint, as the owner set on
/// 2026-10-29; a level's own endpoint is used only when Discussion has none.
pub(crate) fn level_route(settings: &Settings, level: &Level, key: impl FnOnce(&str) -> Option<String>) -> Result<Route, String> {
    let endpoint = settings
        .discussion_endpoint()
        .or_else(|| settings.endpoint(&level.endpoint))
        .ok_or(NO_ENDPOINT)?;
    Ok(Route::new(endpoint, key(&endpoint.id), &level.model, level.effort))
}

/// A skill's route: its model on its endpoint, at no set effort.
pub(crate) fn skill_route(settings: &Settings, skill: &SkillSetting, key: impl FnOnce(&str) -> Option<String>) -> Result<Route, String> {
    let endpoint = skill.endpoint.as_deref().and_then(|id| settings.endpoint(id)).ok_or(NO_ENDPOINT)?;
    Ok(Route::new(endpoint, key(&endpoint.id), skill.model.as_deref().unwrap_or_default(), None))
}

/// What a connection test found.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Status {
    Checking,
    Connected,
    /// Nothing answered.
    NotRunning,
    /// It answered 401 or 403.
    KeyMissing,
    /// It answered with another error.
    Answered(u16),
}

impl Status {
    pub(crate) fn label(&self) -> String {
        match self {
            Status::Checking => "Checking…".into(),
            Status::Connected => "Connected".into(),
            Status::NotRunning => "Not running".into(),
            Status::KeyMissing => "Key missing".into(),
            Status::Answered(code) => format!("Error {code}"),
        }
    }
}

/// The address a connection test asks. OpenRouter lists its models to
/// anyone, so its test asks about the key itself, which a wrong key is
/// refused.
fn probe_url(kind: EndpointKind, address: &str) -> String {
    let base = address.trim_end_matches('/');
    match kind {
        EndpointKind::Comfyui => format!("{base}/system_stats"),
        EndpointKind::Openrouter => format!("{base}/key"),
        EndpointKind::Openai => format!("{base}/models"),
    }
}

/// Tests an endpoint: an OpenRouter endpoint with no key is missing it
/// without asking; any other is asked.
pub(crate) fn check(kind: EndpointKind, address: &str, key: Option<String>) -> Status {
    if kind == EndpointKind::Openrouter && key.as_deref().is_none_or(|key| key.trim().is_empty()) {
        return Status::KeyMissing;
    }
    status_of(assistant::status(&probe_url(kind, address), key.as_deref(), PROBE_LIMIT))
}

fn status_of(answer: Result<u16, String>) -> Status {
    match answer {
        Ok(code) if (200..300).contains(&code) => Status::Connected,
        Ok(401 | 403) => Status::KeyMissing,
        Ok(code) => Status::Answered(code),
        Err(_) => Status::NotRunning,
    }
}

/// One question being asked, and its answer once it came.
enum Asked<T> {
    Waiting(Receiver<T>),
    Answered(T),
}

impl<T> Asked<T> {
    fn poll(&mut self) -> Option<&T> {
        if let Asked::Waiting(rx) = self
            && let Ok(answer) = rx.try_recv()
        {
            *self = Asked::Answered(answer);
        }
        match self {
            Asked::Answered(answer) => Some(answer),
            Asked::Waiting(_) => None,
        }
    }
}

fn ask<T: Send + 'static>(ctx: &egui::Context, work: impl FnOnce() -> T + Send + 'static) -> Asked<T> {
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    let _ = std::thread::Builder::new().name(String::from("endpoint")).spawn(move || {
        let _ = tx.send(work());
        ctx.request_repaint();
    });
    Asked::Waiting(rx)
}

pub(crate) type Models = Result<Vec<assistant::Model>, String>;

/// Statuses and model lists, by endpoint id, asked once until forgotten.
#[derive(Default)]
pub(crate) struct Probes {
    statuses: HashMap<String, Asked<Status>>,
    models: HashMap<String, Asked<Models>>,
    /// The form's own test, apart from the saved endpoints'.
    form: Option<Asked<Status>>,
}

impl Probes {
    /// The endpoint's status, asking for it the first time.
    pub(crate) fn status(&mut self, ctx: &egui::Context, endpoint: &EndpointSetting, key: Option<String>) -> Status {
        let asked = self.statuses.entry(endpoint.id.clone()).or_insert_with(|| {
            let (kind, address) = (endpoint.kind, endpoint.address.clone());
            ask(ctx, move || check(kind, &address, key))
        });
        asked.poll().cloned().unwrap_or(Status::Checking)
    }

    /// Tests what the form holds, now.
    pub(crate) fn test_form(&mut self, ctx: &egui::Context, kind: EndpointKind, address: &str, key: Option<String>) {
        let address = address.to_string();
        self.form = Some(ask(ctx, move || check(kind, &address, key)));
    }

    pub(crate) fn form_status(&mut self) -> Option<Status> {
        let asked = self.form.as_mut()?;
        Some(asked.poll().cloned().unwrap_or(Status::Checking))
    }

    pub(crate) fn clear_form(&mut self) {
        self.form = None;
    }

    /// The models a chat endpoint offers, asking for them the first time.
    /// OpenRouter's list keeps the models that can use tools; another
    /// server's keeps every model it names.
    pub(crate) fn models(&mut self, ctx: &egui::Context, endpoint: &EndpointSetting, key: Option<String>) -> Option<&Models> {
        if !endpoint.kind.chats() {
            return None;
        }
        let asked = self.models.entry(endpoint.id.clone()).or_insert_with(|| {
            let base = endpoint.address.trim_end_matches('/').to_string();
            let tools_only = endpoint.kind == EndpointKind::Openrouter;
            ask(ctx, move || assistant::models(&base, key.as_deref(), tools_only))
        });
        asked.poll()
    }

    /// The model lists already read, for naming a model on a card.
    pub(crate) fn known_models(&mut self) -> Vec<assistant::Model> {
        let mut known = Vec::new();
        for asked in self.models.values_mut() {
            if let Some(Ok(list)) = asked.poll() {
                known.extend(list.iter().cloned());
            }
        }
        known
    }

    /// Asks again next time, as after an edit or when the page reopens.
    pub(crate) fn forget(&mut self, endpoint: &str) {
        self.statuses.remove(endpoint);
        self.models.remove(endpoint);
    }

    pub(crate) fn forget_statuses(&mut self) {
        self.statuses.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(kind: EndpointKind) -> EndpointSetting {
        EndpointSetting {
            id: "e".into(),
            name: "Mine".into(),
            kind,
            address: "http://localhost:11434/v1/".into(),
        }
    }

    /// Catches a local server shut out for having no key, a ComfyUI
    /// endpoint sent prompts, and an effort put where the server ignores it.
    #[test]
    fn a_route_runs_where_it_can_and_says_why_not() {
        let local = Route::new(&endpoint(EndpointKind::Openai), None, "llama3.2", Some(Effort::High));
        assert_eq!(local.blocked(), None);
        assert_eq!(local.base, "http://localhost:11434/v1");
        assert_eq!(local.extra()["reasoning_effort"], "high");
        let router = Route::new(&endpoint(EndpointKind::Openrouter), None, "m", None);
        assert!(router.blocked().unwrap().contains("No key is set for “Mine”"));
        let router = Route::new(&endpoint(EndpointKind::Openrouter), Some("k".into()), "m", Some(Effort::Low));
        assert_eq!(router.blocked(), None);
        assert_eq!(router.extra()["reasoning"]["effort"], "low");
        let graphs = Route::new(&endpoint(EndpointKind::Comfyui), None, "m", None);
        assert!(graphs.blocked().unwrap().contains("ComfyUI"));
        assert_eq!(probe_url(EndpointKind::Comfyui, "http://127.0.0.1:8188/"), "http://127.0.0.1:8188/system_stats");
        assert_eq!(status_of(Ok(401)), Status::KeyMissing);
        assert_eq!(status_of(Err("refused".into())), Status::NotRunning);
        assert_eq!(status_of(Ok(200)), Status::Connected);
    }

    /// Catches a connection test that says Connected to a wrong key:
    /// OpenRouter answers its model list to anyone, as this stand-in does,
    /// and refuses the key question to a key it does not know.
    #[test]
    fn a_wrong_openrouter_key_is_found() {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}/api/v1", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                reader.read_line(&mut request).unwrap();
                let mut good = false;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    if header.trim().is_empty() {
                        break;
                    }
                    good |= header.to_lowercase().starts_with("authorization: bearer good");
                }
                let status = if request.contains("/key") && !good { "401 Unauthorized" } else { "200 OK" };
                let _ = write!(stream, "HTTP/1.1 {status}\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{{}}");
            }
        });
        assert_eq!(check(EndpointKind::Openrouter, &address, Some("wrong".into())), Status::KeyMissing);
        assert_eq!(check(EndpointKind::Openrouter, &address, None), Status::KeyMissing);
        assert_eq!(check(EndpointKind::Openrouter, &address, Some("good".into())), Status::Connected);
        assert_eq!(check(EndpointKind::Openai, &address, None), Status::Connected);
    }
}
