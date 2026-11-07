use super::*;

fn file(path: &str, removed: &[&str], added: &[&str]) -> String {
    let mut d = format!(
        "diff --git a/{path} b/{path}\nindex 1..2 100644\n--- a/{path}\n+++ b/{path}\n@@ -1,3 +1,3 @@\n"
    );
    for r in removed {
        d.push_str(&format!("-{r}\n"));
    }
    for a in added {
        d.push_str(&format!("+{a}\n"));
    }
    d.push_str(" }\n");
    d
}

fn deleted(path: &str, lines: &[&str]) -> String {
    let mut d = format!(
        "diff --git a/{path} b/{path}\ndeleted file mode 100644\nindex 1..0\n--- a/{path}\n+++ /dev/null\n@@ -1,2 +0,0 @@\n"
    );
    for l in lines {
        d.push_str(&format!("-{l}\n"));
    }
    d
}

fn created(path: &str, lines: &[&str]) -> String {
    let mut d = format!(
        "diff --git a/{path} b/{path}\nnew file mode 100644\nindex 0..1\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,2 @@\n"
    );
    for l in lines {
        d.push_str(&format!("+{l}\n"));
    }
    d
}

fn kinds(diff: &str) -> Vec<WeakKind> {
    detect(diff, &[]).into_iter().map(|w| w.kind).collect()
}

fn more(diff: &str) -> Vec<WeakKind> {
    detect_more(diff).into_iter().map(|w| w.kind).collect()
}

#[test]
fn a_deleted_test_file_is_found_but_a_moved_one_is_not() {
    let gone = deleted(
        "tests/search.rs",
        &["#[test]", "fn finds() { assert_eq!(f(), 1); }"],
    );
    assert_eq!(more(&gone), vec![WeakKind::TestFileDeleted]);
    let moved = format!(
        "{}{}",
        gone,
        created(
            "tests/find.rs",
            &["#[test]", "fn finds() { assert_eq!(f(), 1); }"]
        )
    );
    assert!(more(&moved).is_empty(), "{:?}", detect_more(&moved));
}

#[test]
fn removed_test_functions_per_framework() {
    let cases: [(&str, &[&str]); 6] = [
        ("tests/a.rs", &["#[test]", "fn a() {}"]),
        ("tests/test_a.py", &["def test_a():", "    pass"]),
        ("src/a.test.ts", &["it('does a', () => {", "});"]),
        ("a_test.go", &["func TestA(t *testing.T) {", "}"]),
        ("test/a_test.dart", &["testWidgets('a', (t) async {", "});"]),
        ("src/ATest.java", &["@Test", "void a() {}"]),
    ];
    for (path, removed) in cases {
        let d = file(path, removed, &[]);
        assert!(
            more(&d).contains(&WeakKind::TestsRemoved),
            "{path}: {:?}",
            detect_more(&d)
        );
    }
}

#[test]
fn a_renamed_or_added_test_is_not_a_removal() {
    let renamed = file(
        "tests/test_a.py",
        &["def test_old_name():", "    assert f() == 1"],
        &["def test_new_name():", "    assert f() == 1"],
    );
    assert!(more(&renamed).is_empty(), "{:?}", detect_more(&renamed));
    let added = file(
        "tests/a.rs",
        &[],
        &["#[test]", "fn also() { assert_eq!(f(), 1); }"],
    );
    assert!(more(&added).is_empty());
    let new_file = created("tests/b.rs", &["#[test]", "fn b() { assert!(true); }"]);
    assert!(more(&new_file).is_empty());
}

#[test]
fn switched_off_tests_per_framework() {
    let cases: [(&str, &str); 8] = [
        ("tests/a.rs", "#[ignore]"),
        ("tests/test_a.py", "@pytest.mark.skip(reason='later')"),
        ("tests/test_a.py", "@pytest.mark.xfail"),
        ("src/a.test.ts", "it.skip('does a', () => {"),
        ("src/a.spec.ts", "it.only('does a', () => {"),
        ("a_test.go", "t.Skip(\"flaky\")"),
        ("test/a_test.dart", "test('a', () {}, skip: true);"),
        ("src/ATest.java", "@Disabled"),
    ];
    for (path, line) in cases {
        let d = file(path, &[], &[line]);
        let found = kinds(&d);
        assert!(
            found.contains(&WeakKind::Skipped),
            "{path} {line}: {found:?}"
        );
    }
}

#[test]
fn dropped_assertions_are_counted_but_a_rewrite_of_equal_strength_is_not() {
    let dropped = file(
        "tests/a.rs",
        &["    assert_eq!(f(), 1);", "    assert_eq!(g(), 2);"],
        &["    assert_eq!(f(), 1);"],
    );
    assert!(more(&dropped).contains(&WeakKind::AssertionsDropped));
    let rewritten = file(
        "tests/a.rs",
        &["    assert_eq!(f(), 1);"],
        &["    assert_eq!(1, f());"],
    );
    assert!(more(&rewritten).is_empty(), "{:?}", detect_more(&rewritten));
    let more_assertions = file(
        "tests/a.rs",
        &["    assert_eq!(f(), 1);"],
        &["    assert_eq!(f(), 1);", "    assert_eq!(g(), 2);"],
    );
    assert!(more(&more_assertions).is_empty());
}

#[test]
fn loosened_matchers_per_framework() {
    for (path, was, now) in [
        ("tests/a.rs", "assert_eq!(f(), 3);", "assert!(true);"),
        ("tests/test_a.py", "assert f() == 3", "assert True"),
        (
            "src/a.test.ts",
            "expect(f()).toBe(3);",
            "expect(f()).toBeTruthy();",
        ),
        (
            "src/a.test.ts",
            "expect(f()).toEqual({a: 1});",
            "expect(f()).toEqual(expect.anything());",
        ),
        (
            "tests/test_a.py",
            "self.assertEqual(f(), 3)",
            "self.assertIsNotNone(f())",
        ),
    ] {
        let d = file(path, &[was], &[now]);
        assert!(more(&d).contains(&WeakKind::LooseMatcher), "{path} {now}");
    }
}

#[test]
fn snapshots_and_goldens_changed_are_found_new_ones_are_not() {
    for path in [
        "src/__snapshots__/a.test.ts.snap",
        "tests/golden/out.txt",
        "testdata/search.golden",
        "tests/__snapshots__/test_a.ambr",
    ] {
        assert!(
            more(&file(path, &["old"], &["new"])).contains(&WeakKind::SnapshotEdited),
            "{path}"
        );
    }
    assert!(more(&created("tests/golden/new.txt", &["x"])).is_empty());
    assert!(more(&deleted("tests/golden/old.txt", &["x"])).contains(&WeakKind::SnapshotEdited));
}

#[test]
fn tolerances_widened_are_found_tightened_are_not() {
    for (path, was, now) in [
        (
            "tests/test_a.py",
            "assert x == pytest.approx(1.0, abs=1e-6)",
            "assert x == pytest.approx(1.0, abs=1e-2)",
        ),
        (
            "tests/test_a.py",
            "assert math.isclose(x, 1, rel_tol=0.001)",
            "assert math.isclose(x, 1, rel_tol=0.1)",
        ),
        (
            "tests/test_a.py",
            "np.testing.assert_allclose(a, b, rtol=1e-7)",
            "np.testing.assert_allclose(a, b, rtol=1e-3)",
        ),
        (
            "tests/a.rs",
            "assert_relative_eq!(a, b, epsilon = 1e-9);",
            "assert_relative_eq!(a, b, epsilon = 1e-3);",
        ),
    ] {
        let d = file(path, &[was], &[now]);
        assert!(more(&d).contains(&WeakKind::ToleranceRelaxed), "{now}");
    }
    let tighter = file(
        "tests/test_a.py",
        &["assert math.isclose(x, 1, rel_tol=0.1)"],
        &["assert math.isclose(x, 1, rel_tol=0.001)"],
    );
    assert!(!more(&tighter).contains(&WeakKind::ToleranceRelaxed));
}

#[test]
fn ci_and_test_configuration_weakened_is_found_a_stricter_step_is_not() {
    let dropped_step = file(
        ".github/workflows/ci.yml",
        &["        run: cargo test --workspace"],
        &["        run: cargo build --workspace"],
    );
    assert!(more(&dropped_step).contains(&WeakKind::ConfigChanged));
    let tolerant = file(
        ".github/workflows/ci.yml",
        &[],
        &["        continue-on-error: true"],
    );
    assert!(more(&tolerant).contains(&WeakKind::ConfigChanged));
    let threshold = file("pyproject.toml", &["fail_under = 90"], &["fail_under = 50"]);
    assert!(more(&threshold).contains(&WeakKind::ConfigChanged));
    let jest = file(
        "package.json",
        &["    \"test\": \"jest\""],
        &["    \"test\": \"echo skipped\""],
    );
    assert!(more(&jest).contains(&WeakKind::ConfigChanged));
    let stricter = file(
        ".github/workflows/ci.yml",
        &[],
        &["        run: cargo test --workspace --all-features"],
    );
    assert!(more(&stricter).is_empty());
    let dep = file(
        "package.json",
        &["    \"react\": \"18.2.0\""],
        &["    \"react\": \"18.3.1\""],
    );
    assert!(more(&dep).is_empty());
}

#[test]
fn ordinary_work_is_quiet() {
    let work = format!(
        "{}{}{}",
        file("src/lib.rs", &["fn a() {}"], &["fn a() { b() }"]),
        created(
            "tests/new.rs",
            &["#[test]", "fn n() { assert_eq!(a(), 1); }"]
        ),
        file("README.md", &["old"], &["new #[ignore]"]),
    );
    assert!(detect(&work, &[]).is_empty(), "{:?}", detect(&work, &[]));
}

#[test]
fn allowed_paths_are_never_reported() {
    let gone = deleted(
        "tests/legacy/old.rs",
        &["#[test]", "fn old() { assert!(x()); }"],
    );
    assert_eq!(kinds(&gone), vec![WeakKind::TestFileDeleted]);
    let allow = vec!["tests/legacy/**".to_string()];
    assert!(detect(&gone, &allow).is_empty());
    let other = deleted("tests/live.rs", &["#[test]", "fn live() { assert!(x()); }"]);
    assert_eq!(detect(&other, &allow).len(), 1);
}

#[test]
fn findings_say_where_and_what() {
    let d = file("tests/a.rs", &[], &["#[ignore]"]);
    let w = &detect(&d, &[])[0];
    assert_eq!(
        w.describe(),
        "tests/a.rs: a test was switched off (`#[ignore]`)"
    );
}

#[test]
fn findings_affect_the_checks_when_protected_named_or_a_suite_runs() {
    let check = GateCheck::new("tests", "cargo", &["test", "--test", "search"]);
    let w = |kind, path: &str| weak(kind, path, "");
    let protected = vec!["tests/oracles/**".to_string()];
    assert!(w(WeakKind::Skipped, "tests/oracles/a.rs").affects_checks(&protected, &[], false));
    assert!(
        w(WeakKind::ConfigChanged, "tests/search.rs").affects_checks(
            &[],
            std::slice::from_ref(&check),
            false
        )
    );
    assert!(w(WeakKind::Skipped, "tests/b.rs").affects_checks(
        &[],
        std::slice::from_ref(&check),
        false
    ));
    assert!(
        !w(WeakKind::ConfigChanged, ".github/workflows/ci.yml").affects_checks(
            &[],
            std::slice::from_ref(&check),
            false
        )
    );
    assert!(!w(WeakKind::Skipped, "tests/b.rs").affects_checks(&[], &[], false));
}
