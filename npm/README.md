# anveesa

A fast, multi-provider terminal AI coding assistant — interactive TUI, one-shot prompts, or web UI — backed by any OpenAI-compatible API.

[![CI](https://github.com/PandhuWibowo/anveesa-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/PandhuWibowo/anveesa-cli/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/anveesa.svg)](https://crates.io/crates/anveesa)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

---

## What it is

Anveesa is a multi-provider terminal AI assistant that works with any OpenAI-compatible API — including OpenAI, Anthropic (via proxy), DeepSeek, Groq, Gemini, OpenRouter, local Ollama, and more. It ships a full TUI with streaming output, diff previews, cost tracking, and plan display, plus a lightweight web UI for browser-based access. You get 28 built-in tools covering file ops, git, web search, screenshot capture, and command execution — all with an approval flow that shows a full diff before any write.

---

## Install

**Cargo (recommended):**

```bash
cargo install anveesa
```

**Pre-built binaries:**

Download from [GitHub Releases](https://github.com/PandhuWibowo/anveesa-cli/releases) for your platform (macOS, Linux x86_64/arm64).

**npm wrapper:**

```bash
npm install -g anveesa
```

**Build from source:**

```bash
git clone https://github.com/PandhuWibowo/anveesa-cli
cd anveesa-cli
cargo build --release
./target/release/anveesa
```

Requires Rust 1.85+ (2024 edition).

---

## Quick start

```bash
# 1. Initialize config at ~/.config/anveesa/config.toml
anveesa config init

# 2. Edit config to set your provider and API key
#    (see sample below)

# 3. Set your API key in the environment
export OPENAI_API_KEY="sk-..."

# 4. Launch the TUI
anveesa

# 5. One-shot prompt
anveesa "explain the auth middleware in this repo"

# 6. Specific provider/model
anveesa --provider deepseek --model deepseek-chat "refactor src/auth.rs"
```

**Sample config snippet** (`~/.config/anveesa/config.toml`):

```toml
default_provider = "openai"

[providers.openai]
kind = "openai-compatible"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
default_model = "gpt-4o"
fast_model = "gpt-4o-mini"

[providers.deepseek]
kind = "openai-compatible"
base_url = "https://api.deepseek.com"
api_key_env = "DEEPSEEK_API_KEY"
default_model = "deepseek-chat"

[providers.ollama]
kind = "openai-compatible"
base_url = "http://localhost:11434/v1"
default_model = "qwen2.5-coder:7b"
```

---

## Providers

| Provider | kind | Notes |
|---|---|---|
| OpenAI | `openai-compatible` | GPT-4o, o1, o3 |
| Anthropic (via proxy) | `openai-compatible` | Claude 3.5/3.7 via OpenRouter or direct |
| DeepSeek | `openai-compatible` | deepseek-chat, deepseek-reasoner |
| Groq | `openai-compatible` | Ultra-fast inference |
| Google Gemini | `openai-compatible` | gemini-1.5-pro/flash |
| OpenRouter | `openai-compatible` | 200+ models via one API |
| Mistral | `openai-compatible` | mistral-large, codestral |
| xAI (Grok) | `openai-compatible` | grok-2 |
| Together AI | `openai-compatible` | Open-source models |
| Fireworks AI | `openai-compatible` | Fast open-source |
| Cerebras | `openai-compatible` | High-throughput |
| SambaNova | `openai-compatible` | Enterprise inference |
| NVIDIA NIM | `openai-compatible` | GPU-backed cloud |
| GitHub Models | `openai-compatible` | Free tier models |
| Perplexity | `openai-compatible` | Web-grounded answers |
| Ollama | `openai-compatible` | Local models |
| LM Studio | `openai-compatible` | Local GUI + API |
| vLLM | `openai-compatible` | Self-hosted inference |
| LiteLLM | `openai-compatible` | Universal proxy |
| LocalAI | `openai-compatible` | Self-hosted |
| Claude Code | `command` | Runs `claude` CLI |
| Codex CLI | `command` | Runs `codex` CLI |
| GitHub Copilot | `command` | Runs `copilot` CLI |

---

## TUI commands

| Command | Description |
|---|---|
| `/add <file>` | Inject a file into context (Aider-style) |
| `/diff` | Show `git diff HEAD` |
| `/commit [msg]` | Stage all + commit; omit msg to have AI generate one |
| `/memory <note>` | Append a note to `.anveesa.md` |
| `/clear` | Reset conversation and session |
| `/undo` | Restore last file changed by AI |
| `/compact` | Drop old turns to free context window |
| `/copy` | Copy last response to clipboard |
| `/export [path]` | Save conversation as Markdown |
| `/model [name]` | Show or switch model |
| `/provider [name]` | Show or switch provider |
| `/status` | Token usage and cost summary |
| `/search` | Search conversation (or Ctrl+R) |
| `/help` | Show all shortcuts |
| `/exit` | Quit |

**TUI keyboard shortcuts:**

| Key | Action |
|---|---|
| Enter | Submit prompt |
| Shift+Enter | Newline in input |
| Tab | Complete `/command`, provider name, or file path |
| Ctrl+R | Search conversation |
| `[` / `]` | Navigate between file diffs and thinking blocks |
| Enter (on diff) | Expand / collapse |
| Ctrl+V / Cmd+V | Paste image or text |
| j / k | Scroll (when input is empty) |
| PageUp / PageDn | Scroll |
| Ctrl+W | Delete word |
| Ctrl+U | Clear line |
| Ctrl+M | Toggle mouse capture (scroll vs. select mode) |
| Ctrl+C | Cancel / quit |

---

## Web UI

```bash
anveesa web           # starts on http://localhost:8374
anveesa web --port 8080
```

The web UI provides a browser-based chat interface with the same provider/model selection as the TUI. Features:

- Streaming responses with Markdown rendering
- Multi-provider switching
- Session history
- Cost and token tracking
- Mobile-friendly layout

---

## Key features

- **Multi-provider** — 28 built-in providers; add any OpenAI-compatible endpoint in one config line
- **Cost tracking** — per-turn and session cost displayed in the header; custom `pricing` override per provider
- **Session persistence** — conversations auto-saved per working directory; resume where you left off
- **Extended thinking** — enable Anthropic extended thinking with `extended_thinking = 10000` budget tokens
- **Image attachments** — paste screenshots with Cmd+V / Ctrl+V; queue multiple images per turn
- **MCP support** — connect Model Context Protocol servers for additional tools
- **Aider-style /add** — inject files into context without leaving the TUI
- **Auto-compact** — smart context compaction kicks in when the window fills up; AI summarises dropped turns
- **Auto-commit helper** — `/commit` stages all changes and pre-fills the input with an AI-generated commit message
- **Model routing** — `fast_model` for cheap read-only tool rounds; main model for final synthesis
- **28 built-in tools** — file ops, git, web search, deep-fetch, screenshot, notes, run commands
- **Approval flow** — every write/run tool shows a diff preview before executing

---

## Config reference

All fields of `OpenAiCompatibleProviderConfig`:

| Field | Type | Default | Description |
|---|---|---|---|
| `base_url` | string | required | API base URL (e.g. `https://api.openai.com/v1`) |
| `api_key` | string | — | Inline API key (prefer `api_key_env`) |
| `api_key_env` | string | — | Environment variable holding the API key |
| `default_model` | string | — | Model to use when none is specified |
| `fast_model` | string | — | Lightweight model for read-only tool rounds |
| `headers` | table | `{}` | Extra HTTP headers (e.g. for GitHub Models) |
| `prompt_cache` | bool | — | Enable prompt caching (Anthropic cache_control markers) |
| `max_tokens` | int | — | Upper bound on tokens generated per response |
| `extended_thinking` | int | — | Enable Anthropic extended thinking; value = budget_tokens |
| `pricing` | [f64; 4] | — | Custom pricing `[input, output, cache_read, cache_write]` per million tokens |

**Example with custom pricing:**

```toml
[providers.my-provider]
kind = "openai-compatible"
base_url = "https://my-llm-api.com/v1"
api_key_env = "MY_API_KEY"
default_model = "my-model-v1"
pricing = [3.0, 15.0, 0.3, 3.75]
```

---

## License

MIT — see [LICENSE](LICENSE).
