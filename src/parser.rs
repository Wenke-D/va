//! Parser for the `vafile` format.
//!
//! Grammar (v0, minimal subset):
//!   - A recipe header is a line of the form:  NAME [params...] : [deps...]
//!       NAME may contain `::` namespace separators, e.g. `docker::build`
//!       params are space-separated bare identifiers, optionally `name?` (optional)
//!       deps (after the `:`) are goal references run before the body, e.g.
//!       `build: configure compile`. Deps take no arguments.
//!   - The body is the set of following lines indented more than the header.
//!   - Blank lines and lines starting with `#` (at column 0) are ignored.
//!   - Body lines have their common leading indentation stripped.
//!
//! This is deliberately small: it exercises the namespace + resolution design
//! without reimplementing all of just's syntax.

use std::collections::BTreeMap;

/// A single positional parameter of a recipe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    pub optional: bool,
}

/// A dependency edge: a goal to run first, with the positional arguments to pass
/// it. `args` may contain `{{param}}` references to the *declaring* recipe's
/// params (resolved when the plan is built); anything else is a literal value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dep {
    pub path: Vec<String>,
    pub args: Vec<String>,
}

/// A parsed recipe (a "goal").
#[derive(Debug, Clone)]
pub struct Recipe {
    /// Full dotted path, e.g. ["docker", "build"] for `docker::build`.
    pub path: Vec<String>,
    pub params: Vec<Param>,
    /// Dependencies, in declaration order — comma-separated after the `:`, each
    /// a goal reference plus optional args, e.g. `ci: test integration, lint` ->
    /// [Dep{test, [integration]}, Dep{lint, []}]. Run (deduped, deps-first)
    /// before this recipe's body.
    pub deps: Vec<Dep>,
    /// Raw body lines (already de-indented), executed as a shell script.
    pub body: Vec<String>,
    /// Source line number of the header (1-based), for error messages.
    pub line: usize,
    /// Label of the file this recipe was parsed from (for error messages).
    /// The pure parser doesn't know filenames; the loader stamps the real path
    /// when merging imports. Defaults to "vafile" for a standalone parse.
    pub source: String,
}

/// An `import "path" [as namespace]` directive, recorded by the parser and
/// resolved later by the loader (which does the filesystem reads and merge).
#[derive(Debug, Clone)]
pub struct Import {
    /// The quoted path, verbatim. Resolved relative to the importing file.
    pub path: String,
    /// `as <ns>` target namespace, if any. `None` means a flat merge.
    pub alias: Option<Vec<String>>,
    /// Source line of the directive (1-based), for error messages.
    pub line: usize,
}

impl Recipe {
    /// The display name as written in the file, e.g. "docker::build".
    pub fn display_name(&self) -> String {
        self.path.join("::")
    }
}

#[derive(Debug)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "vafile:{}: {}", self.line, self.message)
    }
}

/// The parsed model: an ordered collection of recipes keyed by their path,
/// plus any `import` directives the loader still needs to resolve.
#[derive(Debug, Default)]
pub struct Vafile {
    /// path (joined by "::") -> Recipe
    pub recipes: BTreeMap<String, Recipe>,
    /// `import` directives in declaration order. Empty in a fully-merged Vafile.
    pub imports: Vec<Import>,
}

impl Vafile {
    pub fn get(&self, path: &[String]) -> Option<&Recipe> {
        self.recipes.get(&path.join("::"))
    }

    /// True if `path` is a namespace: some recipe exists strictly below it.
    pub fn is_namespace(&self, path: &[String]) -> bool {
        let prefix = if path.is_empty() {
            String::new()
        } else {
            format!("{}::", path.join("::"))
        };
        self.recipes.keys().any(|k| k.starts_with(&prefix) && k.as_str() != path.join("::"))
    }

    /// Direct children goal/namespace segment names under `path`.
    pub fn children(&self, path: &[String]) -> Vec<String> {
        let depth = path.len();
        let prefix = if path.is_empty() {
            String::new()
        } else {
            format!("{}::", path.join("::"))
        };
        let mut out = Vec::new();
        for key in self.recipes.keys() {
            if !path.is_empty() && !key.starts_with(&prefix) {
                continue;
            }
            let segs: Vec<&str> = key.split("::").collect();
            if segs.len() > depth {
                let child = segs[depth].to_string();
                if !out.contains(&child) {
                    out.push(child);
                }
            }
        }
        out
    }
}

fn leading_spaces(s: &str) -> usize {
    s.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

/// Split a dependency segment into tokens on whitespace, honoring double quotes
/// so an argument can contain spaces (`say "hello world"` -> ["say", "hello world"]).
/// Quote characters are stripped; the first token is the goal, the rest are args.
fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let mut in_quote = false;
    for c in s.chars() {
        if in_quote {
            if c == '"' {
                in_quote = false;
            } else {
                cur.push(c);
            }
        } else if c == '"' {
            in_quote = true;
            started = true;
        } else if c.is_whitespace() {
            if started {
                out.push(std::mem::take(&mut cur));
                started = false;
            }
        } else {
            cur.push(c);
            started = true;
        }
    }
    if started {
        out.push(cur);
    }
    out
}

/// Split a `name` (possibly with `::` separators) into validated path segments.
fn parse_name_path(name: &str, lineno: usize) -> Result<Vec<String>, ParseError> {
    let path: Vec<String> = name.split("::").map(|s| s.to_string()).collect();
    for seg in &path {
        if seg.is_empty() {
            return Err(ParseError {
                line: lineno,
                message: format!("empty namespace segment in `{}`", name),
            });
        }
        if !seg.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
            return Err(ParseError {
                line: lineno,
                message: format!("invalid character in name segment `{}`", seg),
            });
        }
    }
    Ok(path)
}

/// True if a column-0 line is an `import` directive rather than a recipe header.
///
/// The test is deliberately narrow: the line must be the bare word `import`
/// followed by whitespace and then a quote. This keeps a recipe legitimately
/// *named* `import` (written `import:`) from being misread as a directive.
fn looks_like_import(line: &str) -> bool {
    match line.trim().strip_prefix("import") {
        Some(rest) if rest.starts_with(|c: char| c.is_whitespace()) => {
            rest.trim_start().starts_with('"')
        }
        _ => false,
    }
}

/// Parse an `import "path" [as namespace]` directive.
fn parse_import(line: &str, lineno: usize) -> Result<Import, ParseError> {
    let err = |message: String| ParseError { line: lineno, message };

    // Strip the leading `import` keyword (presence guaranteed by the caller).
    let rest = line.trim()["import".len()..].trim_start();
    let after_open = rest
        .strip_prefix('"')
        .ok_or_else(|| err("import expects a quoted path".to_string()))?;
    let close = after_open
        .find('"')
        .ok_or_else(|| err("unterminated string in import directive".to_string()))?;

    let path = after_open[..close].to_string();
    if path.is_empty() {
        return Err(err("import path is empty".to_string()));
    }

    // Anything after the closing quote must be exactly `as <namespace>`.
    let tail = after_open[close + 1..].trim();
    let alias = if tail.is_empty() {
        None
    } else {
        let mut toks = tail.split_whitespace();
        match toks.next() {
            Some("as") => {
                let ns = toks
                    .next()
                    .ok_or_else(|| err("`as` requires a namespace name".to_string()))?;
                if toks.next().is_some() {
                    return Err(err(format!(
                        "unexpected text after import namespace `{}`",
                        ns
                    )));
                }
                Some(parse_name_path(ns, lineno)?)
            }
            _ => {
                return Err(err(format!(
                    "expected `as <namespace>` after import path, found `{}`",
                    tail
                )))
            }
        }
    };

    Ok(Import {
        path,
        alias,
        line: lineno,
    })
}

/// Parse a recipe header `NAME params... : deps...` into (path, params, deps).
///
/// The separator is the first "lone" `:` — one not adjacent to another colon,
/// so it is never confused with a `::` inside a name. Everything left of it is
/// the name and its parameters; everything right is the dependency list.
fn parse_header(
    line: &str,
    lineno: usize,
) -> Result<(Vec<String>, Vec<Param>, Vec<Dep>), ParseError> {
    let trimmed = line.trim();
    let bytes = trimmed.as_bytes();
    let mut sep = None;
    let mut k = 0;
    while k < bytes.len() {
        if bytes[k] == b':' {
            let prev_colon = k > 0 && bytes[k - 1] == b':';
            let next_colon = k + 1 < bytes.len() && bytes[k + 1] == b':';
            if !prev_colon && !next_colon {
                sep = Some(k);
                break;
            }
        }
        k += 1;
    }
    let sep = match sep {
        Some(s) => s,
        None => {
            return Err(ParseError {
                line: lineno,
                message: format!("recipe header must contain ':' -> `{}`", trimmed),
            })
        }
    };

    let left = trimmed[..sep].trim();
    let right = trimmed[sep + 1..].trim();

    let mut parts = left.split_whitespace();
    let name = match parts.next() {
        Some(n) => n,
        None => {
            return Err(ParseError {
                line: lineno,
                message: "recipe header is missing a name".to_string(),
            })
        }
    };
    let path = parse_name_path(name, lineno)?;

    // Params (left of `:`).
    let mut params = Vec::new();
    let mut seen_optional = false;
    for tok in parts {
        let (raw, optional) = if let Some(stripped) = tok.strip_suffix('?') {
            (stripped, true)
        } else {
            (tok, false)
        };
        if optional {
            seen_optional = true;
        } else if seen_optional {
            return Err(ParseError {
                line: lineno,
                message: format!(
                    "required parameter `{}` cannot follow an optional parameter",
                    raw
                ),
            });
        }
        if raw.is_empty() || !raw.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Err(ParseError {
                line: lineno,
                message: format!("invalid parameter name `{}`", tok),
            });
        }
        params.push(Param {
            name: raw.to_string(),
            optional,
        });
    }

    // Deps (right of `:`) are comma-separated. Within one dep, whitespace splits
    // the goal from its positional args, e.g. `test integration, lint`.
    let mut deps = Vec::new();
    if !right.is_empty() {
        for segment in right.split(',') {
            let seg = segment.trim();
            if seg.is_empty() {
                return Err(ParseError {
                    line: lineno,
                    message: "empty dependency (stray or trailing comma?)".to_string(),
                });
            }
            let mut tokens = tokenize(seg).into_iter();
            // seg is non-empty, so there is at least the goal token.
            let name = tokens.next().expect("non-empty dependency has a goal");
            let path = parse_name_path(&name, lineno)?;
            let args: Vec<String> = tokens.collect();
            deps.push(Dep { path, args });
        }
    }

    Ok((path, params, deps))
}

pub fn parse(src: &str) -> Result<Vafile, ParseError> {
    let mut vafile = Vafile::default();
    let lines: Vec<&str> = src.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let raw = lines[i];
        let lineno = i + 1;

        // Skip blank lines and full-line comments at column 0.
        if raw.trim().is_empty() || raw.trim_start().starts_with('#') && leading_spaces(raw) == 0 {
            i += 1;
            continue;
        }

        // A header must start at column 0 (no indentation).
        if leading_spaces(raw) != 0 {
            return Err(ParseError {
                line: lineno,
                message: "unexpected indented line outside a recipe body".to_string(),
            });
        }

        // An `import "path" [as ns]` directive (a top-level line, no body).
        if looks_like_import(raw) {
            vafile.imports.push(parse_import(raw, lineno)?);
            i += 1;
            continue;
        }

        let (path, params, deps) = parse_header(raw, lineno)?;

        // Collect the body: subsequent lines indented > 0, until a non-indented
        // non-blank line.
        let mut body_raw: Vec<&str> = Vec::new();
        let mut j = i + 1;
        while j < lines.len() {
            let bl = lines[j];
            if bl.trim().is_empty() {
                body_raw.push(""); // preserve blank lines inside body
                j += 1;
                continue;
            }
            if leading_spaces(bl) == 0 {
                break; // next header
            }
            body_raw.push(bl);
            j += 1;
        }

        // Trim trailing blank lines from body.
        while matches!(body_raw.last(), Some(l) if l.trim().is_empty()) {
            body_raw.pop();
        }

        // De-indent: strip the minimum indentation of non-blank body lines.
        let min_indent = body_raw
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| leading_spaces(l))
            .min()
            .unwrap_or(0);
        let body: Vec<String> = body_raw
            .iter()
            .map(|l| {
                if l.len() >= min_indent {
                    l[min_indent..].to_string()
                } else {
                    l.to_string()
                }
            })
            .collect();

        let key = path.join("::");
        if vafile.recipes.contains_key(&key) {
            return Err(ParseError {
                line: lineno,
                message: format!("duplicate recipe `{}`", key),
            });
        }

        vafile.recipes.insert(
            key,
            Recipe {
                path,
                params,
                deps,
                body,
                line: lineno,
                source: "vafile".to_string(),
            },
        );

        i = j;
    }

    Ok(vafile)
}
