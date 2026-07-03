//! The import loader: turns a root `vafile` plus its `import` directives into a
//! single merged [`Vafile`] for the rest of the pipeline (validate/resolve/run).
//!
//! Design, matching va's "everything explicit, checked up front" stance:
//!   - **Pure parser, IO here.** [`crate::parser::parse`] stays filesystem-free
//!     and only *records* imports; this module does the reads and the merge.
//!   - **Two import shapes.** `import "x"` merges flat (imported goals keep their
//!     names); `import "x" as ns` nests the whole file under `ns` — every
//!     imported recipe's path *and its internal dependency references* are
//!     prefixed, so the CLI form (`va ns goal`) and dep form (`ns::goal`) stay
//!     the single unified path va already uses.
//!   - **All clashes are errors.** Any two files defining the same final goal
//!     name is a hard error, reported before anything runs. Namespacing
//!     (`as ns`) is how you disambiguate.
//!   - **Transitive + cycle-safe.** Imported files may import in turn; a file
//!     reachable from itself is reported as an import cycle, not followed.
//!
//! Paths in an `import` are resolved relative to the directory of the file that
//! contains the directive.

use crate::parser::{self, Recipe, Vafile};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum LoadError {
    /// A file (root or imported) could not be read. `from` names the `import`
    /// directive that referenced it, when there is one.
    Read {
        path: String,
        err: String,
        from: Option<(String, usize)>,
    },
    /// A parse error within a specific file.
    Parse {
        source: String,
        line: usize,
        message: String,
    },
    /// `import` directives form a cycle; `chain` is the offending file loop.
    Cycle { chain: Vec<String> },
    /// The same final goal name is defined in two places.
    Clash {
        name: String,
        first: (String, usize),
        second: (String, usize),
    },
    /// A goal outside an `as` import lives *inside* the namespace that import
    /// introduced. An `as` namespace is sealed: its goals must all come from
    /// the one import. (Giving the namespace a bare default goal is still fine.)
    SealedNamespace {
        namespace: String,
        import_site: (String, usize),
        goal: String,
        goal_site: (String, usize),
    },
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Read { path, err, from } => {
                write!(f, "cannot read `{}`: {}", path, err)?;
                if let Some((label, line)) = from {
                    write!(f, " (imported from {}:{})", label, line)?;
                }
                Ok(())
            }
            LoadError::Parse {
                source,
                line,
                message,
            } => write!(f, "{}:{}: {}", source, line, message),
            LoadError::Cycle { chain } => {
                write!(f, "import cycle: {}", chain.join(" -> "))
            }
            LoadError::Clash {
                name,
                first,
                second,
            } => write!(
                f,
                "goal `{}` is defined in both {}:{} and {}:{}",
                name, first.0, first.1, second.0, second.1
            ),
            LoadError::SealedNamespace {
                namespace,
                import_site,
                goal,
                goal_site,
            } => write!(
                f,
                "{}:{}: `{}` cannot be added to namespace `{}`, which is filled by the import at {}:{}; \
                 an `as` namespace's goals must all come from that one import \
                 (a bare `{}:` default goal is allowed, but new sub-goals are not)",
                goal_site.0,
                goal_site.1,
                goal,
                namespace,
                import_site.0,
                import_site.1,
                namespace
            ),
        }
    }
}

/// Load `path` and everything it imports into one merged [`Vafile`].
pub fn load(path: &Path) -> Result<Vafile, LoadError> {
    let mut merged = Vafile::default();
    let mut stack: Vec<(PathBuf, String)> = Vec::new();
    load_into(path, None, &mut merged, &mut stack)?;
    Ok(merged)
}

/// Parse `path`, merge its own recipes into `merged`, then recurse into its
/// imports. `from` is the `import` directive that pulled `path` in (for error
/// messages); `stack` holds the canonical paths currently being loaded, so a
/// file that imports (transitively) back into itself is caught as a cycle.
fn load_into(
    path: &Path,
    from: Option<(String, usize)>,
    merged: &mut Vafile,
    stack: &mut Vec<(PathBuf, String)>,
) -> Result<(), LoadError> {
    let label = path.display().to_string();

    // Canonicalize for cycle detection. This also surfaces a missing import as
    // a clean read error rather than a panic deeper in.
    let canon = std::fs::canonicalize(path).map_err(|e| LoadError::Read {
        path: label.clone(),
        err: e.to_string(),
        from: from.clone(),
    })?;
    if stack.iter().any(|(c, _)| c == &canon) {
        let mut chain: Vec<String> = stack.iter().map(|(_, l)| l.clone()).collect();
        chain.push(label);
        return Err(LoadError::Cycle { chain });
    }

    let src = std::fs::read_to_string(path).map_err(|e| LoadError::Read {
        path: label.clone(),
        err: e.to_string(),
        from: from.clone(),
    })?;
    let parsed = parser::parse(&src).map_err(|e| LoadError::Parse {
        source: label.clone(),
        line: e.line,
        message: e.message,
    })?;

    stack.push((canon, label.clone()));

    // This file's own goals, stamped with its label for later error messages.
    for (_, mut recipe) in parsed.recipes {
        recipe.source = label.clone();
        merge_one(merged, recipe)?;
    }

    // Imported files, resolved relative to this file's directory.
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    for import in &parsed.imports {
        let child = base.join(&import.path);
        let mut sub = Vafile::default();
        load_into(&child, Some((label.clone(), import.line)), &mut sub, stack)?;
        if let Some(ns) = &import.alias {
            apply_namespace(&mut sub, ns);
            // The `as` namespace is sealed: nothing already merged at this level
            // may live *inside* it. Catches both extending it with a new sub-goal
            // and (redundantly with the clash check) redefining one of its goals.
            // A bare `ns` default goal has path == ns, so it is *not* "inside".
            if let Some(intruder) = merged.recipes.values().find(|r| strictly_under(&r.path, ns)) {
                return Err(LoadError::SealedNamespace {
                    namespace: ns.join("::"),
                    import_site: (label.clone(), import.line),
                    goal: intruder.path.join("::"),
                    goal_site: (intruder.source.clone(), intruder.line),
                });
            }
        }
        for (_, recipe) in sub.recipes {
            merge_one(merged, recipe)?;
        }
    }

    stack.pop();
    Ok(())
}

/// Insert `recipe` into `merged`, treating any name collision as a hard error.
fn merge_one(merged: &mut Vafile, recipe: Recipe) -> Result<(), LoadError> {
    let key = recipe.path.join("::");
    if let Some(existing) = merged.recipes.get(&key) {
        return Err(LoadError::Clash {
            name: key,
            first: (existing.source.clone(), existing.line),
            second: (recipe.source.clone(), recipe.line),
        });
    }
    merged.recipes.insert(key, recipe);
    Ok(())
}

/// Nest every recipe in `sub` under namespace `ns`: prefix each recipe's path
/// and each of its dependency references. Because an imported file's deps point
/// only within its own (sub)tree, prefixing them uniformly keeps them resolving.
fn apply_namespace(sub: &mut Vafile, ns: &[String]) {
    let old = std::mem::take(&mut sub.recipes);
    for (_, mut recipe) in old {
        recipe.path = prefixed(ns, &recipe.path);
        for dep in &mut recipe.deps {
            // Only the target path is namespaced; args are values, not paths.
            dep.path = prefixed(ns, &dep.path);
        }
        sub.recipes.insert(recipe.path.join("::"), recipe);
    }
}

fn prefixed(ns: &[String], path: &[String]) -> Vec<String> {
    let mut out = ns.to_vec();
    out.extend_from_slice(path);
    out
}

/// True if `path` is strictly inside namespace `ns` (a descendant, not `ns`
/// itself). `["ci","test"]` is under `["ci"]`; `["ci"]` is not.
fn strictly_under(path: &[String], ns: &[String]) -> bool {
    path.len() > ns.len() && path.starts_with(ns)
}
