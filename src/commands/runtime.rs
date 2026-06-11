use anyhow::{bail, Result};
use aws_sdk_bedrockagentcorecontrol as control;

use crate::config::{AliasStore, RuntimeSpec};
use crate::RuntimeAction;

pub async fn handle(action: RuntimeAction, region_override: Option<String>) -> Result<()> {
    match action {
        RuntimeAction::Apply { file } => apply(&file, region_override).await,
        RuntimeAction::Get { name } => get(&name, region_override).await,
        RuntimeAction::List => list(region_override).await,
        RuntimeAction::Delete { name, yes } => delete(&name, yes, region_override).await,
        RuntimeAction::Restart { name } => restart(&name, region_override).await,
    }
}

async fn make_client(region: Option<String>) -> control::Client {
    let mut config = aws_config::from_env();
    if let Some(r) = region {
        config = config.region(aws_config::Region::new(r));
    }
    control::Client::new(&config.load().await)
}

async fn apply(file: &str, region_override: Option<String>) -> Result<()> {
    let spec = RuntimeSpec::from_file(file)?;
    let region = region_override.unwrap_or_else(|| spec.region());
    let client = make_client(Some(region.clone())).await;

    // Check if runtime exists by name
    let existing = client.list_agent_runtimes().send().await?;
    let found = existing.agent_runtimes().iter().find(|r| {
        r.agent_runtime_name() == spec.metadata.name
    });

    let mut fs_configs = Vec::new();
    if let Some(ref fs) = spec.spec.filesystem {
        if let Some(ref mount) = fs.session_storage {
            fs_configs.push(
                control::types::FilesystemConfiguration::SessionStorage(
                    control::types::SessionStorageConfiguration::builder()
                        .mount_path(mount)
                        .build()?,
                ),
            );
        }
    }

    if let Some(rt) = found {
        // Update
        let id = rt.agent_runtime_id();
        println!("Updating runtime '{}'...", spec.metadata.name);
        let mut req = client.update_agent_runtime()
            .agent_runtime_id(id)
            .agent_runtime_artifact(
                control::types::AgentRuntimeArtifact::ContainerConfiguration(
                    control::types::ContainerConfiguration::builder()
                        .container_uri(&spec.spec.image)
                        .build()?,
                ),
            )
            .role_arn(&spec.spec.role)
            .network_configuration(
                control::types::NetworkConfiguration::builder()
                    .network_mode(spec.spec.network.parse().unwrap_or(control::types::NetworkMode::Public))
                    .build()?,
            );

        for fc in &fs_configs {
            req = req.filesystem_configurations(fc.clone());
        }
        for (k, v) in &spec.spec.env {
            req = req.environment_variables(k.clone(), v.clone());
        }

        req.send().await?;
        println!("✅ Runtime '{}' updated", spec.metadata.name);
    } else {
        // Create
        println!("Creating runtime '{}'...", spec.metadata.name);
        let mut req = client.create_agent_runtime()
            .agent_runtime_name(&spec.metadata.name)
            .agent_runtime_artifact(
                control::types::AgentRuntimeArtifact::ContainerConfiguration(
                    control::types::ContainerConfiguration::builder()
                        .container_uri(&spec.spec.image)
                        .build()?,
                ),
            )
            .role_arn(&spec.spec.role)
            .network_configuration(
                control::types::NetworkConfiguration::builder()
                    .network_mode(spec.spec.network.parse().unwrap_or(control::types::NetworkMode::Public))
                    .build()?,
            )
            .protocol_configuration(
                control::types::ProtocolConfiguration::builder()
                    .server_protocol(control::types::ServerProtocol::Http)
                    .build()?,
            );

        for fc in &fs_configs {
            req = req.filesystem_configurations(fc.clone());
        }
        for (k, v) in &spec.spec.env {
            req = req.environment_variables(k.clone(), v.clone());
        }

        let resp = req.send().await?;
        let arn = resp.agent_runtime_arn();
        println!("✅ Runtime '{}' created: {}", spec.metadata.name, arn);

        // Auto-set alias
        let mut store = AliasStore::load();
        store.aliases.insert(spec.metadata.name.clone(), arn.to_string());
        store.save()?;
        println!("   Alias set: {} → {}", spec.metadata.name, arn);
    }

    Ok(())
}

async fn get(name: &str, region_override: Option<String>) -> Result<()> {
    let store = AliasStore::load();
    let arn = store.resolve(name);
    let region = region_override.or_else(|| extract_region(&arn));
    let client = make_client(region).await;

    // Extract ID from ARN
    let id = arn.rsplit('/').next().unwrap_or(&arn);
    let rt = client.get_agent_runtime().agent_runtime_id(id).send().await?;

    println!("Name:    {}", rt.agent_runtime_name());
    println!("ID:      {}", rt.agent_runtime_id());
    println!("ARN:     {}", rt.agent_runtime_arn());
    println!("Status:  {:?}", rt.status());
    println!("Version: {}", rt.agent_runtime_version());
    println!("Image:   {:?}", rt.agent_runtime_artifact());
    if !rt.filesystem_configurations().is_empty() {
        println!("FS:      {:?}", rt.filesystem_configurations());
    }
    Ok(())
}

async fn list(region_override: Option<String>) -> Result<()> {
    let client = make_client(region_override).await;
    let resp = client.list_agent_runtimes().send().await?;

    if resp.agent_runtimes().is_empty() {
        println!("No runtimes found.");
        return Ok(());
    }

    println!("{:<30} {:<10} {:<8}", "NAME", "STATUS", "VERSION");
    for rt in resp.agent_runtimes() {
        println!(
            "{:<30} {:<10} {:<8}",
            rt.agent_runtime_name(),
            format!("{:?}", rt.status()),
            rt.agent_runtime_version(),
        );
    }
    Ok(())
}

async fn delete(name: &str, yes: bool, region_override: Option<String>) -> Result<()> {
    let store = AliasStore::load();
    let arn = store.resolve(name);
    let region = region_override.or_else(|| extract_region(&arn));
    let client = make_client(region).await;

    let id = arn.rsplit('/').next().unwrap_or(&arn);

    if !yes {
        eprint!("Delete runtime '{name}' ({id})? [y/N] ");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            bail!("aborted");
        }
    }

    client.delete_agent_runtime().agent_runtime_id(id).send().await?;
    println!("✅ Runtime '{name}' deleted");
    Ok(())
}

async fn restart(name: &str, region_override: Option<String>) -> Result<()> {
    println!("Restarting '{name}' (delete + recreate)...");
    // TODO: save spec before delete, then re-apply
    bail!("restart not yet implemented — use `agctl runtime delete` + `agctl runtime apply`");
}

fn extract_region(arn: &str) -> Option<String> {
    let parts: Vec<&str> = arn.split(':').collect();
    if parts.len() >= 4 && !parts[3].is_empty() {
        Some(parts[3].to_string())
    } else {
        None
    }
}
