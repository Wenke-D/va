#[path = "../src/parser.rs"]
mod parser;
#[path = "../src/validate.rs"]
mod validate;

use validate::{plan, validate as run_validate, ValidateError};

fn vf(src: &str) -> parser::Vafile {
    parser::parse(src).expect("parse ok")
}

fn path(s: &[&str]) -> Vec<String> {
    s.iter().map(|x| x.to_string()).collect()
}

fn dep(p: &[&str], args: &[&str]) -> parser::Dep {
    parser::Dep {
        path: path(p),
        args: args.iter().map(|a| a.to_string()).collect(),
    }
}

#[test]
fn parses_deps_in_order() {
    let v = vf("build: configure, docker::build\n    echo go\n");
    let r = v.get(&path(&["build"])).unwrap();
    assert_eq!(r.deps, vec![dep(&["configure"], &[]), dep(&["docker", "build"], &[])]);
}

#[test]
fn parses_dep_arguments() {
    let v = vf("ci: test integration, lint\n    echo go\n");
    let r = v.get(&path(&["ci"])).unwrap();
    assert_eq!(r.deps, vec![dep(&["test"], &["integration"]), dep(&["lint"], &[])]);
}

#[test]
fn header_without_deps_has_empty_deps() {
    let v = vf("greet name:\n    echo hi {{name}}\n");
    let r = v.get(&path(&["greet"])).unwrap();
    assert!(r.deps.is_empty());
}

#[test]
fn valid_dag_passes() {
    let v = vf("configure:\n    echo c\ncompile:\n    echo k\nbuild: configure, compile\n    echo b\n");
    assert!(run_validate(&v).is_ok());
}

#[test]
fn plan_is_deps_first_deduped_root_last() {
    // build -> {configure, compile}; compile -> configure.
    // configure must run once, before compile, before build.
    let v = vf(
        "configure:\n    echo c\ncompile: configure\n    echo k\nbuild: configure, compile\n    echo b\n",
    );
    let order = plan(&v, &path(&["build"]), &[]);
    let paths: Vec<Vec<String>> = order.iter().map(|n| n.path.clone()).collect();
    assert_eq!(
        paths,
        vec![path(&["configure"]), path(&["compile"]), path(&["build"])]
    );
}

#[test]
fn unknown_dependency_errors() {
    let v = vf("build: configre\n    echo b\n");
    let errs = run_validate(&v).unwrap_err();
    assert!(errs
        .iter()
        .any(|e| matches!(e, ValidateError::UnknownDependency { dep, .. } if dep == "configre")));
}

#[test]
fn dependency_with_required_args_errors() {
    let v = vf("greet name:\n    echo {{name}}\nbuild: greet\n    echo b\n");
    let errs = run_validate(&v).unwrap_err();
    assert!(errs.iter().any(|e| matches!(
        e,
        ValidateError::DependencyNeedsArgs { dep, required, .. }
            if dep == "greet" && required == &vec!["name".to_string()]
    )));
}

#[test]
fn dependency_on_namespace_without_default_errors() {
    let v = vf("docker::build:\n    echo d\nrelease: docker\n    echo r\n");
    let errs = run_validate(&v).unwrap_err();
    assert!(errs
        .iter()
        .any(|e| matches!(e, ValidateError::DependencyIsNamespace { dep, .. } if dep == "docker")));
}

#[test]
fn self_loop_is_a_cycle() {
    let v = vf("build: build\n    echo b\n");
    let errs = run_validate(&v).unwrap_err();
    assert!(matches!(errs.as_slice(), [ValidateError::Cycle { .. }]));
}

#[test]
fn multi_node_cycle_is_detected() {
    let v = vf("a: b\n    echo a\nb: c\n    echo b\nc: a\n    echo c\n");
    let errs = run_validate(&v).unwrap_err();
    match errs.as_slice() {
        [ValidateError::Cycle { path, .. }] => {
            // Closed loop: first and last node match, and all three appear.
            assert_eq!(path.first(), path.last());
            assert!(["a", "b", "c"].iter().all(|n| path.contains(&n.to_string())));
        }
        other => panic!("expected a single cycle error, got {:?}", other),
    }
}

#[test]
fn error_carries_the_declaring_line() {
    // `build`'s header is on line 3; its bad dep should report line 3.
    let v = vf("configure:\n    echo c\nbuild: nope\n    echo b\n");
    let errs = run_validate(&v).unwrap_err();
    assert!(errs
        .iter()
        .any(|e| matches!(e, ValidateError::UnknownDependency { line: 3, dep, .. } if dep == "nope")));
}

#[test]
fn multiple_resolution_errors_aggregate() {
    let v = vf("build: nope1, nope2\n    echo b\n");
    let errs = run_validate(&v).unwrap_err();
    let unknown = errs
        .iter()
        .filter(|e| matches!(e, ValidateError::UnknownDependency { .. }))
        .count();
    assert_eq!(unknown, 2);
}

#[test]
fn dep_passes_literal_args() {
    let v = vf("test name:\n    echo {{name}}\nci: test integration\n    echo done\n");
    assert!(run_validate(&v).is_ok());
    let order = plan(&v, &path(&["ci"]), &[]);
    assert_eq!(order[0].path, path(&["test"]));
    assert_eq!(order[0].args, vec![("name".to_string(), "integration".to_string())]);
    assert_eq!(order.last().unwrap().path, path(&["ci"]));
}

#[test]
fn dep_forwards_parent_param() {
    // `check`'s own `suite` arg flows into its `test {{suite}}` dependency.
    let v = vf("test name:\n    echo {{name}}\ncheck suite: test {{suite}}\n    echo checked\n");
    assert!(run_validate(&v).is_ok());
    let order = plan(&v, &path(&["check"]), &[("suite".to_string(), "foo".to_string())]);
    assert_eq!(order[0].path, path(&["test"]));
    assert_eq!(order[0].args, vec![("name".to_string(), "foo".to_string())]);
}

#[test]
fn same_dep_different_args_runs_each() {
    let v = vf("build t:\n    echo {{t}}\nall: build x86, build arm\n    echo all\n");
    assert!(run_validate(&v).is_ok());
    let order = plan(&v, &path(&["all"]), &[]);
    let builds: Vec<&validate::PlanNode> =
        order.iter().filter(|n| n.path == path(&["build"])).collect();
    assert_eq!(builds.len(), 2);
    assert_eq!(builds[0].args, vec![("t".to_string(), "x86".to_string())]);
    assert_eq!(builds[1].args, vec![("t".to_string(), "arm".to_string())]);
}

#[test]
fn same_dep_same_args_runs_once() {
    let v = vf("build t:\n    echo {{t}}\nall: build x86, build x86\n    echo all\n");
    let order = plan(&v, &path(&["all"]), &[]);
    let builds = order.iter().filter(|n| n.path == path(&["build"])).count();
    assert_eq!(builds, 1);
}

#[test]
fn dep_too_many_args_errors() {
    let v = vf("test name:\n    echo {{name}}\nci: test a b\n    echo x\n");
    let errs = run_validate(&v).unwrap_err();
    assert!(errs
        .iter()
        .any(|e| matches!(e, ValidateError::DependencyTooManyArgs { got: 2, max: 1, .. })));
}

#[test]
fn dep_unknown_param_reference_errors() {
    // `ci` has no params, so `{{nope}}` in its dependency arg is undeclared.
    let v = vf("test name:\n    echo {{name}}\nci: test {{nope}}\n    echo x\n");
    let errs = run_validate(&v).unwrap_err();
    assert!(errs
        .iter()
        .any(|e| matches!(e, ValidateError::DependencyUnknownParam { param, .. } if param == "nope")));
}
