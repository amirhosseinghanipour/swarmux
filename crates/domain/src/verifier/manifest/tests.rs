use super::*;

fn change(path: &str, old: &str, new: &str) -> ManifestChange {
    analyze(path, Some(old), Some(new)).expect("a manifest")
}

#[test]
fn cargo_dependencies_are_not_harness_but_everything_else_is() {
    let old = "[package]\nname = \"a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nserde = \"1\"\n\n[dev-dependencies]\ntempfile = \"3\"\n";
    let c = change(
        "Cargo.toml",
        old,
        "[package]\nname = \"a\"\nversion = \"0.2.0\"\nedition = \"2021\"\n\n[dependencies]\nserde = { version = \"1\", features = [\"derive\"] }\nleftpad = \"2\"\n\n[dev-dependencies]\ntempfile = \"3\"\n",
    );
    assert_eq!(c.harness, Vec::<String>::new());
    assert_eq!(c.dependencies, ["leftpad", "serde"]);

    for (what, new, says) in [
        (
            "patch",
            format!("{old}\n[patch.crates-io]\nserde = {{ path = \"../x\" }}\n"),
            "[patch]",
        ),
        (
            "replace",
            format!("{old}\n[replace]\n\"serde:1.0.0\" = {{ path = \"x\" }}\n"),
            "[patch]",
        ),
        (
            "build script",
            old.replace(
                "edition = \"2021\"",
                "edition = \"2021\"\nbuild = \"evil.rs\"",
            ),
            "`build`",
        ),
        (
            "test target",
            format!("{old}\n[[test]]\nname = \"t\"\nharness = false\n"),
            "test",
        ),
        (
            "profile",
            format!("{old}\n[profile.test]\nopt-level = 3\n"),
            "profile.test",
        ),
        ("edition", old.replace("2021", "2018"), "`edition`"),
    ] {
        let c = change("Cargo.toml", old, &new);
        assert!(
            c.harness.iter().any(|h| h.contains(says)),
            "{what}: {:?}",
            c.harness
        );
    }
}

#[test]
fn a_dependency_subtable_is_a_dependency() {
    let c = change(
        "Cargo.toml",
        "[package]\nname = \"a\"\n",
        "[package]\nname = \"a\"\n\n[dependencies.leftpad]\nversion = \"2\"\nfeatures = [\"x\"]\n\n[target.'cfg(unix)'.dependencies]\nlibc = \"0.2\"\n",
    );
    assert!(c.harness.is_empty(), "{:?}", c.harness);
    assert_eq!(c.dependencies, ["leftpad", "libc"]);
}

#[test]
fn pyproject_dependencies_and_pytest_configuration() {
    let old = "[project]\nname = \"a\"\nversion = \"1\"\ndependencies = [\n  \"requests>=2\",\n]\n\n[tool.pytest.ini_options]\naddopts = \"-q\"\n";
    let c = change(
        "pyproject.toml",
        old,
        "[project]\nname = \"a\"\nversion = \"2\"\ndependencies = [\n  \"requests>=2\",\n  \"leftpad==2.0\",\n]\n\n[tool.pytest.ini_options]\naddopts = \"-q\"\n",
    );
    assert!(c.harness.is_empty(), "{:?}", c.harness);
    assert_eq!(c.dependencies, ["leftpad"]);

    let c = change("pyproject.toml", old, &old.replace("-q", "-q -p evil"));
    assert_eq!(c.harness, ["[tool.pytest.ini_options] `addopts`"]);
    let c = change(
        "pyproject.toml",
        old,
        &format!("{old}\n[project.entry-points.pytest11]\nevil = \"evil.plugin\"\n"),
    );
    assert!(c.harness[0].contains("pytest11"), "{:?}", c.harness);
}

#[test]
fn package_json_scripts_and_test_configuration_are_harness() {
    let old = r#"{"name":"a","version":"1.0.0","scripts":{"test":"jest","build":"tsc"},"dependencies":{"x":"1"},"jest":{"testPathIgnorePatterns":[]}}"#;
    let c = change(
        "package.json",
        old,
        r#"{"name":"a","version":"1.0.1","scripts":{"test":"jest","build":"tsc"},"dependencies":{"x":"1","y":"2"},"jest":{"testPathIgnorePatterns":[]}}"#,
    );
    assert!(c.harness.is_empty(), "{:?}", c.harness);
    assert_eq!(c.dependencies, ["y"]);

    let c = change(
        "package.json",
        old,
        &old.replace("\"test\":\"jest\"", "\"test\":\"true\""),
    );
    assert_eq!(c.harness, ["`scripts.test`"]);
    let c = change(
        "package.json",
        old,
        &old.replace(
            "\"testPathIgnorePatterns\":[]",
            "\"testPathIgnorePatterns\":[\"/src/\"]",
        ),
    );
    assert_eq!(c.harness, ["`jest.testPathIgnorePatterns`"]);
    let c = change("package.json", old, "{not json");
    assert_eq!(c.harness, ["package.json does not parse"]);
}

#[test]
fn pubspec_overrides_are_harness_and_dependencies_are_not() {
    let old = "name: app\nversion: 1.0.0\nenvironment:\n  sdk: ^3.0.0\ndependencies:\n  http: ^1.0.0\ndev_dependencies:\n  test: ^1.0.0\nflutter:\n  uses-material-design: true\n";
    let c = change(
        "pubspec.yaml",
        old,
        &old.replace("  http: ^1.0.0\n", "  http: ^1.2.0\n  leftpad: ^2.0.0\n")
            .replace("version: 1.0.0", "version: 1.1.0"),
    );
    assert!(c.harness.is_empty(), "{:?}", c.harness);
    assert_eq!(c.dependencies, ["http", "leftpad"]);
    let c = change(
        "pubspec.yaml",
        old,
        &format!("{old}dependency_overrides:\n  test:\n    path: ../evil\n"),
    );
    assert!(
        c.harness[0].contains("dependency_overrides"),
        "{:?}",
        c.harness
    );
}

#[test]
fn go_mod_requires_are_dependencies_and_replacements_are_harness() {
    let old = "module a\n\ngo 1.22\n\nrequire (\n\tgithub.com/x/y v1.0.0\n)\n";
    let c = change(
        "go.mod",
        old,
        "module a\n\ngo 1.22\n\nrequire (\n\tgithub.com/x/y v1.0.0\n\tgithub.com/z/w v2.0.0\n)\n",
    );
    assert!(c.harness.is_empty(), "{:?}", c.harness);
    assert_eq!(c.dependencies, ["github.com/z/w"]);
    let c = change(
        "go.mod",
        old,
        &format!("{old}replace github.com/x/y => ../evil\n"),
    );
    assert_eq!(c.harness, ["`replace` redirects a module"]);
}

#[test]
fn unknown_files_and_missing_sides_are_not_analysed() {
    assert_eq!(analyze("conftest.py", Some("a"), Some("b")), None);
    assert_eq!(analyze("Cargo.toml", None, Some("a")), None);
    assert_eq!(analyze("Cargo.toml", Some("a"), None), None);
}

fn lock(path: &str, added: &[&str], removed: &[&str]) -> Vec<String> {
    let f = FileDiff {
        path: path.into(),
        added: added.iter().map(|s| s.to_string()).collect(),
        removed: removed.iter().map(|s| s.to_string()).collect(),
        ..FileDiff::default()
    };
    lock_dependencies(&f).expect("a lockfile")
}

#[test]
fn lockfiles_name_the_packages_they_gained() {
    assert_eq!(
        lock(
            "Cargo.lock",
            &["name = \"new\"", "name = \"bumped\"", "version = \"2\""],
            &["name = \"bumped\""]
        ),
        ["new"]
    );
    assert_eq!(
        lock(
            "package-lock.json",
            &[
                "    \"node_modules/leftpad\": {",
                "    \"node_modules/a/node_modules/@s/pkg\": {",
                "      \"version\": \"1\""
            ],
            &[]
        ),
        ["@s/pkg", "leftpad"]
    );
    assert_eq!(
        lock(
            "pnpm-lock.yaml",
            &["  /leftpad@1.0.0:", "  bumped@2.0.0:"],
            &["  bumped@1.0.0:"]
        ),
        ["leftpad"]
    );
    assert_eq!(
        lock(
            "yarn.lock",
            &[
                "leftpad@^1.0.0:",
                "\"@s/pkg@^2.0.0\", \"@s/pkg@^2.1.0\":",
                "  version \"1\""
            ],
            &[]
        ),
        ["@s/pkg", "leftpad"]
    );
    assert_eq!(
        lock("poetry.lock", &["name = \"leftpad\""], &[]),
        ["leftpad"]
    );
    assert_eq!(
        lock(
            "go.sum",
            &[
                "github.com/x/y v1.0.0 h1:abc=",
                "github.com/x/y v1.0.0/go.mod h1:def="
            ],
            &[]
        ),
        ["github.com/x/y"]
    );
    assert_eq!(
        lock(
            "pubspec.lock",
            &["  leftpad:", "    dependency: direct"],
            &[]
        ),
        ["leftpad"]
    );
    assert_eq!(
        lock(
            "requirements.txt",
            &["leftpad==2.0", "-r other.txt", "# c", "Requests>=2"],
            &["requests>=1"]
        ),
        ["leftpad"]
    );
    assert_eq!(
        lock("sub/requirements-dev.txt", &["pytest"], &[]),
        ["pytest"]
    );
    assert_eq!(
        lock_dependencies(&FileDiff {
            path: "src/lib.rs".into(),
            ..FileDiff::default()
        }),
        None
    );
}
