//! Executor: runs a resolved recipe — its dependency closure first, then itself.
//!
//! The run order comes from `validate::plan` (deps-first, deduped, root last).
//! Dependencies run with no arguments; only the invoked goal receives the args
//! bound during resolution. Each recipe's body is one shell invocation, so `cd`
//! and variables persist across its lines (but not across recipes). Bodies run
//! with `set -e`, so a body aborts on its first failing command; a goal opts out
//! by starting with `set +e`. The first non-zero exit aborts the sequence. A
//! body's commands are echoed to stderr (bold on a TTY) just before it runs.
//!
//! Parameters are made available two ways: as environment variables and via
//! `{{name}}` substitution in the body text.

use crate::parser::{Recipe, Vafile};
use crate::resolver::Resolved;
use crate::validate::plan;
use std::process::Command;

pub fn execute(vafile: &Vafile, resolved: &Resolved) -> i32 {
    let order = plan(vafile, &resolved.recipe.path);
    let no_args: Vec<(String, String)> = Vec::new();

    for path in &order {
        let recipe = vafile.get(path).expect("planned recipe exists");
        // Only the invoked goal gets arguments; dependencies take none.
        let args = if path == &resolved.recipe.path {
            &resolved.args
        } else {
            &no_args
        };
        let code = run_recipe(recipe, args);
        if code != 0 {
            return code;
        }
    }
    0
}

/// Echo a recipe's commands to stderr just before they run — bold on a TTY — so
/// it's visible what executed (as `just` does). Blank and comment-only lines are
/// skipped, and the internal `set -e` is never shown (it isn't part of the body).
/// Styling is dropped when stderr isn't a terminal or `NO_COLOR` is set, so piped
/// output and logs stay clean.
fn print_commands(script: &str) {
    use std::io::IsTerminal;
    let bold = std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    for line in script.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if bold {
            eprintln!("\x1b[1m{line}\x1b[0m");
        } else {
            eprintln!("{line}");
        }
    }
}

fn run_recipe(recipe: &Recipe, args: &[(String, String)]) -> i32 {
    // Substitute {{param}} occurrences in the body.
    let mut script = recipe.body.join("\n");
    for (name, value) in args {
        script = script.replace(&format!("{{{{{}}}}}", name), value);
    }

    if script.trim().is_empty() {
        // Empty body (e.g. a pure dependency aggregator): nothing to run.
        return 0;
    }

    // Echo the commands about to run (like `just`), so what executed is visible.
    print_commands(&script);

    // Bodies fail-fast: a non-zero command aborts the goal, in keeping with the
    // deps-first "stop on first failure" model (and `just`/`make`). Prepending
    // `set -e` keeps the single shell session, so `cd`/vars still persist across
    // lines; a goal that wants continue-on-error starts its body with `set +e`.
    let script = format!("set -e\n{script}");

    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(&script);

    // Inject params as environment variables too.
    for (name, value) in args {
        cmd.env(name, value);
    }

    match cmd.status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(e) => {
            eprintln!(
                "va: failed to execute recipe `{}`: {}",
                recipe.display_name(),
                e
            );
            1
        }
    }
}
