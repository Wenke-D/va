//! The CLI resolver: implements the strict `va` parse model.
//!
//! Rules (final spec):
//!   1. One goal per invocation.
//!   2. Greedy path descent: a token matching a sub-goal/namespace is ALWAYS
//!      path (it shadows any same-named argument). No `--` escape.
//!   3. Once a goal is selected, remaining tokens are arguments only.
//!   4. Each argument must fill a declared positional parameter, else hard error.
//!   5. A namespace's default goal is the same-named plain recipe. If absent,
//!      bare invocation of the namespace is an error listing its sub-goals.
//!   6. `--` is illegal anywhere.

use crate::parser::{Recipe, Vafile};

#[derive(Debug)]
pub enum ResolveError {
    /// `--` was used.
    DashDashForbidden,
    /// A token during path descent matched nothing.
    UnknownCommand {
        token: String,
        scope: Vec<String>,
        available: Vec<String>,
    },
    /// Bare namespace with no default goal.
    NamespaceNeedsSubcommand {
        path: Vec<String>,
        available: Vec<String>,
    },
    /// Extra token that is neither a subcommand nor an accepted parameter.
    NotSubcommandNorParam {
        token: String,
        goal: String,
        takes_args: bool,
    },
    /// Required parameters were not supplied.
    MissingParams {
        goal: String,
        missing: Vec<String>,
    },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::DashDashForbidden => {
                write!(f, "`--` is not allowed in va invocations")
            }
            ResolveError::UnknownCommand {
                token,
                scope,
                available,
            } => {
                let where_ = if scope.is_empty() {
                    "".to_string()
                } else {
                    format!(" in `{}`", scope.join(" "))
                };
                write!(
                    f,
                    "`{}` is not a known command{}. available: {}",
                    token,
                    where_,
                    if available.is_empty() {
                        "(none)".to_string()
                    } else {
                        available.join(", ")
                    }
                )
            }
            ResolveError::NamespaceNeedsSubcommand { path, available } => write!(
                f,
                "`{}` is a namespace, choose a subcommand: {}",
                path.join(" "),
                available.join(", ")
            ),
            ResolveError::NotSubcommandNorParam {
                token,
                goal,
                takes_args,
            } => {
                if *takes_args {
                    write!(
                        f,
                        "`{}` is not a subcommand, and `{}` has no more parameters to fill",
                        token, goal
                    )
                } else {
                    write!(
                        f,
                        "`{}` is not a subcommand, and `{}` takes no arguments",
                        token, goal
                    )
                }
            }
            ResolveError::MissingParams { goal, missing } => write!(
                f,
                "`{}` is missing required argument(s): {}",
                goal,
                missing.join(", ")
            ),
        }
    }
}

/// A fully resolved invocation: which recipe to run and the argument bindings.
#[derive(Debug)]
pub struct Resolved<'a> {
    pub recipe: &'a Recipe,
    /// param name -> value
    pub args: Vec<(String, String)>,
}

/// Resolve CLI tokens against the parsed vafile.
pub fn resolve<'a>(vafile: &'a Vafile, tokens: &[String]) -> Result<Resolved<'a>, ResolveError> {
    // Rule 6: `--` is illegal anywhere.
    if tokens.iter().any(|t| t == "--") {
        return Err(ResolveError::DashDashForbidden);
    }

    let mut path: Vec<String> = Vec::new();
    let mut idx = 0;

    // Phase 1: greedy path descent.
    loop {
        let next = tokens.get(idx);

        match next {
            Some(tok) => {
                let mut candidate = path.clone();
                candidate.push(tok.clone());

                let is_goal = vafile.get(&candidate).is_some();
                let is_ns = vafile.is_namespace(&candidate);

                if is_goal || is_ns {
                    // Token is part of the path (path always wins). Descend.
                    path.push(tok.clone());
                    idx += 1;

                    if is_goal && !is_ns {
                        // Pure goal (leaf). Stop descent, go to args phase.
                        break;
                    }
                    if is_goal && is_ns {
                        // Name is both a goal (default) and a namespace.
                        // Peek: does the NEXT token descend further into the namespace?
                        if let Some(peek) = tokens.get(idx) {
                            let mut deeper = path.clone();
                            deeper.push(peek.clone());
                            if vafile.get(&deeper).is_some() || vafile.is_namespace(&deeper) {
                                // Continue descending; path wins.
                                continue;
                            }
                        }
                        // Next token isn't a sub-path -> select this default goal,
                        // remaining tokens become its args.
                        break;
                    }
                    // Pure namespace: keep descending.
                    continue;
                } else {
                    // Token matches nothing at this scope.
                    if path.is_empty() {
                        // First token unknown -> unknown top-level command.
                        return Err(ResolveError::UnknownCommand {
                            token: tok.clone(),
                            scope: vec![],
                            available: vafile.children(&[]),
                        });
                    }
                    // We're mid-path at a namespace with no goal here, or at a
                    // goal already (handled above). If current path is a goal,
                    // we wouldn't be here. So current path is a namespace ->
                    // it needs a default goal to absorb this token as an arg.
                    if vafile.get(&path).is_some() {
                        // Has default goal; token is an argument. Stop descent.
                        break;
                    }
                    // Namespace with no default goal but an extra token given.
                    return Err(ResolveError::UnknownCommand {
                        token: tok.clone(),
                        scope: path.clone(),
                        available: vafile.children(&path),
                    });
                }
            }
            None => {
                // Ran out of tokens during descent.
                break;
            }
        }
    }

    // After Phase 1, `path` is either a goal or a bare namespace.
    let recipe = match vafile.get(&path) {
        Some(r) => r,
        None => {
            // Bare namespace (or empty). Needs a subcommand.
            if path.is_empty() {
                // `va` with no args and no top-level default -> list everything.
                return Err(ResolveError::NamespaceNeedsSubcommand {
                    path: vec![],
                    available: vafile.children(&[]),
                });
            }
            return Err(ResolveError::NamespaceNeedsSubcommand {
                path: path.clone(),
                available: vafile.children(&path),
            });
        }
    };

    // Phase 2: remaining tokens are arguments only (no path resolution).
    let rest = &tokens[idx..];
    let goal_name = recipe.display_name();

    if rest.len() > recipe.params.len() {
        // Too many args. Report the first offending token.
        let offending = &rest[recipe.params.len()];
        return Err(ResolveError::NotSubcommandNorParam {
            token: offending.clone(),
            goal: goal_name,
            takes_args: !recipe.params.is_empty(),
        });
    }

    // Bind args positionally.
    let mut args = Vec::new();
    for (k, param) in recipe.params.iter().enumerate() {
        if let Some(val) = rest.get(k) {
            args.push((param.name.clone(), val.clone()));
        } else if param.optional {
            args.push((param.name.clone(), String::new()));
        } else {
            // Missing required param. Collect all missing for a good message.
            let missing: Vec<String> = recipe.params[k..]
                .iter()
                .filter(|p| !p.optional)
                .map(|p| p.name.clone())
                .collect();
            return Err(ResolveError::MissingParams {
                goal: goal_name,
                missing,
            });
        }
    }

    Ok(Resolved { recipe, args })
}
