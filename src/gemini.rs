//! Gemini Live API client for real-time transcription.
//!
//! Uses the dedicated live-transcription model (`gemini-3.5-transcribe-live`)
//! over the `BidiGenerateContent` WebSocket with `inputAudioTranscription`,
//! exactly like Antigravity's record button and Google's Live transcription doc.

use std::time::Duration;

use base64::Engine as _;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use crate::config::Config;

const WS_URL: &str =
    "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.\
     GenerativeService.BidiGenerateContent";

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Debug, Clone)]
pub enum TranscriptEvent {
    Ready,
    Interim(String),
    Final(String),
    /// The server says the turn is fully transcribed.
    TurnComplete,
    Error(String),
    Closed,
}

/// What one server message means for us.
#[derive(Debug, PartialEq)]
enum Parsed {
    Ready,
    Error(String),
    Content {
        interim: Option<String>,
        fin: Option<String>,
        turn_complete: bool,
    },
    Ignore,
}

fn parse(value: &Value) -> Parsed {
    if value.get("setupComplete").is_some() {
        return Parsed::Ready;
    }
    if let Some(err) = value.get("error") {
        return Parsed::Error(
            err.get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error")
                .to_string(),
        );
    }
    if let Some(sc) = value.get("serverContent") {
        let text = |field: &str| {
            sc.get(field)
                .and_then(|x| x.get("text"))
                .and_then(|t| t.as_str())
                .map(str::to_string)
        };
        let flag = |field: &str| sc.get(field).and_then(Value::as_bool).unwrap_or(false);
        return Parsed::Content {
            interim: text("interimInputTranscription"),
            fin: text("inputTranscription"),
            turn_complete: flag("turnComplete") || flag("generationComplete"),
        };
    }
    Parsed::Ignore
}

/// Human-readable reason for an abnormal close frame, if any.
fn close_reason(message: &Message) -> Option<String> {
    let Message::Close(Some(frame)) = message else {
        return None;
    };
    if frame.code == CloseCode::Normal {
        return None;
    }
    let reason = frame.reason.to_string();
    Some(if reason.trim().is_empty() {
        format!("connection closed by server ({})", u16::from(frame.code))
    } else {
        reason
    })
}

async fn connect_and_setup(
    url: &str,
    cfg: &Config,
) -> anyhow::Result<(SplitSink<Ws, Message>, SplitStream<Ws>)> {
    let mut request = url.into_client_request()?;
    request.headers_mut().insert(
        HeaderName::from_static("x-goog-api-key"),
        HeaderValue::from_str(cfg.api_key.trim())
            .map_err(|_| anyhow::anyhow!("the API key contains invalid characters"))?,
    );

    let (ws, _resp) = connect_async(request)
        .await
        .map_err(|e| anyhow::anyhow!("cannot reach Gemini: {e}"))?;
    let (mut write, read) = ws.split();

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
    Ok((write, read))
}

/// Check that the key and model work: connect, set up, wait for the server's
/// `setupComplete`. Sends no audio.
pub async fn verify(cfg: &Config) -> anyhow::Result<()> {
    verify_at(WS_URL, cfg).await
}

async fn verify_at(url: &str, cfg: &Config) -> anyhow::Result<()> {
    let attempt = async {
        let (mut write, mut read) = connect_and_setup(url, cfg).await?;
        while let Some(message) = read.next().await {
            let message = message?;
            if let Some(reason) = close_reason(&message) {
                anyhow::bail!(reason);
            }
            let Ok(text) = message.into_text() else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            match parse(&value) {
                Parsed::Ready => {
                    let _ = write.send(Message::Close(None)).await;
                    return Ok(());
                }
                Parsed::Error(e) => anyhow::bail!(e),
                _ => {}
            }
        }
        anyhow::bail!("the server closed the connection without accepting the setup")
    };
    tokio::time::timeout(Duration::from_secs(12), attempt)
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for Gemini"))?
}

/// Spawn the transcription session. Returns immediately; the session runs until
/// it errors, the server closes it, or the returned task handle is aborted.
pub fn spawn(
    cfg: Config,
    audio_rx: UnboundedReceiver<Vec<i16>>,
    events: UnboundedSender<TranscriptEvent>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = run(WS_URL, cfg, audio_rx, events.clone()).await {
            let _ = events.send(TranscriptEvent::Error(e.to_string()));
        }
        let _ = events.send(TranscriptEvent::Closed);
    })
}

/// Aborts the wrapped task when dropped (so aborting the session also stops
/// the audio writer).
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn run(
    url: &str,
    cfg: Config,
    mut audio_rx: UnboundedReceiver<Vec<i16>>,
    events: UnboundedSender<TranscriptEvent>,
) -> anyhow::Result<()> {
    let (mut write, mut read) = connect_and_setup(url, &cfg).await?;

    // --- audio writer task -------------------------------------------------
    let _writer = AbortOnDrop(tokio::spawn(async move {
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
    }));

    // --- read loop ---------------------------------------------------------
    while let Some(message) = read.next().await {
        let message = message?;
        if let Some(reason) = close_reason(&message) {
            anyhow::bail!(reason);
        }
        let Ok(text) = message.into_text() else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        match parse(&value) {
            Parsed::Ready => {
                let _ = events.send(TranscriptEvent::Ready);
            }
            Parsed::Error(e) => {
                let _ = events.send(TranscriptEvent::Error(e));
            }
            Parsed::Content {
                interim,
                fin,
                turn_complete,
            } => {
                if let Some(t) = interim {
                    let _ = events.send(TranscriptEvent::Interim(t));
                }
                if let Some(t) = fin {
                    let _ = events.send(TranscriptEvent::Final(t));
                }
                if turn_complete {
                    let _ = events.send(TranscriptEvent::TurnComplete);
                }
            }
            Parsed::Ignore => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FileConfig, Mode};
    use tokio::net::TcpListener;
    use tokio::sync::mpsc::unbounded_channel;
    use tokio_tungstenite::tungstenite::protocol::CloseFrame;

    fn test_config() -> Config {
        let mut cfg = Config::from_file(FileConfig::default());
        cfg.api_key = "test-key".into();
        cfg.mode = Mode::Smart;
        cfg.custom_vocabulary = vec!["Lalia".into()];
        cfg
    }

    /// A tiny stand-in for the Live API: checks the setup, answers like the
    /// real service and returns how many audio chunks it received.
    async fn mock_server(listener: TcpListener) -> usize {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        let setup: Value =
            serde_json::from_str(&ws.next().await.unwrap().unwrap().into_text().unwrap()).unwrap();
        let iat = &setup["setup"]["inputAudioTranscription"];
        assert_eq!(iat["mode"], "SMART");
        assert_eq!(iat["customVocabulary"][0], "Lalia");
        assert_eq!(setup["setup"]["model"], "models/gemini-3.5-transcribe-live");
        ws.send(Message::text(json!({"setupComplete": {}}).to_string()))
            .await
            .unwrap();
        let mut chunks = 0;
        while let Some(Ok(msg)) = ws.next().await {
            let v: Value = serde_json::from_str(&msg.into_text().unwrap()).unwrap();
            if v["realtimeInput"]["audioStreamEnd"] == true {
                for reply in [
                    json!({"serverContent": {"inputTranscription": {"text": "hello world"}}}),
                    json!({"serverContent": {"turnComplete": true}}),
                ] {
                    ws.send(Message::text(reply.to_string())).await.unwrap();
                }
                break;
            }
            let data = v["realtimeInput"]["audio"]["data"].as_str().unwrap();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data)
                .unwrap();
            assert_eq!(bytes.len(), 3200, "100 ms of 16 kHz PCM16");
            chunks += 1;
            if chunks == 1 {
                let interim =
                    json!({"serverContent": {"interimInputTranscription": {"text": "hel"}}});
                ws.send(Message::text(interim.to_string())).await.unwrap();
            }
        }
        chunks
    }

    #[tokio::test]
    async fn streams_audio_and_reports_transcripts() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(mock_server(listener));

        let (audio_tx, audio_rx) = unbounded_channel();
        let (events_tx, mut events_rx) = unbounded_channel();
        let client =
            tokio::spawn(async move { run(&url, test_config(), audio_rx, events_tx).await });
        audio_tx.send(vec![0i16; 1600]).unwrap();
        audio_tx.send(vec![1i16; 1600]).unwrap();
        drop(audio_tx); // end of speech → audioStreamEnd

        let mut seen = Vec::new();
        while let Ok(Some(ev)) =
            tokio::time::timeout(Duration::from_secs(5), events_rx.recv()).await
        {
            let done = matches!(ev, TranscriptEvent::TurnComplete);
            seen.push(format!("{ev:?}"));
            if done {
                break;
            }
        }
        assert_eq!(
            seen,
            vec![
                "Ready",
                "Interim(\"hel\")",
                "Final(\"hello world\")",
                "TurnComplete"
            ]
        );
        assert_eq!(server.await.unwrap(), 2);
        client.abort();
    }

    #[tokio::test]
    async fn rejected_key_is_reported() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            let _setup = ws.next().await;
            let _ = ws
                .close(Some(CloseFrame {
                    code: CloseCode::Policy,
                    reason: "API key not valid. Please pass a valid API key.".into(),
                }))
                .await;
        });
        let err = verify_at(&url, &test_config()).await.unwrap_err();
        assert!(err.to_string().contains("API key not valid"), "{err}");
    }

    #[test]
    fn parses_server_messages() {
        assert_eq!(parse(&json!({"setupComplete": {}})), Parsed::Ready);
        assert_eq!(
            parse(&json!({"error": {"message": "API key not valid"}})),
            Parsed::Error("API key not valid".into())
        );
        assert_eq!(
            parse(&json!({"serverContent": {
                "interimInputTranscription": {"text": "hel"},
                "inputTranscription": {"text": "hello"},
                "turnComplete": true
            }})),
            Parsed::Content {
                interim: Some("hel".into()),
                fin: Some("hello".into()),
                turn_complete: true
            }
        );
        assert_eq!(parse(&json!({"usageMetadata": {}})), Parsed::Ignore);
    }

    #[test]
    fn abnormal_close_is_an_error() {
        let bad = Message::Close(Some(CloseFrame {
            code: CloseCode::Policy,
            reason: "API key not valid. Please pass a valid API key.".into(),
        }));
        assert_eq!(
            close_reason(&bad).as_deref(),
            Some("API key not valid. Please pass a valid API key.")
        );
        let normal = Message::Close(Some(CloseFrame {
            code: CloseCode::Normal,
            reason: "".into(),
        }));
        assert_eq!(close_reason(&normal), None);
        assert_eq!(close_reason(&Message::Close(None)), None);
    }
}
