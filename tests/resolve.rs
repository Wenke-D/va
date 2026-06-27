#[path = "../src/parser.rs"]
mod parser;
#[path = "../src/resolver.rs"]
mod resolver;

use resolver::ResolveError;

fn vf(src: &str) -> parser::Vafile { parser::parse(src).expect("parse ok") }
fn toks(s: &[&str]) -> Vec<String> { s.iter().map(|x| x.to_string()).collect() }

const SAMPLE: &str = r#"
build:
    cmake --build

build::release:
    make --build --preset release

docker::build:
    docker build -t app .

docker::push:
    docker push app

greet name:
    echo hello {{name}}
"#;

#[test]
fn default_goal_runs() {
    let v = vf(SAMPLE);
    let r = resolver::resolve(&v, &toks(&["build"])).unwrap();
    assert_eq!(r.recipe.display_name(), "build");
}

#[test]
fn subgoal_path_wins_and_shadows() {
    let v = vf(SAMPLE);
    let r = resolver::resolve(&v, &toks(&["build", "release"])).unwrap();
    assert_eq!(r.recipe.display_name(), "build::release");
    assert!(r.args.is_empty());
}

#[test]
fn unknown_token_after_no_param_goal_errors() {
    let v = vf(SAMPLE);
    let err = resolver::resolve(&v, &toks(&["build", "xxx"])).unwrap_err();
    match err {
        ResolveError::NotSubcommandNorParam { token, goal, takes_args } => {
            assert_eq!(token, "xxx");
            assert_eq!(goal, "build");
            assert!(!takes_args);
        }
        other => panic!("wrong error: {:?}", other),
    }
}

#[test]
fn namespace_descend() {
    let v = vf(SAMPLE);
    let r = resolver::resolve(&v, &toks(&["docker", "build"])).unwrap();
    assert_eq!(r.recipe.display_name(), "docker::build");
}

#[test]
fn bare_namespace_without_default_errors() {
    let v = vf(SAMPLE);
    let err = resolver::resolve(&v, &toks(&["docker"])).unwrap_err();
    match err {
        ResolveError::NamespaceNeedsSubcommand { path, available } => {
            assert_eq!(path, vec!["docker".to_string()]);
            assert!(available.contains(&"build".to_string()));
            assert!(available.contains(&"push".to_string()));
        }
        other => panic!("wrong error: {:?}", other),
    }
}

#[test]
fn param_binding() {
    let v = vf(SAMPLE);
    let r = resolver::resolve(&v, &toks(&["greet", "world"])).unwrap();
    assert_eq!(r.args, vec![("name".to_string(), "world".to_string())]);
}

#[test]
fn missing_required_param_errors() {
    let v = vf(SAMPLE);
    let err = resolver::resolve(&v, &toks(&["greet"])).unwrap_err();
    assert!(matches!(err, ResolveError::MissingParams { .. }));
}

#[test]
fn dashdash_forbidden() {
    let v = vf(SAMPLE);
    let err = resolver::resolve(&v, &toks(&["build", "--", "x"])).unwrap_err();
    assert!(matches!(err, ResolveError::DashDashForbidden));
}

#[test]
fn unknown_top_level_errors() {
    let v = vf(SAMPLE);
    let err = resolver::resolve(&v, &toks(&["nope"])).unwrap_err();
    match err {
        ResolveError::UnknownCommand { token, .. } => assert_eq!(token, "nope"),
        other => panic!("wrong error: {:?}", other),
    }
}
