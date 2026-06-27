//! va — a command runner.
//!
//! v0 entry point. Enforces the strict cwd rule (a `vafile` must exist in the
//! current directory), parses it, and resolves/executes the requested goal.

mod executor;
mod parser;
mod resolver;
mod validate;

use std::path::Path;
use std::process::exit;

fn print_listing(vafile: &parser::Vafile) {
    let top = vafile.children(&[]);
    if top.is_empty() {
        println!("vafile has no recipes.");
        return;
    }
    println!("Available commands:");
    for name in &top {
        let path = vec![name.clone()];
        let is_goal = vafile.get(&path).is_some();
        let is_ns = vafile.is_namespace(&path);
        let marker = match (is_goal, is_ns) {
            (true, true) => format!("{} (+ subcommands)", name),
            (false, true) => format!("{} <subcommand>", name),
            _ => name.clone(),
        };
        if let Some(r) = vafile.get(&path) {
            if !r.params.is_empty() {
                let ps: Vec<String> = r
                    .params
                    .iter()
                    .map(|p| if p.optional { format!("[{}]", p.name) } else { p.name.clone() })
                    .collect();
                println!("  {}  {}", marker, ps.join(" "));
                continue;
            }
        }
        println!("  {}", marker);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let vafile_path = Path::new("vafile");
    if !vafile_path.exists() {
        eprintln!("va: no `vafile` in the current directory");
        exit(2);
    }

    let src = match std::fs::read_to_string(vafile_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("va: cannot read vafile: {}", e);
            exit(2);
        }
    };

    let vafile = match parser::parse(&src) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("va: parse error: {}", e);
            exit(2);
        }
    };

    // Phase 1: validate the whole dependency graph before running anything.
    if let Err(errors) = validate::validate(&vafile) {
        for e in &errors {
            eprintln!("va: {}", e);
        }
        exit(2);
    }

    if args.is_empty() {
        print_listing(&vafile);
        exit(0);
    }

    match resolver::resolve(&vafile, &args) {
        Ok(resolved) => {
            let code = executor::execute(&vafile, &resolved);
            exit(code);
        }
        Err(e) => {
            eprintln!("va: {}", e);
            exit(1);
        }
    }
}
