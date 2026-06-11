# agctl

Just as [ecsctl](https://github.com/oablab/ecsctl), we love kubectl-style declarative administration. Managing AgentCore runtimes via raw AWS CLI commands with 10+ flags and JSON blobs is painful — so we built `agctl` that lets you declare your agent runtime in a YAML file and apply it with a single command.

## Usage

### Declarative runtime management

```yaml
# kiro.yaml
metadata:
  name: kiro_coding_agent
spec:
  image: <ACCOUNT>.dkr.ecr.us-east-1.amazonaws.com/agentcore-kiro:latest
  role: arn:aws:iam::<ACCOUNT>:role/agentcore-kiro-role
  network: PUBLIC
  filesystem:
    sessionStorage: /mnt/agent
  env:
    AWS_REGION: us-east-1
    HOME: /mnt/agent
```

```bash
agctl runtime apply -f kiro.yaml     # create or update
agctl runtime get kiro               # show status
agctl runtime list                   # list all runtimes
agctl runtime delete kiro            # teardown
agctl runtime restart kiro           # delete + recreate
```

### Execute commands in a running session

```bash
agctl exec kiro "echo hello"
agctl exec kiro "/app/run.sh chat 'what is 2+2?'"
agctl exec kiro --session-id my-session-00000000000000 "whoami"
```

### Aliases

```bash
agctl alias set kiro arn:aws:bedrock-agentcore:us-east-1:<ACCOUNT>:runtime/kiro_coding_agent-ABC123
agctl alias list
agctl alias remove kiro
```

Aliases are stored in `~/.config/agctl/aliases.json`. Auto-set on `runtime apply`.

### Shorthand: `agcrt` / `agcsh`

Add to `~/.zshrc` (or `~/.bashrc`):

```bash
alias agcrt='agctl runtime'
agcsh() {
  if [ $# -gt 1 ]; then
    agctl exec "$@"
  else
    agctl exec --it "$1" --session-id "agcsh-${1}-$(date +%s)00000000000000"
  fi
}
```

Then:

```bash
source ~/.zshrc   # reload

agcrt list                 # agctl runtime list
agcrt apply -f kiro.yaml   # agctl runtime apply -f kiro.yaml
agcsh kiro                 # interactive PTY shell
agcsh kiro "whoami"        # one-shot command
```

> **Note:** Interactive PTY (`agctl exec --it`) is planned for v0.2.0 via WebSocket shell API.

## How it works

```
agctl runtime apply -f kiro.yaml
  → parse YAML spec
  → check if runtime exists (by name)
  → create or update via bedrock-agentcore-control API
  → auto-set alias (name → ARN)

agctl exec kiro "command"
  → resolve alias → ARN
  → extract region from ARN
  → invoke_agent_runtime_command (streaming stdout/stderr)
```

## Requirements

- AWS credentials configured (`~/.aws/credentials`, env vars, or IAM role)
- Permissions: `bedrock-agentcore:*` (or scoped to specific runtime ARNs)

## Install

```bash
# macOS (Apple Silicon)
curl -fsSL https://github.com/oablab/agctl/releases/latest/download/agctl-darwin-arm64.tar.gz | tar xz -C ~/.local/bin

# Linux (arm64)
curl -fsSL https://github.com/oablab/agctl/releases/latest/download/agctl-linux-arm64.tar.gz | tar xz -C ~/.local/bin

# Linux (amd64)
curl -fsSL https://github.com/oablab/agctl/releases/latest/download/agctl-linux-amd64.tar.gz | tar xz -C ~/.local/bin
```

## Known Limitations

- **Runtime env vars don't propagate to interactive shells.** Env vars set via `spec.env` (or `update_agent_runtime`) only apply to PID 1 (container entrypoint). WebSocket PTY shells spawn a separate bash process that doesn't inherit them. Workaround: `eval $(cat /proc/1/environ | tr '\0' '\n' | grep -v ^HOME= | sed 's/^/export /')` inside the shell.
- **Interactive shell requires PTY.** `agctl exec --it` must be run from a real terminal — pipes and scripts won't work.
- **Session ID must be ≥33 characters.** AgentCore rejects shorter values.

## License

MIT
