# anveesa

A fast, terminal-native AI coding assistant. One command — interactive TUI or one-shot prompts — backed by any OpenAI-compatible provider.

```
npm install -g anveesa
anveesa
```

---

## Features

- **Full TUI** — streaming output, diff previews on file edits, cost tracking, plan display
- **28 built-in tools** — file ops, git, web search, deep-fetch, screenshot, notes, run commands
- **Multi-provider** — Claude, GPT-4o, Gemini, DeepSeek, local Ollama, any OpenAI-compatible API
- **Model routing** — `fast_model` for read-only tool rounds, main model for synthesis
- **Parallel tools** — read-only tool calls run concurrently; write tools stay sequential for approval
- **Approval flow** — every write/run tool shows a full diff preview; approve once, for the turn, or deny
- **Multi-image paste** — ⌘V (macOS) / Ctrl+V to queue multiple clipboard images per turn
- **Project memory** — `.anveesa.md` in your repo root is auto-injected into every session
- **Path sandboxing** — write tools blocked outside the git root by default
- **Conversation search** — Ctrl+R to search through all messages

---

## Install

```bash
# npm (recommended — downloads prebuilt binary)
npm install -g anveesa

# Cargo (builds from source)
cargo install --path .
```

---

## Quick start

```bash
# 1. Initialize config
anveesa config init

# 2. Set your API key
export ANTHROPIC_API_KEY="sk-ant-..."
export OPENAI_API_KEY="sk-..."

# 3. Launch the TUI
anveesa

# 4. One-shot prompt
anveesa "explain the auth middleware in this repo"

# 5. Specific provider/model
anveesa --provider anthropic --model claude-sonnet-4-6 "refactor src/auth.ts"
```

---

## Configuration

Config lives at `~/.config/anveesa/config.toml`.

```toml
default_provider = "anthropic"

[providers.anthropic]
api_base      = "https://api.anthropic.com/v1"
api_key_env   = "ANTHROPIC_API_KEY"
default_model = "claude-sonnet-4-6"
fast_model    = "claude-haiku-4-5-20251001"  # cheap model for read-only tool rounds
prompt_cache  = true

[providers.openai]
api_base      = "https://api.openai.com/v1"
api_key_env   = "OPENAI_API_KEY"
default_model = "gpt-4o"
fast_model    = "gpt-4o-mini"

[providers.local]
api_base      = "http://localhost:11434/v1"
default_model = "qwen2.5-coder:7b"
```

### Project instructions

Drop `.anveesa.md` in your repo root — auto-injected every session:

```markdown
# My Project

Stack: React 18, TypeScript, Tailwind, Prisma, PostgreSQL

Rules:
- Use named exports only
- Tests go in __tests__/ next to the source file
- Run `pnpm test` before committing
- DB migrations live in db/migrations/
```

---

## TUI shortcuts

| Key | Action |
|---|---|
| Enter | Submit prompt |
| Shift+Enter | Newline in input |
| ↑ / ↓ | Navigate input history |
| Tab | Complete `/command` or file path |
| Ctrl+R | Search conversation |
| `[` / `]` | Navigate file diffs |
| Enter (on diff) | Expand / collapse diff |
| ⌘V / Ctrl+V | Paste image or text (repeat to queue multiple) |
| j / k | Scroll (empty input) |
| PageUp / PageDn | Scroll |
| Ctrl+W | Delete word |
| Ctrl+U | Clear line |
| Ctrl+C | Cancel / quit |

---

## Slash commands

| Command | Description |
|---|---|
| `/help` | Show all shortcuts |
| `/clear` | Reset conversation |
| `/compact` | Drop old turns to free context |
| `/search` | Search conversation (or Ctrl+R) |
| `/undo` | Restore last AI-modified file |
| `/copy` | Copy last response to clipboard |
| `/export [path]` | Save as Markdown |
| `/model [name]` | Switch model |
| `/provider [name]` | Switch provider |
| `/status` | Token and cost info |
| `/exit` | Quit |

---

## Tools

**File ops:** `read_file` `write_file` `edit_file` `patch_file` `delete_file` `move_file` `copy_file` `create_dir` `list_dir` `find_files` `search_text`

**Git:** `git_status` `git_diff` `git_log` `git_blame` `git_show` `git_commit` `git_stash` `git_branch`

**Web:** `web_search` `fetch_url` `screenshot_url`

**Notes:** `save_note` `read_notes` `search_notes` `delete_note`

**Execution:** `run_command`

### fetch_url modes

```
fetch_url(url="...", mode="text")   # default — plain text, HTML stripped
fetch_url(url="...", mode="raw")    # full HTML source
fetch_url(url="...", mode="deep")   # HTML + all linked CSS (+ JS if include_js=true)
```

### screenshot_url

```
screenshot_url(url="https://localhost:3000", full_page=true)
```

Requires Playwright: `npm install -g playwright && npx playwright install chromium`

---

## Security

- **Approval flow** — file writes and commands show a diff preview before running; `--yes` to auto-approve
- **Path sandboxing** — writes outside the git root are refused
- **Dangerous commands** — `rm -rf /`, pipe-to-shell, and similar patterns are hard-blocked
- **Secret guard** — model is instructed never to expose API keys, `.env` files, or SSH keys

---

## Environment variables

| Variable | Purpose |
|---|---|
| `ANTHROPIC_API_KEY` | Anthropic |
| `OPENAI_API_KEY` | OpenAI |
| `GEMINI_API_KEY` | Google Gemini |
| `BRAVE_SEARCH_API_KEY` | Better web search |
| `SERPER_API_KEY` | Alternative web search |
| `ANVEESA_MAX_TOOL_ROUNDS` | Override tool round limit (default 32) |

---

## Build from source

```bash
git clone https://github.com/PandhuWibowo/anveesa-cli
cd anveesa-cli
cargo build --release
./target/release/anveesa
```

Requires Rust 1.85+ (2024 edition).

---

MIT License
