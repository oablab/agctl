use anyhow::{bail, Result};
use aws_sdk_bedrockagentcore as agentcore;
use crate::config::AliasStore;

pub async fn handle(
    runtime: String,
    command: Vec<String>,
    session_id: Option<String>,
    _it: bool,
    region_override: Option<String>,
) -> Result<()> {
    let store = AliasStore::load();
    let arn = store.resolve(&runtime);
    let region = region_override.or_else(|| extract_region(&arn));

    let mut config = aws_config::from_env();
    if let Some(r) = region {
        config = config.region(aws_config::Region::new(r));
    }
    let client = agentcore::Client::new(&config.load().await);

    let sid = session_id.unwrap_or_else(|| {
        format!("agctl-session-{:0>33}", std::process::id())
    });

    if command.is_empty() {
        bail!("Interactive PTY (--it) not yet supported. Use: agctl exec <runtime> <command>");
    }

    let cmd = command.join(" ");
    let body = agentcore::types::InvokeAgentRuntimeCommandRequestBody::builder()
        .command(&cmd)
        .timeout(300)
        .build()?;

    let mut output = client.invoke_agent_runtime_command()
        .agent_runtime_arn(&arn)
        .runtime_session_id(&sid)
        .body(body)
        .send()
        .await?;

    while let Some(event) = output.stream.recv().await? {
        if let agentcore::types::InvokeAgentRuntimeCommandStreamOutput::Chunk(chunk) = event {
            if let Some(ref delta) = chunk.content_delta {
                if let Some(ref s) = delta.stdout {
                    print!("{s}");
                }
                if let Some(ref s) = delta.stderr {
                    eprint!("{s}");
                }
            }
        }
    }

    Ok(())
}

fn extract_region(arn: &str) -> Option<String> {
    let parts: Vec<&str> = arn.split(':').collect();
    if parts.len() >= 4 && !parts[3].is_empty() {
        Some(parts[3].to_string())
    } else {
        None
    }
}
