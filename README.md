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

Add to your shell profile (`~/.zshrc` or `~/.bashrc`):

```bash
alias agcrt='agctl runtime'
agcsh() { agctl exec "$1" bash; }
```

Then:

```bash
agcrt list                 # agctl runtime list
agcrt apply -f kiro.yaml   # agctl runtime apply -f kiro.yaml
agcsh kiro                 # agctl exec kiro bash (interactive shell)
```

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

## License

MIT
