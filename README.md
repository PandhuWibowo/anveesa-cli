# Anveesa

Anveesa is a Rust terminal wrapper for AI providers. It gives you one command,
`anveesa`, while each provider is configured as either:

- `openai-compatible`: HTTP chat completions providers such as OpenRouter and other compatible gateways.
- `command`: local CLIs such as Codex, Copilot, and Claude Code, where Anveesa spawns a command and passes the prompt.

## Install locally

```sh
cargo install --path . --force
```

## Quick start

```sh
anveesa config init
export SUMOPOD_API_KEY="..."
anveesa config set-model "your-sumopod-model"
anveesa
```

Running `anveesa` with no prompt opens an interactive prompt for the default provider:

```text
anveesa | provider: sumopod | model: kimi-k2.6
state   | turns:0 | ctx:on | tools:on | writes:ask | memory:new
commands| /clear reset | /exit quit
approve | y once | a all for current turn | enter no

anveesa[0]>
```

Interactive mode keeps running after each answer. It also keeps the conversation
context for the same provider/model/system in the same working directory, even
after restarting Anveesa. Use `/clear` to reset that context and `/exit` to
return to the shell.

The prompt has full line editing, and your input history is remembered across
sessions (stored next to the config as `history`). The active conversation is
stored next to it as `session.json`. Use the up/down arrows to recall previous
prompts.

`ctx:on` means Anveesa sends lightweight terminal context with each request:
current directory, parent directory, git root/branch/status when available, and
a capped directory listing. This lets the model answer questions like "where are
you?" using the terminal workspace instead of guessing.

`tools:on` means OpenAI-compatible providers can ask Anveesa to inspect the
workspace: list directories, find files by name, search text, read capped file
snippets, and do a basic web lookup. The tools can inspect paths outside the
current project, but obvious secret files such as SSH keys and `.env` files are
blocked.

`writes:ask` covers the workspace-modifying tools — `create_dir`, `write_file`,
`edit_file`, and `run_command`. By default Anveesa asks for confirmation on the
terminal before each one:

```text
allow run command `cargo test`? [y]es/[a]ll this turn/[N]o
```

Answer `a` to approve the remaining write/run tools for the current assistant
turn, which is useful when scaffolding several files.

The indicator reflects the active policy: `writes:ask` (confirm each action,
the interactive default and the default for one-shot prompts typed directly in a
terminal), `writes:auto` (run without asking, enabled with `--yes`), or
`writes:off` (disabled for non-interactive stdin runs unless `--yes` is passed).

Responses stream token-by-token as the model generates them. While Anveesa waits
for the first token it shows a small status line such as:

```text
- thinking... 2s
```

When usage is reported by the provider, a token summary is printed to stderr
after the answer:

```text
[tokens: 812 in / 144 out / 956 total]
```

OpenAI-compatible providers can use up to 32 tool rounds per answer by default.
After that, Anveesa stops advertising tools and asks the model to produce a
final answer from the gathered results. Override the cap with
`ANVEESA_MAX_TOOL_ROUNDS`, up to 256.

Use GLM/Z.ai:

```sh
export ZAI_API_KEY="..."
anveesa config set-provider glm
anveesa config set-model "glm-5.1"
anveesa "write a rust module outline"
```

You can also use the default `ask` behavior:

```sh
anveesa "write a git commit message"
```

Pipe stdin into a prompt:

```sh
git diff | anveesa ask --stdin "review this diff"
```

Let the model make changes. In interactive mode and one-shot terminal prompts,
it asks before each write or command. Pass `--yes` (`-y`) to allow file writes
and command execution without prompting:

```sh
anveesa --provider sumopod --yes "add a Default impl for the Config struct"
```

Run through Claude Code if the `claude` CLI is installed:

```sh
anveesa --provider claude-code "summarize this project"
```

Run through Codex if the `codex` CLI is installed:

```sh
anveesa --provider codex --model "gpt-5.1-codex" "review this repository"
```

Run through GitHub Copilot CLI if the `copilot` CLI is installed:

```sh
anveesa --provider copilot --model "gpt-5.1" "explain this function"
```

Use Sumopod with its OpenAI-compatible API:

```sh
export SUMOPOD_API_KEY="..."
anveesa --provider sumopod --model "your-sumopod-model" "explain this error"
```

## Built-in providers

OpenAI-compatible API providers:

- `openai`
- `sumopod`
- `openrouter`
- `glm`
- `glm-coding`
- `deepseek`
- `gemini`
- `github-models`
- `groq`
- `mistral`
- `xai`
- `together`
- `fireworks`
- `cerebras`
- `sambanova`
- `nvidia`
- `dashscope`
- `qwen`
- `huggingface`
- `vercel-ai-gateway`
- `perplexity`
- `ollama`
- `lm-studio`
- `vllm`
- `litellm`
- `localai`

Terminal command providers:

- `claude-code`
- `codex`
- `copilot`

Check the effective list any time:

```sh
anveesa providers
```

## Config

Default path:

```sh
anveesa config path
```

The path can be overridden with `ANVEESA_CONFIG`.

Set defaults once:

```sh
anveesa config set-provider sumopod
anveesa config set-model "kimi-k2.6"
```

After that, just run:

```sh
anveesa
```

Example provider config:

```toml
default_provider = "sumopod"

[providers.sumopod]
kind = "openai-compatible"
base_url = "https://ai.sumopod.com/v1"
api_key_env = "SUMOPOD_API_KEY"
default_model = "your-sumopod-model"

[providers.openrouter]
kind = "openai-compatible"
base_url = "https://openrouter.ai/api/v1"
api_key_env = "OPENROUTER_API_KEY"

[providers.glm]
kind = "openai-compatible"
base_url = "https://api.z.ai/api/paas/v4"
api_key_env = "ZAI_API_KEY"
default_model = "glm-5.1"

[providers.codex]
kind = "command"
command = "codex"
args = ["exec", "{model_args}", "{prompt}"]
model_args = ["--model", "{model}"]

[providers.claude-code]
kind = "command"
command = "claude"
args = ["-p", "{system_args}", "{model_args}", "{prompt}"]
model_args = ["--model", "{model}"]
system_args = ["--system-prompt", "{system}"]
```

Command providers can use placeholders in args or env values:

- `{prompt}`
- `{model}`
- `{system}`
- `{model_args}`
- `{system_args}`
