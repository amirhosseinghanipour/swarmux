use super::detect::{just_recipes, make_targets};
use super::*;

fn listing(files: &[&str], texts: &[(&str, &str)]) -> RepoListing {
    RepoListing {
        files: files.iter().map(|f| f.to_string()).collect(),
        texts: texts
            .iter()
            .map(|(p, t)| (p.to_string(), t.to_string()))
            .collect(),
    }
}

fn lines(d: &Detected) -> Vec<&str> {
    d.commands.iter().map(|c| c.line.as_str()).collect()
}

fn repo(name: &str, path: &str) -> Repo {
    Repo {
        name: name.into(),
        path: path.into(),
        git_root: format!("/w/{path}").into(),
        default_branch: Some("main".into()),
        stacks: Vec::new(),
        commands: Vec::new(),
    }
}

fn workspace() -> Workspace {
    let mut w = Workspace::new(WorkspaceId::random(), " Product ", "/w".into(), 1).unwrap();
    w.set_repos(
        vec![
            repo("app", "app"),
            repo("backend", "backend"),
            repo("admin", "admin"),
        ],
        2,
    );
    w
}

#[test]
fn flutter_and_dart_are_told_apart_by_the_pubspec() {
    let flutter = detect(&listing(
        &["pubspec.yaml", "lib/"],
        &[(
            "pubspec.yaml",
            "name: app\ndependencies:\n  flutter:\n    sdk: flutter\n",
        )],
    ));
    assert_eq!(flutter.stacks, [Stack::Flutter]);
    assert_eq!(lines(&flutter), ["flutter analyze", "flutter test"]);
    let dart = detect(&listing(
        &["pubspec.yaml"],
        &[("pubspec.yaml", "name: cli\n")],
    ));
    assert_eq!(dart.stacks, [Stack::Dart]);
    assert_eq!(lines(&dart), ["dart analyze", "dart test"]);
}

#[test]
fn node_scripts_run_with_the_lockfiles_manager() {
    let json = r#"{"name":"admin","scripts":{"dev":"vite","test":"vitest","lint":"eslint ."}}"#;
    let npm = detect(&listing(&["package.json"], &[("package.json", json)]));
    assert_eq!(npm.stacks, [Stack::Node]);
    assert_eq!(lines(&npm), ["npm run dev", "npm test", "npm run lint"]);
    assert_eq!(npm.commands[1].name, "test");
    assert_eq!(npm.commands[1].source, "package.json");
    let pnpm = detect(&listing(
        &["package.json", "pnpm-lock.yaml"],
        &[("package.json", json)],
    ));
    assert_eq!(lines(&pnpm)[1], "pnpm run test");
    let yarn = detect(&listing(
        &["package.json", "yarn.lock"],
        &[("package.json", json)],
    ));
    assert_eq!(lines(&yarn)[0], "yarn dev");
    // A manifest that does not parse still says what the stack is.
    let broken = detect(&listing(&["package.json"], &[("package.json", "{")]));
    assert_eq!(broken.stacks, [Stack::Node]);
    assert!(broken.commands.is_empty());
}

#[test]
fn rust_go_python_ruby_and_dotnet() {
    let rust = detect(&listing(&["Cargo.toml", "src/"], &[]));
    assert_eq!(rust.stacks, [Stack::Rust]);
    assert_eq!(lines(&rust), ["cargo build", "cargo test"]);
    let go = detect(&listing(&["go.mod"], &[]));
    assert_eq!(go.stacks, [Stack::Go]);
    assert_eq!(lines(&go), ["go build ./...", "go test ./..."]);
    let py = detect(&listing(&["requirements.txt"], &[]));
    assert_eq!(py.stacks, [Stack::Python]);
    assert!(py.commands.is_empty(), "no tests, no test command");
    let pytest = detect(&listing(
        &["pyproject.toml"],
        &[("pyproject.toml", "[tool.pytest.ini_options]\n")],
    ));
    assert_eq!(lines(&pytest), ["python -m pytest"]);
    let ruby = detect(&listing(&["Gemfile", "spec/a_spec.rb"], &[]));
    assert_eq!(ruby.stacks, [Stack::Ruby]);
    assert_eq!(lines(&ruby), ["bundle exec rspec"]);
    let dotnet = detect(&listing(&["src/Api/Api.csproj"], &[]));
    assert_eq!(dotnet.stacks, [Stack::DotNet]);
    assert_eq!(lines(&dotnet), ["dotnet build", "dotnet test"]);
}

#[test]
fn supabase_unity_and_godot() {
    let supa = detect(&listing(
        &["supabase/config.toml", "supabase/tests/rls.sql"],
        &[],
    ));
    assert_eq!(supa.stacks, [Stack::Supabase]);
    assert_eq!(lines(&supa), ["supabase db lint", "supabase test db"]);
    let unity = detect(&listing(&["ProjectSettings/ProjectVersion.txt"], &[]));
    assert_eq!(unity.stacks, [Stack::Unity]);
    let godot = detect(&listing(&["project.godot"], &[]));
    assert_eq!(godot.stacks, [Stack::Godot]);
    assert!(detect(&listing(&["README.md"], &[])).stacks.is_empty());
}

#[test]
fn make_and_just_targets_become_commands() {
    let make =
        "VAR := 1\n.PHONY: test\nall: build\nbuild test: deps\n\tcargo build\n%.o: %.c\nX = $(Y)\n";
    assert_eq!(make_targets(make), ["all", "build", "test"]);
    let just = "set shell := [\"bash\"]\nalias t := test\nversion := \"1\"\n# a comment\ntest *args:\n    cargo test\n_private:\n    echo\nbuild: test\n";
    assert_eq!(just_recipes(just), ["test", "build"]);
    let d = detect(&listing(
        &["Makefile", "justfile"],
        &[("Makefile", make), ("justfile", just)],
    ));
    assert!(lines(&d).contains(&"make build"));
    assert!(lines(&d).contains(&"just test"));
    assert_eq!(d.commands[0].source, "Makefile");
}

#[test]
fn stacks_mix_and_commands_are_not_repeated() {
    let d = detect(&listing(
        &["Cargo.toml", "package.json", "Makefile"],
        &[
            ("package.json", r#"{"scripts":{"test":"x"}}"#),
            ("Makefile", "test:\n\tnpm test\n"),
        ],
    ));
    assert_eq!(d.stacks, [Stack::Node, Stack::Rust]);
    assert_eq!(
        lines(&d),
        ["npm test", "cargo build", "cargo test", "make test"]
    );
}

#[test]
fn stacks_round_trip() {
    for s in Stack::ALL {
        assert_eq!(Stack::parse(s.as_str()), Some(s));
    }
    assert_eq!(Stack::parse("cobol"), None);
}

#[test]
fn workspace_names_are_checked_and_slugged() {
    let w = workspace();
    assert_eq!(w.name, "Product");
    assert_eq!(w.slug, "product");
    assert_eq!(w.updated_at, 2);
    assert_eq!(
        Workspace::new(WorkspaceId::random(), "  ", "/w".into(), 1),
        Err(WorkspaceError::EmptyName)
    );
    assert_eq!(
        Workspace::new(WorkspaceId::random(), &"x".repeat(65), "/w".into(), 1),
        Err(WorkspaceError::NameTooLong { max: 64 })
    );
    let id = w.id;
    assert_eq!(WorkspaceId::parse(&id.to_string()), Some(id));
    assert_eq!(
        w.repo_ref("app"),
        Some(RepoRef {
            workspace: id,
            repo: "app".into()
        })
    );
    assert_eq!(w.repo_ref("nope"), None);
}

#[test]
fn touched_repos_follow_the_given_order_or_are_all() {
    let w = workspace();
    let names = |r: Vec<Repo>| r.into_iter().map(|r| r.name).collect::<Vec<_>>();
    assert_eq!(names(w.touched(&[]).unwrap()), ["app", "backend", "admin"]);
    assert_eq!(
        names(w.touched(&["backend".into(), "app".into()]).unwrap()),
        ["backend", "app"]
    );
    let e = w.touched(&["web".into()]).unwrap_err();
    assert_eq!(
        e.to_string(),
        "no repository \"web\" in the workspace (it has app, backend, admin)"
    );
    assert_eq!(
        w.touched(&["app".into(), "app".into()]),
        Err(WorkspaceError::RepeatedRepo("app".into()))
    );
}

#[test]
fn repos_are_named_by_folder_unless_two_share_a_name() {
    let paths: Vec<String> = [".", "app", "services/api", "tools/api"]
        .map(String::from)
        .to_vec();
    assert_eq!(
        repo_names(&paths, "My Product"),
        ["my-product", "app", "services-api", "tools-api"]
    );
}

#[test]
fn repo_folders_sit_below_a_base() {
    let base = std::path::Path::new("/t");
    assert_eq!(
        repo("a", "apps/mobile").under(base),
        base.join("apps").join("mobile")
    );
    assert_eq!(repo("r", ".").under(base), base.to_path_buf());
    assert_eq!(repo("a", "apps/mobile").depth(), 2);
    assert_eq!(repo("r", ".").depth(), 0);
}

#[test]
fn checks_name_where_they_run() {
    assert_eq!(
        split_check("app-flutter: flutter test"),
        (Some("app-flutter"), "flutter test")
    );
    assert_eq!(
        split_check("  backend:   supabase db lint"),
        (Some("backend"), "supabase db lint")
    );
    assert_eq!(split_check("cargo test"), (None, "cargo test"));
    assert_eq!(
        split_check("sh -c \"a: b\""),
        (None, "sh -c \"a: b\""),
        "a colon later on is part of the command"
    );
    assert_eq!(split_check("C: dir"), (None, "C: dir"), "a drive");
    assert_eq!(split_check("app:"), (None, "app:"));
    assert_eq!(split_check("../x: y"), (None, "../x: y"));
    assert_eq!(
        split_check("apps/web: npm test"),
        (Some("apps/web"), "npm test")
    );
}
