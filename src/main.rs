mod commands;
mod config;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "agctl", about = "Declarative CLI for Amazon Bedrock AgentCore Runtime")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// AWS region (default: from spec or us-east-1)
    #[arg(long, global = true)]
    region: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Manage agent runtimes
    Runtime {
        #[command(subcommand)]
        action: RuntimeAction,
    },
    /// Execute commands in a running session
    Exec {
        /// Runtime name or alias
        runtime: String,
        /// Command to run (omit for interactive shell)
        #[arg(trailing_var_arg = true)]
        command: Vec<String>,
        /// Session ID (auto-generated if omitted)
        #[arg(long)]
        session_id: Option<String>,
        /// Interactive PTY
        #[arg(long)]
        it: bool,
    },
    /// Manage aliases
    Alias {
        #[command(subcommand)]
        action: AliasAction,
    },
}

#[derive(Subcommand)]
enum RuntimeAction {
    /// Create or update a runtime from a spec file
    Apply {
        /// Path to YAML spec file
        #[arg(short, long)]
        file: String,
    },
    /// Get runtime status
    Get {
        /// Runtime name or alias
        name: String,
    },
    /// List all runtimes
    List,
    /// Delete a runtime
    Delete {
        /// Runtime name or alias
        name: String,
        /// Skip confirmation
        #[arg(short, long)]
        yes: bool,
    },
    /// Delete and recreate (force new image pull)
    Restart {
        /// Runtime name or alias
        name: String,
    },
}

#[derive(Subcommand)]
enum AliasAction {
    /// Set an alias
    Set {
        /// Alias name
        name: String,
        /// Runtime ARN
        arn: String,
    },
    /// List all aliases
    List,
    /// Remove an alias
    Remove {
        /// Alias name
        name: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Runtime { action } => commands::runtime::handle(action, cli.region).await,
        Commands::Exec { runtime, command, session_id, it } => {
            commands::exec::handle(runtime, command, session_id, it, cli.region).await
        }
        Commands::Alias { action } => commands::alias::handle(action),
    }
}
