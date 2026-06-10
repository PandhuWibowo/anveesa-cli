use super::{App, Msg};

pub(super) fn msg_text(msg: &Msg) -> Option<&str> {
    match msg {
        Msg::User { text } | Msg::Assistant { text } | Msg::Error(text) | Msg::System(text) => {
            Some(text)
        }
        Msg::Tool { text, .. } => Some(text),
        Msg::FileOp { path, .. } => Some(path),
        Msg::Thinking { text, .. } => Some(text),
        Msg::Separator => None,
    }
}

pub(super) fn update_search(app: &mut App) {
    let q = app.view.search_query.to_lowercase();
    app.view.search_results = if q.is_empty() {
        vec![]
    } else {
        app.view
            .messages
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                msg_text(m)
                    .map(|t| t.to_lowercase().contains(&q))
                    .unwrap_or(false)
            })
            .map(|(i, _)| i)
            .collect()
    };
    app.view.search_idx = 0;
    if let Some(&first) = app.view.search_results.first()
        && let Some(&off) = app.view.msg_line_offsets.get(first)
    {
        app.view.auto_scroll = false;
        app.view.scroll = off.saturating_sub(2);
    }
}

const SLASH_COMMANDS: &[&str] = &[
    "/add",
    "/agent",
    "/branch",
    "/clear",
    "/commit",
    "/compact",
    "/copy",
    "/cost",
    "/diff",
    "/exit",
    "/export",
    "/help",
    "/init",
    "/memory",
    "/model",
    "/note",
    "/notes",
    "/provider",
    "/quit",
    "/retry",
    "/search",
    "/status",
    "/undo",
];

pub(super) fn tab_complete(app: &mut App) {
    let input = app.kbd.input.clone();

    let continuing = app
        .kbd
        .tab_state
        .as_ref()
        .map(|(_, cands, idx)| cands.get(*idx).map(|s| s == &input).unwrap_or(false))
        .unwrap_or(false);

    if continuing {
        if let Some((_, cands, idx)) = &mut app.kbd.tab_state {
            *idx = (*idx + 1) % cands.len();
            let next = cands[*idx].clone();
            app.kbd.input = next;
            app.kbd.input_cursor = app.kbd.input.len();
        }
        return;
    }

    let providers: Vec<String> = app.config.providers.keys().cloned().collect();
    let cands = compute_tab_completions(&input, &app.cwd, &providers);
    if cands.is_empty() {
        return;
    }

    app.kbd.input = cands[0].clone();
    app.kbd.input_cursor = app.kbd.input.len();
    app.kbd.tab_state = Some((input, cands, 0));
}

fn compute_tab_completions(input: &str, cwd: &str, providers: &[String]) -> Vec<String> {
    if input.starts_with('/') && !input.contains(' ') {
        let matches: Vec<String> = SLASH_COMMANDS
            .iter()
            .filter(|c| c.starts_with(input))
            .map(|s| s.to_string())
            .collect();
        if !matches.is_empty() {
            return matches;
        }
    }

    if let Some(partial) = input.strip_prefix("/provider ") {
        let mut matches: Vec<String> = providers
            .iter()
            .filter(|p| p.starts_with(partial))
            .map(|p| format!("/provider {p}"))
            .collect();
        matches.sort();
        if !matches.is_empty() {
            return matches;
        }
    }

    if let Some(partial) = input.strip_prefix("/add ") {
        let paths = tab_complete_path(partial, cwd);
        return paths.into_iter().map(|p| format!("/add {p}")).collect();
    }

    if let Some(partial) = input.strip_prefix("/export ") {
        let paths = tab_complete_path(partial, cwd);
        return paths.into_iter().map(|p| format!("/export {p}")).collect();
    }

    vec![]
}

fn tab_complete_path(partial: &str, cwd: &str) -> Vec<String> {
    let (dir_part, file_part) = if let Some(i) = partial.rfind('/') {
        (&partial[..i + 1], &partial[i + 1..])
    } else {
        ("", partial)
    };

    let search_dir = if dir_part.is_empty() {
        std::path::PathBuf::from(cwd)
    } else if dir_part.starts_with('/') {
        std::path::PathBuf::from(dir_part)
    } else {
        std::path::Path::new(cwd).join(dir_part)
    };

    let Ok(entries) = std::fs::read_dir(&search_dir) else {
        return vec![];
    };
    let mut out: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            if !name.starts_with(file_part) {
                return None;
            }
            let trail = if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                "/"
            } else {
                ""
            };
            Some(format!("{dir_part}{name}{trail}"))
        })
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_command_completion() {
        let result = compute_tab_completions("/cl", ".", &[]);
        assert_eq!(result, vec!["/clear"]);
    }

    #[test]
    fn provider_completion() {
        let providers = vec![
            "openai".to_string(),
            "anthropic".to_string(),
            "openrouter".to_string(),
        ];
        let result = compute_tab_completions("/provider op", ".", &providers);
        assert_eq!(result, vec!["/provider openai", "/provider openrouter"]);
    }

    #[test]
    fn provider_completion_exact() {
        let providers = vec!["openai".to_string(), "openrouter".to_string()];
        let result = compute_tab_completions("/provider openai", ".", &providers);
        assert_eq!(result, vec!["/provider openai"]);
    }

    #[test]
    fn no_completion_for_empty_slash() {
        let result = compute_tab_completions("hello", ".", &[]);
        assert!(result.is_empty());
    }
}
