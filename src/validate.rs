//! Phase-1 static validation of the dependency graph, plus the execution plan.
//!
//! This runs *before anything executes*, honoring va's rule that every failure
//! is explicit and nothing is run on an unsound file. Design choices:
//!   - Whole-file: the entire graph is validated, not just the subgraph reachable
//!     from the invoked goal — a vafile is valid or it isn't, regardless of which
//!     goal you ran.
//!   - Aggregate: all edge-resolution errors are collected and reported together.
//!   - Cycle detection runs only once every edge is known-good (so it can never
//!     trip over a dangling reference), and reports the offending path.
//!
//! Execution order (`plan`) is a deduped, deps-first, post-order DFS: each recipe
//! runs at most once per invocation, dependencies before dependents.

use crate::parser::Vafile;
use std::collections::HashSet;

#[derive(Debug)]
pub enum ValidateError {
    /// A dependency names a goal that does not exist.
    UnknownDependency {
        source: String,
        line: usize,
        recipe: String,
        dep: String,
    },
    /// A dependency target has required parameters, which deps cannot supply.
    DependencyNeedsArgs {
        source: String,
        line: usize,
        recipe: String,
        dep: String,
        required: Vec<String>,
    },
    /// A dependency points at a namespace with no default goal (not runnable).
    DependencyIsNamespace {
        source: String,
        line: usize,
        recipe: String,
        dep: String,
        available: Vec<String>,
    },
    /// The dependency graph contains a cycle; `path` is the offending loop.
    /// `line`/`source` point at the header of the first goal in the loop.
    Cycle {
        source: String,
        line: usize,
        path: Vec<String>,
    },
}

impl std::fmt::Display for ValidateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidateError::UnknownDependency {
                source,
                line,
                recipe,
                dep,
            } => write!(
                f,
                "{}:{}: `{}` depends on `{}`, which is not a defined goal",
                source, line, recipe, dep
            ),
            ValidateError::DependencyNeedsArgs {
                source,
                line,
                recipe,
                dep,
                required,
            } => write!(
                f,
                "{}:{}: `{}` depends on `{}`, which requires argument(s) {}; dependencies cannot pass arguments",
                source,
                line,
                recipe,
                dep,
                required.join(", ")
            ),
            ValidateError::DependencyIsNamespace {
                source,
                line,
                recipe,
                dep,
                available,
            } => write!(
                f,
                "{}:{}: `{}` depends on `{}`, which is a namespace, not a runnable goal (subcommands: {})",
                source,
                line,
                recipe,
                dep,
                available.join(", ")
            ),
            ValidateError::Cycle { source, line, path } => {
                write!(f, "{}:{}: dependency cycle: {}", source, line, path.join(" -> "))
            }
        }
    }
}

/// Validate the whole dependency graph. Returns every problem found.
pub fn validate(vafile: &Vafile) -> Result<(), Vec<ValidateError>> {
    let mut errors = Vec::new();

    // Phase A: every dependency edge must resolve to a runnable, arg-free goal.
    for recipe in vafile.recipes.values() {
        let from = recipe.display_name();
        let line = recipe.line;
        let source = recipe.source.clone();
        for dep in &recipe.deps {
            let dep_name = dep.join("::");
            match vafile.get(dep) {
                Some(target) => {
                    let required: Vec<String> = target
                        .params
                        .iter()
                        .filter(|p| !p.optional)
                        .map(|p| p.name.clone())
                        .collect();
                    if !required.is_empty() {
                        errors.push(ValidateError::DependencyNeedsArgs {
                            source: source.clone(),
                            line,
                            recipe: from.clone(),
                            dep: dep_name,
                            required,
                        });
                    }
                }
                None if vafile.is_namespace(dep) => {
                    errors.push(ValidateError::DependencyIsNamespace {
                        source: source.clone(),
                        line,
                        recipe: from.clone(),
                        dep: dep_name,
                        available: vafile.children(dep),
                    });
                }
                None => errors.push(ValidateError::UnknownDependency {
                    source: source.clone(),
                    line,
                    recipe: from.clone(),
                    dep: dep_name,
                }),
            }
        }
    }

    // A dangling edge would make cycle detection meaningless, so stop here.
    if !errors.is_empty() {
        return Err(errors);
    }

    // Phase B: cycle detection over the now fully-resolved graph.
    if let Some(path) = find_cycle(vafile) {
        // Point at the header of the first goal in the loop.
        let first = vafile.recipes.get(&path[0]);
        let line = first.map(|r| r.line).unwrap_or(0);
        let source = first.map(|r| r.source.clone()).unwrap_or_default();
        return Err(vec![ValidateError::Cycle { source, line, path }]);
    }

    Ok(())
}

/// Three-color DFS. Returns the cycle path (closed, e.g. build -> a -> build).
fn find_cycle(vafile: &Vafile) -> Option<Vec<String>> {
    // 0 = white (unseen), 1 = gray (on stack), 2 = black (done).
    let mut color: std::collections::HashMap<String, u8> = std::collections::HashMap::new();
    let mut stack: Vec<String> = Vec::new();
    for key in vafile.recipes.keys() {
        if color.get(key).copied().unwrap_or(0) == 0 {
            if let Some(cycle) = dfs_cycle(vafile, key, &mut color, &mut stack) {
                return Some(cycle);
            }
        }
    }
    None
}

fn dfs_cycle(
    vafile: &Vafile,
    key: &str,
    color: &mut std::collections::HashMap<String, u8>,
    stack: &mut Vec<String>,
) -> Option<Vec<String>> {
    color.insert(key.to_string(), 1);
    stack.push(key.to_string());
    // Every dep is known to resolve (Phase A passed), so this lookup is safe.
    let recipe = vafile.recipes.get(key).expect("recipe exists");
    for dep in &recipe.deps {
        let dep_key = dep.join("::");
        match color.get(&dep_key).copied().unwrap_or(0) {
            1 => {
                // Back-edge: slice the stack from where this node was first seen.
                let start = stack.iter().position(|n| n == &dep_key).unwrap();
                let mut cycle: Vec<String> = stack[start..].to_vec();
                cycle.push(dep_key); // close the loop for display
                return Some(cycle);
            }
            0 => {
                if let Some(cycle) = dfs_cycle(vafile, &dep_key, color, stack) {
                    return Some(cycle);
                }
            }
            _ => {} // black: fully explored, no cycle through it
        }
    }
    stack.pop();
    color.insert(key.to_string(), 2);
    None
}

/// Build the run order from `root`: deduped, dependencies first, root last.
/// Assumes a validated (acyclic, fully-resolved) graph.
pub fn plan(vafile: &Vafile, root: &[String]) -> Vec<Vec<String>> {
    let mut visited: HashSet<String> = HashSet::new();
    let mut order: Vec<Vec<String>> = Vec::new();
    plan_visit(vafile, root, &mut visited, &mut order);
    order
}

fn plan_visit(
    vafile: &Vafile,
    path: &[String],
    visited: &mut HashSet<String>,
    order: &mut Vec<Vec<String>>,
) {
    let key = path.join("::");
    if !visited.insert(key) {
        return; // already scheduled
    }
    if let Some(recipe) = vafile.get(path) {
        for dep in &recipe.deps {
            plan_visit(vafile, dep, visited, order);
        }
        order.push(path.to_vec());
    }
}
