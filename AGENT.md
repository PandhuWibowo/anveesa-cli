# AGENT.md — AI agent guide for anveesa-cli

This file tells an AI coding agent everything it needs to work on this codebase without asking clarifying questions.

## Project summary

**anveesa** is a Rust CLI (edition 2024) that wraps any OpenAI-compatible AI provider into a unified terminal interface. It has three modes: a full-screen TUI (ratatui), a plain REPL, and a one-shot `anveesa "prompt"` mode. A browser chat UI is available via `anveesa web`.

**Version:** 0.7.5 | **Tests:** 246 passing (237 unit + 9 doc) | **Warnings:** 0

## Repository layout

```
anveesa-cli/
├── src/                    # All Rust source
│   ├── lib.rs              # Root: CLI dispatch, interactive REPL, ask_streaming
│   ├── cli.rs              # Clap CLI definitions
│   ├── config.rs           # Configuration types and loader
│   ├── display.rs          # Plain-terminal (non-TUI) output helpers
│   ├── image.rs            # Clipboard/file image loading
│   ├── mcp.rs              # MCP protocol client
│   ├── prompt.rs           # Raw terminal line reader (non-TUI REPL)
│   ├── session.rs          # Session persistence (save/load/purge)
│   ├── tools.rs            # All 32 AI tool implementations
│   ├── tools_scenarios.rs  # Scenario-driven tool tests
│   ├── web.rs              # axum web server + SSE endpoint
│   ├── web_ui.html         # Embedded single-file browser UI
│   ├── workspace.rs        # System context builder (git, files, README)
│   ├── tui.rs              # TUI App struct, types, event loop
│   ├── provider/
│   │   ├── mod.rs          # Shared types: StreamEvent, TurnResult, etc.
│   │   ├── openai_compatible.rs  # Main provider: SSE streaming, tools, retry
│   │   └── command.rs      # Shell-command provider wrapper
│   └── tui/
│       ├── commands.rs     # /add /diff /commit /memory and other slash cmds
│       ├── format.rs       # Text formatting + cursor helpers
│       ├── input.rs        # Tab completion, search
│       ├── render.rs       # ratatui rendering (with incremental cache)
│       └── stream.rs       # submit_prompt, turn management, auto-compact
├── Cargo.toml
├── package.json            # npm package (root-level, not in npm/)
├── scripts/install.js      # npm postinstall: download binary or build from source
├── bin/anveesa.js          # npm bin wrapper (finds and execs the Rust binary)
├── npm/                    # Deprecated — root package.json is the source of truth
├── .github/workflows/
│   ├── ci.yml              # fmt + clippy + tests (Ubuntu + macOS)
│   └── release.yml         # Build binaries + npm publish + crates.io on vX.Y.Z tag
├── CLAUDE.md               # Context for Claude Code
├── AGENT.md                # This file
├── .anveesa.md             # Project instructions injected into anveesa's own prompts
└── README.md
```

## Non-negotiable rules

1. **`cargo build` must produce zero warnings.** Use `#[allow(lint)]` only with a comment explaining why.
2. **`cargo fmt --check` must pass** — always run `cargo fmt` before committing.
3. **`cargo clippy -- -D warnings` must pass on Linux** — Ubuntu clippy is stricter than macOS. Test locally too.
4. **`cargo test` must show 246 passed, 0 failed.**
5. **No logic changes in refactoring PRs** — pure moves only.
6. **No new dependencies without explicit approval** — Cargo.toml is lean by design.

## Commands to run after any change

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```

## App struct layout (tui.rs)

`App` is split into four sub-structs. Always use the correct sub-struct:

| Sub-struct | Access via | Contains |
|---|---|---|
| `InputState` | `app.kbd` | `input`, `input_cursor`, `input_history`, `hist_idx`, `hist_saved`, `pending_images`, `last_image_fp`, `tab_state` |
| `StreamState` | `app.live` | `streaming_buf`, `accumulated_response`, `pending_tool`, `tool_status`, `plan_tasks`, `plan_done`, `pending_prompt`, `streaming_started_at`, `tool_started_at`, `unread_count`, `thinking_buf` |
| `ConvState` | `app.conv` | `history`, `session_path`, `last_saved_at`, `seen_paths`, `undo_stack` |
| `ViewState` | `app.view` | `messages`, `scroll`, `auto_scroll`, `total_lines`, `msg_focus`, `msg_line_offsets`, `search_query`, `search_results`, `search_idx`, `search_scroll_saved`, `mouse_capture`, `render_cache`, `render_cache_streaming_len` |

Top-level `App` keeps: `mode`, `confirm`, `provider`, `model`, `last_model_used`, `usage`, `session_cost_usd`, `cwd`, `images_available`, `config`, `options`, `workspace_context`, `policy`, `mcp`, channels, `quit`, `spinner_frame`.

## Adding a new slash command

1. Add the handler in `src/tui/commands.rs` inside `handle_slash_command` (match arm before `_ => false`)
2. Add the command name to `SLASH_COMMANDS` in `src/tui/input.rs`
3. Update the `/help` text in `commands.rs`
4. If it needs tab completion (e.g., file paths), add a branch in `compute_tab_completions` in `input.rs`

**Custom commands:** Users can create `~/.anveesa/commands/*.md` files. These are auto-discovered and registered as `/filename` commands (filename becomes the command name). No code changes needed.

## Adding a new tool

1. Add a `pub async fn` in `src/tools.rs` following the existing pattern
2. Add it to `definitions()` (always) and optionally mark it write-only in `is_write_tool()`
3. Add it to `describe_call()` for user-facing descriptions
4. Wire the call in the `run()` dispatch match (async for new tools)
5. Add a scenario block in `src/tools_scenarios.rs`
6. Wire the call in `openai_compatible.rs` in the tool dispatch match

## Adding a new provider kind

1. Add a variant to `ProviderConfig` in `config.rs`
2. Add an `ask()` implementation in a new `src/provider/my_provider.rs`
3. Dispatch from `provider::ask()` in `provider/mod.rs`

## Releasing a new version

```bash
# 1. Bump versions
vim Cargo.toml        # version = "X.Y.Z"
vim package.json      # "version": "X.Y.Z"
vim npm/package.json  # "version": "X.Y.Z"
cargo build           # updates Cargo.lock

# 2. Test + lint
cargo fmt && cargo clippy -- -D warnings && cargo test

# 3. Commit + tag (triggers CI and release workflow)
git add -A && git commit -m "feat: vX.Y.Z — ..."
git tag vX.Y.Z
git push origin main --tags

# 4. Once GitHub Actions finishes building binaries (~5 min):
npm publish           # uses pandhuw npm account
```

## Key invariants

- `workspace_context_for` injects repo files from `git ls-files --cached` (source extensions only, max 250 files) — do not change this to read file contents; it would blow up the context window.
- `auto_compact_if_needed` in `stream.rs` MUST inject a `ChatMessage::user` context note before draining history, or the model loses orientation. Do not remove that insert.
- The `#[allow(unused_assignments)]` on `last_effective_model` in `openai_compatible.rs` is intentional — Rust can't prove the `loop` body runs at least once, but it always does.
- `pub conv: ConvState` on App is `pub(crate)` (not fully pub) to avoid leaking private types.
- **Render cache:** `app.view.render_cache` is a `Vec<(usize, u64, Vec<Line>)>` — message index, content hash, cached lines. O(1) lookup by index. Only re-format messages whose hash changed. During streaming, only the last streaming buffer gets re-formatted per frame.
- **Tool render throttling:** When `pending_tool.is_some()`, renders are throttled to ~2Hz (500ms interval) to avoid freeze during long-running tool execution.
