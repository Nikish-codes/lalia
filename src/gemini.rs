//! Gemini Live API client for real-time transcription.
//!
//! Uses the dedicated live-transcription model (`gemini-3.5-transcribe-live`)
//! over the `BidiGenerateContent` WebSocket with `inputAudioTranscription`,
//! exactly like Antigravity's record button and Google's Live transcription doc.

use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::Message;

use crate::config::Config;

const WS_URL: &str =
    "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.\
     GenerativeService.BidiGenerateContent";

#[derive(Debug, Clone)]
pub enum TranscriptEvent {
    Ready,
    Interim(String),
    Final(String),
    Error(String),
    Closed,
}

/// Spawn the transcription session. Returns immediately; the session runs until
/// it errors, the server closes it, or the returned task handle is aborted.
pub fn spawn(
    cfg: Config,
    audio_rx: UnboundedReceiver<Vec<i16>>,
    events: UnboundedSender<TranscriptEvent>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = run(cfg, audio_rx, events.clone()).await {
            let _ = events.send(TranscriptEvent::Error(e.to_string()));
        }
        let _ = events.send(TranscriptEvent::Closed);
    })
}

async fn run(
    cfg: Config,
    mut audio_rx: UnboundedReceiver<Vec<i16>>,
    events: UnboundedSender<TranscriptEvent>,
) -> anyhow::Result<()> {
    let mut request = WS_URL.into_client_request()?;
    request.headers_mut().insert(
        HeaderName::from_static("x-goog-api-key"),
        HeaderValue::from_str(&cfg.api_key)?,
    );

    let (ws, _resp) = connect_async(request).await?;
    let (mut write, mut read) = ws.split();

    // --- setup message -----------------------------------------------------
    let mut iat = json!({ "mode": cfg.mode.as_str() });
    if !cfg.language_codes.is_empty() {
        iat["languageCodes"] = json!(cfg.language_codes);
    }
    if !cfg.custom_vocabulary.is_empty() {
        iat["customVocabulary"] = json!(cfg.custom_vocabulary);
    }
    let setup = json!({
        "setup": {
            "model": format!("models/{}", cfg.model),
            "generationConfig": { "responseModalities": ["TEXT"] },
            "inputAudioTranscription": iat,
        }
    });
    write.send(Message::text(setup.to_string())).await?;

    // --- audio writer task -------------------------------------------------
    let writer = tokio::spawn(async move {
        let engine = base64::engine::general_purpose::STANDARD;
        while let Some(chunk) = audio_rx.recv().await {
            let mut bytes = Vec::with_capacity(chunk.len() * 2);
            for s in chunk {
                bytes.extend_from_slice(&s.to_le_bytes());
            }
            let msg = json!({
                "realtimeInput": {
                    "audio": { "mimeType": "audio/pcm;rate=16000", "data": engine.encode(&bytes) }
                }
            });
            if write.send(Message::text(msg.to_string())).await.is_err() {
                return;
            }
        }
        // Audio source ended → ask the server to finalize immediately (hybrid VAD).
        let _ = write
            .send(Message::text(
                json!({"realtimeInput": {"audioStreamEnd": true}}).to_string(),
            ))
            .await;
    });

    // --- read loop ---------------------------------------------------------
    while let Some(message) = read.next().await {
        let message = match message {
            Ok(m) => m,
            Err(e) => {
                let _ = events.send(TranscriptEvent::Error(e.to_string()));
                break;
            }
        };
        let text = match message.into_text() {
            Ok(t) => t,
            Err(_) => continue,
        };
        let value: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if value.get("setupComplete").is_some() {
            let _ = events.send(TranscriptEvent::Ready);
            continue;
        }
        if let Some(err) = value.get("error") {
            let _ = events.send(TranscriptEvent::Error(
                err.get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown error")
                    .to_string(),
            ));
            continue;
        }
        if let Some(sc) = value.get("serverContent") {
            if let Some(t) = sc
                .get("interimInputTranscription")
                .and_then(|x| x.get("text"))
                .and_then(|t| t.as_str())
            {
                let _ = events.send(TranscriptEvent::Interim(t.to_string()));
            }
            if let Some(t) = sc
                .get("inputTranscription")
                .and_then(|x| x.get("text"))
                .and_then(|t| t.as_str())
            {
                let _ = events.send(TranscriptEvent::Final(t.to_string()));
            }
        }
    }

    writer.abort();
    Ok(())
}
