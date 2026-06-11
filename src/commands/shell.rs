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

    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();

    // Send initial terminal size
    if let Ok((cols, rows)) = terminal::size() {
        let resize = format!("{{\"type\":\"resize\",\"cols\":{cols},\"rows\":{rows}}}");
        let _ = ws_write.send(Message::Binary(resize.into_bytes().into())).await;
    }

    loop {
        let mut buf = [0u8; 4096];
        tokio::select! {
            n = stdin.read(&mut buf) => {
                match n {
                    Ok(0) => break,
                    Ok(n) => {
                        if buf[..n].contains(&0x1d) { // Ctrl+]
                            eprintln!("\r\nDetached.");
                            break;
                        }
                        ws_write.send(Message::Binary(buf[..n].to_vec().into())).await?;
                    }
                    Err(_) => break,
                }
            }
            msg = ws_read.next() => {
                match msg {
                    Some(Ok(Message::Binary(data))) => {
                        stdout.write_all(&data).await?;
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
                    Some(Err(_)) | None => break,
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

    let mut query = String::from("qualifier=DEFAULT");
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
