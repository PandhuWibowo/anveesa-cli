use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::State,
    response::sse::{Event, KeepAlive, Sse},
    response::{Html, IntoResponse},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tokio_stream::StreamExt as _;
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::{
    cli::AskOptions,
    config::AppConfig,
    provider::{ApprovalPolicy, ChatMessage, PromptRequest, StreamEvent},
    workspace::workspace_context,
};

// ── Embedded chat UI ─────────────────────────────────────────────────────────

const HTML: &str = include_str!("web_ui.html");

// ── Server state ──────────────────────────────────────────────────────────────

#[derive(Clone)]
struct WebState {
    config: Arc<AppConfig>,
    provider_name: String,
    model: String,
    options: AskOptions,
}

// ── Request / response types ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct AskBody {
    prompt: String,
    #[serde(default)]
    history: Vec<HistoryMsg>,
}

#[derive(Deserialize)]
struct HistoryMsg {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct InfoResponse<'a> {
    provider: &'a str,
    model: &'a str,
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub async fn run_web(options: AskOptions, port: u16) -> Result<()> {
    let config = Arc::new(AppConfig::load()?);
    let provider_name = config
        .provider_name(options.provider.as_deref())?
        .to_string();
    let model = options
        .model
        .clone()
        .or_else(|| {
            config
                .providers
                .get(&provider_name)
                .and_then(|p| p.default_model().map(str::to_string))
        })
        .unwrap_or_else(|| "default".to_string());

    let state = WebState {
        config,
        provider_name,
        model,
        options,
    };

    let app = Router::new()
        .route("/", get(serve_ui))
        .route("/api/info", get(handle_info))
        .route("/api/ask", post(handle_ask))
        .with_state(state);

    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("cannot bind to {addr} — is the port already in use?"))?;

    let url = format!("http://127.0.0.1:{port}");
    println!("Anveesa web UI → {url}");
    println!("Press Ctrl+C to stop.");
    open_browser(&url);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("web server error")
}

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn serve_ui() -> Html<&'static str> {
    Html(HTML)
}

async fn handle_info(State(state): State<WebState>) -> Json<InfoResponse<'static>> {
    // Leak the strings into 'static so we can return borrows — acceptable for
    // the lifetime of the process.
    let provider: &'static str = Box::leak(state.provider_name.into_boxed_str());
    let model: &'static str = Box::leak(state.model.into_boxed_str());
    Json(InfoResponse { provider, model })
}

async fn handle_ask(State(state): State<WebState>, Json(body): Json<AskBody>) -> impl IntoResponse {
    let history: Vec<ChatMessage> = body
        .history
        .into_iter()
        .map(|m| {
            if m.role == "user" {
                ChatMessage::user(m.content)
            } else {
                ChatMessage::assistant(m.content)
            }
        })
        .collect();

    let workspace = workspace_context().ok();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<StreamEvent>();

    tokio::spawn(async move {
        let request = PromptRequest {
            prompt: body.prompt,
            model: state.options.model.clone(),
            system: state.options.system.clone(),
            workspace_context: workspace,
            history,
            images: vec![],
            mcp: None,
        };
        let _ = crate::provider::ask(
            &state.config,
            &state.provider_name,
            request,
            ApprovalPolicy::Deny,
            &tx,
        )
        .await;
    });

    let stream = UnboundedReceiverStream::new(rx).filter_map(|ev| {
        let data = match ev {
            StreamEvent::Token(t) => {
                let escaped = serde_json::to_string(&t).unwrap_or_default();
                Some(format!("{{\"token\":{escaped}}}"))
            }
            StreamEvent::Usage(_) => Some("{\"done\":true}".to_string()),
            _ => None,
        };
        data.map(|d| Ok::<Event, std::convert::Infallible>(Event::default().data(d)))
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/c", "start", url])
        .spawn();
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    println!("\nAnveesa web UI stopped.");
}
