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

/// A parsed recipe (a "goal").
#[derive(Debug, Clone)]
pub struct Recipe {
    /// Full dotted path, e.g. ["docker", "build"] for `docker::build`.
    pub path: Vec<String>,
    pub params: Vec<Param>,
    /// Dependency goal references, in declaration order. Each is a path, e.g.
    /// `build: configure docker::build` -> [["configure"], ["docker", "build"]].
    /// Run (deduped, deps-first) before this recipe's body. They take no args.
    pub deps: Vec<Vec<String>>,
    /// Raw body lines (already de-indented), executed as a shell script.
    pub body: Vec<String>,
    /// Source line number of the header (1-based), for error messages.
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

/// The parsed model: an ordered collection of recipes keyed by their path.
#[derive(Debug, Default)]
pub struct Vafile {
    /// path (joined by "::") -> Recipe
    pub recipes: BTreeMap<String, Recipe>,
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

/// Parse a recipe header `NAME params... : deps...` into (path, params, deps).
///
/// The separator is the first "lone" `:` — one not adjacent to another colon,
/// so it is never confused with a `::` inside a name. Everything left of it is
/// the name and its parameters; everything right is the dependency list.
fn parse_header(
    line: &str,
    lineno: usize,
) -> Result<(Vec<String>, Vec<Param>, Vec<Vec<String>>), ParseError> {
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

    // Deps (right of `:`). Each is a goal reference; they take no arguments.
    let mut deps = Vec::new();
    for tok in right.split_whitespace() {
        if tok.contains('?') {
            return Err(ParseError {
                line: lineno,
                message: format!("dependency `{}` cannot be optional", tok),
            });
        }
        deps.push(parse_name_path(tok, lineno)?);
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
            },
        );

        i = j;
    }

    Ok(vafile)
}
