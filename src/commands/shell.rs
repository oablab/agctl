//! Interactive PTY shell via InvokeAgentRuntimeCommandShell WebSocket API.
//!
//! # Protocol
//!
//! Reference: https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/runtime-get-started-command-shell.html
//!
//! **Endpoint:**
//!   `wss://bedrock-agentcore.{region}.amazonaws.com/runtimes/{url_encoded_arn}/ws/shells?qualifier=DEFAULT`
//!
//! **Auth:** SigV4 header signing (Authorization + X-Amz-Date on the HTTP upgrade request).
//!   NOT presigned URL. Session ID passed via signed header:
//!   `X-Amzn-Bedrock-AgentCore-Runtime-Session-Id`
//!
//! **Binary frame format (1-byte type prefix + payload):**
//!
//! | Byte | Direction       | Meaning                              |
//! |------|-----------------|--------------------------------------|
//! | 0x00 | client→server   | stdin (raw terminal input)           |
//! | 0x01 | server→client   | stdout                               |
//! | 0x02 | server→client   | stderr                               |
//! | 0x04 | client→server   | resize: `{"width":N,"height":N}`     |
//! | 0x05 | client→server   | heartbeat (keepalive)                |
//! | 0xFF | client→server   | close                                |
//!
//! **Initial handshake:** Server sends a Text frame with JSON metadata:
//!   `{"apiVersion":"v1","kind":"Status","metadata":{"shellId":"..."},"status":"Success"}`
//!   The `shellId` is needed for reconnection.
//!
//! **Limits:** 64KB max frame, 250 frames/sec, 1hr max connection, 10 concurrent shells.
//! **Close codes:** 4000 = kicked by another client, 1008 = TTL/rate limit.

use anyhow::Result;
use aws_credential_types::provider::ProvideCredentials;
use aws_sigv4::http_request::{sign, SignableBody, SignableRequest, SigningSettings};
use aws_sigv4::sign::v4;
use crossterm::terminal;
use futures_util::{SinkExt, StreamExt};
use std::time::SystemTime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::tungstenite::http;

/// Open an interactive PTY shell via InvokeAgentRuntimeCommandShell WebSocket.
pub async fn open_shell(
    arn: &str,
    session_id: &str,
    shell_id: Option<&str>,
    region: &str,
) -> Result<()> {
    let request = build_signed_request(arn, session_id, shell_id, region).await?;

    let (ws_stream, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|e| anyhow::anyhow!("WebSocket connect failed: {e}"))?;

    let (mut ws_write, mut ws_read) = ws_stream.split();

    terminal::enable_raw_mode()?;
    let _guard = RawModeGuard;

    let mut stdout = tokio::io::stdout();

    // Spawn a blocking thread for stdin (tokio::io::stdin doesn't work in raw mode on macOS)
    let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(32);
    std::thread::spawn(move || {
        use std::io::Read;
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 4096];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if stdin_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Don't send resize on connect — server uses default PTY size.
    // TODO: implement SIGWINCH handler for dynamic resize.
    // Send initial resize with proper framing (0x04 prefix)
    if let Ok((cols, rows)) = terminal::size() {
        let resize_json = format!("{{\"width\":{cols},\"height\":{rows}}}");
        let mut frame = vec![0x04u8];
        frame.extend_from_slice(resize_json.as_bytes());
        let _ = ws_write.send(Message::Binary(frame.into())).await;
    }

    loop {
        tokio::select! {
            Some(data) = stdin_rx.recv() => {
                if data.contains(&0x1d) { // Ctrl+]
                    // Send close frame (0xFF)
                    let _ = ws_write.send(Message::Binary(vec![0xFF].into())).await;
                    eprintln!("\r\nDetached.");
                    break;
                }
                // Frame type 0x00 = stdin
                let mut frame = Vec::with_capacity(1 + data.len());
                frame.push(0x00);
                frame.extend_from_slice(&data);
                ws_write.send(Message::Binary(frame.into())).await?;
            }
            msg = ws_read.next() => {
                match msg {
                    Some(Ok(Message::Binary(data))) => {
                        if data.len() > 1 {
                            // First byte is frame type: 0x01=stdout, 0x02=stderr, etc.
                            // Skip it and write the rest
                            let frame_type = data[0];
                            let payload = &data[1..];
                            match frame_type {
                                0x01 | 0x02 => {
                                    stdout.write_all(payload).await?;
                                    stdout.flush().await?;
                                }
                                _ => {
                                    // Other frame types (status, etc.) - write as-is for now
                                    stdout.write_all(payload).await?;
                                    stdout.flush().await?;
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        // Status/metadata frames come as text
                        stdout.write_all(text.as_bytes()).await?;
                        stdout.flush().await?;
                    }
                    Some(Ok(Message::Close(frame))) => {
                        if let Some(f) = &frame {
                            let code: u16 = f.code.into();
                            if code == 4000 {
                                eprintln!("\r\nSession taken over by another client.");
                            }
                        }
                        break;
                    }
                    Some(Ok(Message::Ping(d))) => { let _ = ws_write.send(Message::Pong(d)).await; }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        eprintln!("\r\nWebSocket error: {e}");
                        break;
                    }
                    None => break,
                }
            }
        }
    }

    Ok(())
}

/// Build a WebSocket upgrade request with SigV4 Authorization header.
async fn build_signed_request(
    arn: &str,
    session_id: &str,
    shell_id: Option<&str>,
    region: &str,
) -> Result<http::Request<()>> {
    let config = aws_config::from_env()
        .region(aws_config::Region::new(region.to_string()))
        .load()
        .await;

    let creds = config
        .credentials_provider()
        .ok_or_else(|| anyhow::anyhow!("No AWS credentials"))?
        .provide_credentials()
        .await?;

    let identity = creds.into();

    let encoded_arn = urlencoding::encode(arn);
    let host = format!("bedrock-agentcore.{region}.amazonaws.com");
    let path = format!("/runtimes/{encoded_arn}/ws/shells");

    let mut query = format!("qualifier=DEFAULT&runtimeSessionId={}", urlencoding::encode(session_id));
    if let Some(sid) = shell_id {
        query.push_str(&format!("&shellId={sid}"));
    }

    let uri = format!("https://{host}{path}?{query}");

    // Sign with headers (not presigned URL)
    let mut settings = SigningSettings::default();
    settings.expires_in = None; // header signing, not presigned

    let signing_params = v4::SigningParams::builder()
        .identity(&identity)
        .region(region)
        .name("bedrock-agentcore")
        .time(SystemTime::now())
        .settings(settings)
        .build()?;

    let headers = [
        ("host", host.as_str()),
        ("x-amzn-bedrock-agentcore-runtime-session-id", session_id),
    ];

    let signable = SignableRequest::new(
        "GET",
        &uri,
        headers.into_iter(),
        SignableBody::empty(),
    )?;

    let (instructions, _sig) = sign(signable, &signing_params.into())?.into_parts();

    // Build the HTTP request with signed headers + WebSocket upgrade headers
    let wss_uri = format!("wss://{host}{path}?{query}");
    let mut builder = http::Request::builder()
        .method("GET")
        .uri(&wss_uri)
        .header("host", &host)
        .header("x-amzn-bedrock-agentcore-runtime-session-id", session_id)
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", tokio_tungstenite::tungstenite::handshake::client::generate_key());

    // Add SigV4 headers (Authorization, X-Amz-Date, etc.)
    for (name, value) in instructions.headers() {
        builder = builder.header(name, value);
    }

    Ok(builder.body(())?)
}

struct RawModeGuard;
impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        eprintln!();
    }
}
