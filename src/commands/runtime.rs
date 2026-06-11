use anyhow::{bail, Result};
use aws_sdk_bedrockagentcorecontrol as control;

use crate::config::{AliasStore, RuntimeSpec};
use crate::RuntimeAction;

pub async fn handle(action: RuntimeAction, region_override: Option<String>) -> Result<()> {
    match action {
        RuntimeAction::Apply { file } => apply(&file, region_override).await,
        RuntimeAction::Get { name } => get(&name, region_override).await,
        RuntimeAction::Export { name, file } => export(&name, file.as_deref(), region_override).await,
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

/// Resolve a name to a runtime ID: alias → ARN → runtime ID, or match by runtime name.
async fn resolve_runtime_id(name: &str, client: &control::Client) -> Result<String> {
    let store = AliasStore::load();
    let resolved = store.resolve(name);

    // If it's an ARN, extract the ID
    if resolved.starts_with("arn:") {
        return Ok(resolved.rsplit('/').next().unwrap_or(&resolved).to_string());
    }

    // Otherwise, try matching by runtime name
    let resp = client.list_agent_runtimes().send().await?;
    if let Some(rt) = resp.agent_runtimes().iter().find(|r| r.agent_runtime_name() == name) {
        return Ok(rt.agent_runtime_id().to_string());
    }

    // Last resort: assume it's a raw runtime ID
    Ok(resolved)
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

async fn export(name: &str, output: Option<&str>, region_override: Option<String>) -> Result<()> {
    let store = AliasStore::load();
    let arn = store.resolve(name);
    let cfg = crate::config::AgctlConfig::load();
    let region = Some(cfg.resolve_region(region_override.as_deref(), Some(&arn)));
    let client = make_client(region).await;

    let id = resolve_runtime_id(name, &client).await?;
    let rt = client.get_agent_runtime().agent_runtime_id(&id).send().await?;

    // Extract image from artifact
    let image = rt.agent_runtime_artifact()
        .and_then(|a| a.as_container_configuration().ok())
        .map(|c| c.container_uri().to_string())
        .unwrap_or_default();

    let role = rt.role_arn().to_string();

    let network = rt.network_configuration()
        .map(|n| format!("{:?}", n.network_mode()))
        .unwrap_or_else(|| "PUBLIC".into());

    // Build filesystem config
    let fs = rt.filesystem_configurations()
        .iter()
        .find_map(|f| {
            if let control::types::FilesystemConfiguration::SessionStorage(s) = f {
                Some(s.mount_path().to_string())
            } else {
                None
            }
        });

    // Build env vars
    let env: std::collections::HashMap<String, String> = rt.environment_variables()
        .map(|e| e.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();

    let spec = crate::config::RuntimeSpec {
        metadata: crate::config::Metadata {
            name: rt.agent_runtime_name().to_string(),
            region: extract_region(rt.agent_runtime_arn()),
        },
        spec: crate::config::Spec {
            image,
            role,
            network,
            filesystem: fs.map(|mount| crate::config::FilesystemConfig {
                session_storage: Some(mount),
            }),
            env,
        },
    };

    let yaml = serde_yaml::to_string(&spec)?;

    match output {
        Some(path) => {
            std::fs::write(path, &yaml)?;
            eprintln!("✅ Exported {} → {path}", rt.agent_runtime_name());
        }
        None => print!("{yaml}"),
    }

    Ok(())
}

async fn get(name: &str, region_override: Option<String>) -> Result<()> {
    let store = AliasStore::load();
    let arn = store.resolve(name);
    let cfg = crate::config::AgctlConfig::load();
    let region = Some(cfg.resolve_region(region_override.as_deref(), Some(&arn)));
    let client = make_client(region).await;

    let id = resolve_runtime_id(name, &client).await?;
    let rt = client.get_agent_runtime().agent_runtime_id(&id).send().await?;

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

    // Build reverse map: ARN → alias name
    let store = AliasStore::load();
    let reverse: std::collections::HashMap<&str, &str> = store
        .aliases
        .iter()
        .map(|(name, arn)| (arn.as_str(), name.as_str()))
        .collect();

    println!("{:<30} {:<10} {:<8} {}", "NAME", "STATUS", "VERSION", "ALIAS");
    for rt in resp.agent_runtimes() {
        let alias = reverse.get(rt.agent_runtime_arn()).copied().unwrap_or("");
        println!(
            "{:<30} {:<10} {:<8} {}",
            rt.agent_runtime_name(),
            format!("{:?}", rt.status()),
            rt.agent_runtime_version(),
            alias,
        );
    }
    Ok(())
}

async fn delete(name: &str, yes: bool, region_override: Option<String>) -> Result<()> {
    let store = AliasStore::load();
    let arn = store.resolve(name);
    let cfg = crate::config::AgctlConfig::load();
    let region = Some(cfg.resolve_region(region_override.as_deref(), Some(&arn)));
    let client = make_client(region).await;

    let id = resolve_runtime_id(name, &client).await?;

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
