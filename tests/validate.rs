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

#[test]
fn parses_deps_in_order() {
    let v = vf("build: configure docker::build\n    echo go\n");
    let r = v.get(&path(&["build"])).unwrap();
    assert_eq!(r.deps, vec![path(&["configure"]), path(&["docker", "build"])]);
}

#[test]
fn header_without_deps_has_empty_deps() {
    let v = vf("greet name:\n    echo hi {{name}}\n");
    let r = v.get(&path(&["greet"])).unwrap();
    assert!(r.deps.is_empty());
}

#[test]
fn valid_dag_passes() {
    let v = vf("configure:\n    echo c\ncompile:\n    echo k\nbuild: configure compile\n    echo b\n");
    assert!(run_validate(&v).is_ok());
}

#[test]
fn plan_is_deps_first_deduped_root_last() {
    // build -> {configure, compile}; compile -> configure.
    // configure must run once, before compile, before build.
    let v = vf(
        "configure:\n    echo c\ncompile: configure\n    echo k\nbuild: configure compile\n    echo b\n",
    );
    let order = plan(&v, &path(&["build"]));
    assert_eq!(
        order,
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
    let v = vf("build: nope1 nope2\n    echo b\n");
    let errs = run_validate(&v).unwrap_err();
    let unknown = errs
        .iter()
        .filter(|e| matches!(e, ValidateError::UnknownDependency { .. }))
        .count();
    assert_eq!(unknown, 2);
}
