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

use crate::parser::{Param, Vafile};
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
    /// A dependency supplies fewer arguments than the target goal requires.
    DependencyNeedsArgs {
        source: String,
        line: usize,
        recipe: String,
        dep: String,
        required: Vec<String>,
    },
    /// A dependency supplies more arguments than the target goal accepts.
    DependencyTooManyArgs {
        source: String,
        line: usize,
        recipe: String,
        dep: String,
        got: usize,
        max: usize,
    },
    /// A dependency argument references a `{{param}}` the declaring recipe lacks.
    DependencyUnknownParam {
        source: String,
        line: usize,
        recipe: String,
        dep: String,
        param: String,
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
                "{}:{}: `{}` depends on `{}`, which needs argument(s) {}; pass them like `{} <value>`",
                source,
                line,
                recipe,
                dep,
                required.join(", "),
                dep
            ),
            ValidateError::DependencyTooManyArgs {
                source,
                line,
                recipe,
                dep,
                got,
                max,
            } => write!(
                f,
                "{}:{}: `{}` passes {} argument(s) to `{}`, which takes at most {}",
                source, line, recipe, got, dep, max
            ),
            ValidateError::DependencyUnknownParam {
                source,
                line,
                recipe,
                dep,
                param,
            } => write!(
                f,
                "{}:{}: `{}`'s dependency `{}` references parameter `{}`, which `{}` does not declare",
                source, line, recipe, dep, param, recipe
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

    // Phase A: every dependency edge must resolve to a runnable goal, and its
    // arguments must fit that goal's parameters.
    for recipe in vafile.recipes.values() {
        let from = recipe.display_name();
        let line = recipe.line;
        let source = recipe.source.clone();
        let own_params: Vec<&str> = recipe.params.iter().map(|p| p.name.as_str()).collect();
        for dep in &recipe.deps {
            let dep_name = dep.path.join("::");
            match vafile.get(&dep.path) {
                Some(target) => {
                    let got = dep.args.len();
                    let max = target.params.len();
                    // Positional fill: the params past `got` that aren't optional
                    // are the ones the dependency failed to supply.
                    let missing: Vec<String> = target.params[got.min(max)..]
                        .iter()
                        .filter(|p| !p.optional)
                        .map(|p| p.name.clone())
                        .collect();
                    if !missing.is_empty() {
                        errors.push(ValidateError::DependencyNeedsArgs {
                            source: source.clone(),
                            line,
                            recipe: from.clone(),
                            dep: dep_name.clone(),
                            required: missing,
                        });
                    } else if got > max {
                        errors.push(ValidateError::DependencyTooManyArgs {
                            source: source.clone(),
                            line,
                            recipe: from.clone(),
                            dep: dep_name.clone(),
                            got,
                            max,
                        });
                    }
                    // A `{{param}}` in a dep argument must name one of *this*
                    // recipe's parameters (that's the value that gets forwarded).
                    for arg in &dep.args {
                        for pref in param_refs(arg) {
                            if !own_params.contains(&pref.as_str()) {
                                errors.push(ValidateError::DependencyUnknownParam {
                                    source: source.clone(),
                                    line,
                                    recipe: from.clone(),
                                    dep: dep_name.clone(),
                                    param: pref,
                                });
                            }
                        }
                    }
                }
                None if vafile.is_namespace(&dep.path) => {
                    errors.push(ValidateError::DependencyIsNamespace {
                        source: source.clone(),
                        line,
                        recipe: from.clone(),
                        dep: dep_name,
                        available: vafile.children(&dep.path),
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
        let dep_key = dep.path.join("::");
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

/// One scheduled step: a goal to run and the arguments bound to its parameters.
#[derive(Debug, PartialEq, Eq)]
pub struct PlanNode {
    pub path: Vec<String>,
    pub args: Vec<(String, String)>,
}

/// Build the run order from `root` (invoked with `root_args`): deduped,
/// dependencies first, root last. A dependency's `{{param}}` arguments are
/// resolved against the declaring recipe's bound args as we descend, so the same
/// goal reached with *different* args runs once per distinct argument set.
/// Assumes a validated (acyclic, fully-resolved, arg-checked) graph.
pub fn plan(vafile: &Vafile, root: &[String], root_args: &[(String, String)]) -> Vec<PlanNode> {
    let mut visited: HashSet<String> = HashSet::new();
    let mut order: Vec<PlanNode> = Vec::new();
    plan_visit(vafile, root, root_args.to_vec(), &mut visited, &mut order);
    order
}

fn plan_visit(
    vafile: &Vafile,
    path: &[String],
    args: Vec<(String, String)>,
    visited: &mut HashSet<String>,
    order: &mut Vec<PlanNode>,
) {
    if !visited.insert(plan_key(path, &args)) {
        return; // this goal+args pairing is already scheduled
    }
    if let Some(recipe) = vafile.get(path) {
        for dep in &recipe.deps {
            // Fill this dep's `{{param}}` args from the current recipe's args,
            // then bind the resulting values to the dep target's parameters.
            let values: Vec<String> = dep.args.iter().map(|a| substitute(a, &args)).collect();
            let bound = match vafile.get(&dep.path) {
                Some(target) => bind_positional(&target.params, &values),
                None => Vec::new(), // unreachable on a validated graph
            };
            plan_visit(vafile, &dep.path, bound, visited, order);
        }
        order.push(PlanNode {
            path: path.to_vec(),
            args,
        });
    }
}

/// Dedup key that distinguishes the same goal invoked with different arguments.
fn plan_key(path: &[String], args: &[(String, String)]) -> String {
    let mut key = path.join("::");
    for (_, value) in args {
        key.push('\u{0}');
        key.push_str(value);
    }
    key
}

/// Replace `{{name}}` occurrences in `text` using the given bound args. Same
/// mechanism as body substitution, so `{{x}}` means the same thing everywhere.
fn substitute(text: &str, args: &[(String, String)]) -> String {
    let mut out = text.to_string();
    for (name, value) in args {
        out = out.replace(&format!("{{{{{}}}}}", name), value);
    }
    out
}

/// Bind positional values to a target's parameters (optionals default to empty).
/// Counts were checked in `validate`, so surplus values cannot reach here.
fn bind_positional(params: &[Param], values: &[String]) -> Vec<(String, String)> {
    params
        .iter()
        .enumerate()
        .map(|(i, p)| (p.name.clone(), values.get(i).cloned().unwrap_or_default()))
        .collect()
}

/// The parameter names referenced by `{{name}}` markers in `s`.
fn param_refs(s: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let mut rest = s;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let name = after[..end].trim();
                if !name.is_empty() {
                    refs.push(name.to_string());
                }
                rest = &after[end + 2..];
            }
            None => break,
        }
    }
    refs
}
