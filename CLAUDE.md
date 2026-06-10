# anveesa-cli — Claude Code context

## What this is

A multi-provider terminal AI assistant written in Rust (edition 2024). Ships a full TUI, a browser web UI, and a one-shot CLI mode. Every AI provider that speaks the OpenAI chat/completions API works out of the box.

**Version:** 0.7.5 | **Tests:** 246 passing (237 unit + 9 doc) + 55 provider edge-case tests | **Warnings:** 0

## Module map

```
src/
  lib.rs              — CLI dispatch, run_interactive (plain REPL), ask_streaming
  main.rs             — binary entry point (calls lib::run_anveesa)
  cli.rs              — Clap structs: Cli, Command, AskOptions, WebArgs
  config.rs           — AppConfig, OpenAiCompatibleProviderConfig, CommandProviderConfig
  display.rs          — REPL (non-TUI) terminal output, render_stream, print_* helpers
  image.rs            — clipboard image read (macOS), load_image_file, parse_attach_command
  mcp.rs              — Model Context Protocol client (McpManager)
  prompt.rs           — PromptBuffer, RawPromptMode, raw line-reader for the plain REPL
  session.rs          — InteractiveSession, save/load/purge, format_session_age
  tools.rs            — all 32 tool implementations + definitions() + approval
  tools_scenarios.rs  — exhaustive scenario-based tests for tools
  web.rs              — axum HTTP server (GET /, GET /api/info, POST /api/ask SSE)
  web_ui.html         — embedded single-file chat UI (include_str! at compile time)
  workspace.rs        — workspace_context_for: git status, repo map, README inject

  provider/
    mod.rs            — StreamEvent, TurnResult, PromptRequest, Usage, ChatMessage
    openai_compatible.rs — SSE streaming, tool dispatch, retry, extended thinking
    openai_compatible_tests.rs — Edge-case provider tests (55 tests: SSE parsing, truncation, unicode)
    command.rs        — shell-command provider (wraps claude/codex/copilot CLIs)

  tui/                — ratatui TUI (declared as mod inside tui.rs)
    (tui.rs is the parent — App struct + event loop + type definitions)
    commands.rs       — handle_slash_command (/add /diff /commit /memory ...)
    format.rs         — wrap_text, format_assistant_lines, cursor helpers
    input.rs          — tab completion, update_search, msg_text
    render.rs         — render_header/messages/input/status, model_pricing (with incremental cache)
    stream.rs         — submit_prompt, handle_stream_event, turn management
```

## App struct sub-structs (tui.rs)

`App` groups state into four sub-structs to avoid a 40-field God object:

| Field | Type | Contains |
|---|---|---|
| `app.kbd` | `InputState` | input string, cursor, history, pending images, tab state |
| `app.live` | `StreamState` | streaming buffer, pending tool, thinking buf, tool status |
| `app.conv` | `ConvState` | history, session path, seen paths, undo stack |
| `app.view` | `ViewState` | messages, scroll, search, msg_line_offsets, render_cache, last_tool_render |

Top-level App keeps: `mode`, `confirm`, `provider`, `model`, `usage`, `config`, `options`, `policy`, channels.

## Key conventions

- **Zero warnings** — `cargo build` must be clean. `#[allow(...)]` only with a comment.
- **cargo fmt** — always run before committing; CI enforces `cargo fmt --check`.
- **cargo clippy -- -D warnings** — CI runs on Ubuntu (stricter than macOS). Test locally too.
- **No new dependencies** without good reason — Cargo.toml is deliberately lean.
- **Tests live in the same file** as the code they test (bottom `#[cfg(test)] mod tests`).
- **246 tests** — `cargo test` must stay green.

## Build & test

```bash
cargo build          # dev build
cargo build --release # production binary
cargo test           # 246 tests
cargo clippy -- -D warnings
cargo fmt --check
```

## Release process

1. Bump `version` in both `Cargo.toml`, `package.json`, and `npm/package.json`
2. `git tag vX.Y.Z && git push origin main --tags`
3. GitHub Actions (`release.yml`) builds 5 platform binaries and uploads to GitHub Release
4. After binaries are live, `npm publish` from repo root (or automated via `NPM_TOKEN` secret)

## Config file location

`~/.config/anveesa/config.toml` (override with `ANVEESA_CONFIG` env var).

## Provider abstraction

`provider::ask(config, provider_name, request, policy, events_tx)` is the single entry point. It dispatches to `openai_compatible::ask` or `command::ask` based on the provider kind.

`StreamEvent` is the channel type: `Token`, `Thinking`, `ToolCall`, `ToolResult`, `FileOp`, `Confirm`, `Usage`, `PlanSet`, `PlanTaskDone`.

## TuiEvent

`TuiEvent` bridges the provider stream to the TUI event loop: `Token`, `Thinking`, `SystemMsg`, `ModelUsed`, `SetInput` (pre-fills input field), `Usage`, `Error`, `FileOp`, `Confirm`, `ToolCall`, `ToolDone`, `PlanSet`, `PlanTaskDone`.

## Web server (web.rs)

Uses axum 0.7. Routes:
- `GET /` → serve embedded `web_ui.html`
- `GET /api/info` → `{"provider":"…","model":"…"}`
- `POST /api/ask` → SSE stream of `{"token":"…"}` events, ends with `{"done":true}`

## Workspace context

`workspace_context_for(cwd)` builds the system context injected into every prompt. It includes: cwd, git branch/status/log, repo map (`git ls-files --cached`, source files only, max 250), README (3k chars), `.anveesa.md` project instructions, Cargo.toml/package.json metadata.

## TUI Performance

- **Render cache:** `app.view.render_cache` uses O(1) index lookups with hash-based change detection. Only re-format messages whose content hash changed.
- **Streaming optimization:** During streaming, only the last streaming buffer gets re-formatted per frame (not all historical messages).
- **Tool render throttling:** When a tool is executing (`pending_tool.is_some()`), renders are throttled to ~2Hz (500ms interval) to avoid UI freeze.
- **Scroll stability:** `finish_turn` resets `auto_scroll = true` and `mode = Input` so the next turn starts scrolled to bottom.

## Tools (32 total)

### Original 28
`read_file`, `write_file`, `edit_file`, `list_dir`, `find_files`, `search_text`, `run_command`, `git_status`, `git_diff`, `git_log`, `git_blame`, `git_show`, `git_branch`, `git_commit`, `git_stash`, `copy_file`, `delete_file`, `create_dir`, `move_file`, `web_search`, `fetch_url`, `screenshot_url`, `read_notes`, `save_note`, `search_notes`, `delete_note`, `set_plan`, `complete_task`

### Added in 0.7.5
- **`read_image`** — Load image files, return base64 + metadata (so the AI can "see" images)
- **`glob`** — Recursive file pattern matching (glob patterns)
- **`grep`** — Regex search across files with context lines
- **`patch`** — In-place regex find-and-replace across files

## Custom Slash Commands

Users can create `~ ~/.anveesa/commands/*.md` files. Each file is auto-discovered and registered as `/filename` command. Commands are executed by sending the file content as a user message. No code changes needed.

## TUI Slash Commands

`/add <file>`, `/diff`, `/commit [msg]`, `/memory <note>`, `/compact`, `/undo`, `/copy`, `/export`, `/export text`, `/model`, `/provider`, `/status`, `/search`, `/clear`, `/help`, `/exit`, `/plan`
