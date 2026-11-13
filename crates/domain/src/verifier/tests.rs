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

#[test]
fn hidden_tallies_count_only() {
    let t = HiddenTally { ran: 3, passed: 2 };
    assert_eq!(t.failed(), 1);
    assert!(!t.clean());
    assert_eq!(t.text(), "2/3");
    assert!(HiddenTally::default().clean());
}

#[test]
fn names_and_paths_are_checked() {
    assert!(valid_name("my-repo_2.x").is_ok());
    for bad in ["", ".hidden", "a/b", "a\\b", "..", "a b", &"x".repeat(65)] {
        assert!(valid_name(bad).is_err(), "{bad:?}");
    }
    assert_eq!(
        repo_key("C:\\Users\\me\\Swarmux\\").as_deref(),
        Some("swarmux")
    );
    assert_eq!(repo_key("/home/me/my repo").as_deref(), Some("my-repo"));
    assert_eq!(repo_key("/home/me/.dots").as_deref(), Some("dots"));
    assert_eq!(repo_key("/"), None);
    assert_eq!(
        holdout_path("./tests\\hidden.rs").unwrap(),
        "tests/hidden.rs"
    );
    for bad in [
        "",
        "/etc/x",
        "C:/x",
        "../x",
        "a/../b",
        ".git/hooks/x",
        ".swarmux/gates.toml",
        "a//b",
    ] {
        assert!(holdout_path(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn test_paths_are_recognised() {
    for p in [
        "tests/answer.rs",
        "pkg/test/x.go",
        "src/__tests__/a.js",
        "src/answer_test.go",
        "test_answer.py",
        "src/a.test.ts",
        "src/a.spec.js",
        "src/AnswerTest.java",
        "AnswerTests.cs",
        "conftest.py",
    ] {
        assert!(is_test_path(p), "{p}");
    }
    for p in ["src/lib.rs", "src/latest.rs", "contest.py", "README.md"] {
        assert!(!is_test_path(p), "{p}");
    }
}

#[test]
fn editing_a_test_to_pass_is_caught() {
    let d = file(
        "tests/answer.rs",
        &["    assert_eq!(answer(), 42);"],
        &["    assert_eq!(answer(), 41);"],
    );
    let found = scan_diff(&d);
    assert_eq!(
        found,
        [Tamper::TestEdited {
            path: "tests/answer.rs".into(),
            lines: 1
        }]
    );
    assert!(found[0].describe().contains("1 line of existing tests"));
}

#[test]
fn an_inline_assertion_changed_is_caught() {
    let d = file(
        "src/lib.rs",
        &["        assert_eq!(answer(), 42);"],
        &["        assert_eq!(answer(), 41);"],
    );
    assert!(matches!(&scan_diff(&d)[..], [Tamper::TestEdited { .. }]));
}

#[test]
fn deleting_a_test_file_is_caught() {
    let d = "diff --git a/tests/a_test.py b/tests/a_test.py\ndeleted file mode 100644\n--- a/tests/a_test.py\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-def test_a():\n-    assert a() == 1\n";
    assert_eq!(
        scan_diff(d),
        [Tamper::TestEdited {
            path: "tests/a_test.py".into(),
            lines: 2
        }]
    );
}

#[test]
fn switching_a_test_off_is_caught_in_any_language() {
    for (path, line) in [
        ("tests/answer.rs", "#[ignore]"),
        ("src/lib.rs", "    #[ignore = \"flaky\"]"),
        ("test_a.py", "@pytest.mark.skip(reason=\"later\")"),
        ("a_test.go", "\tt.Skip(\"later\")"),
        ("src/a.test.ts", "it.skip('adds', () => {"),
        ("src/a.test.ts", "describe.only('x', () => {"),
        ("src/AnswerTest.java", "@Disabled"),
    ] {
        let found = scan_diff(&file(path, &[], &[line]));
        assert!(
            found.iter().any(|t| matches!(t, Tamper::Skipped { .. })),
            "{path}: {line}"
        );
    }
}

#[test]
fn honest_changes_are_not_tampering() {
    // New tests, moved tests, imports and prose are fine.
    let added = file(
        "tests/answer.rs",
        &[],
        &[
            "#[test]",
            "fn bigger() {",
            "    assert_eq!(answer_for(3), 9);",
            "}",
        ],
    );
    assert!(scan_diff(&added).is_empty());
    let moved = format!(
        "{}{}",
        file("tests/a.rs", &["    assert_eq!(f(), 1);"], &[]),
        file("tests/b.rs", &[], &["    assert_eq!(f(), 1);"])
    );
    assert!(scan_diff(&moved).is_empty());
    let imports = file("tests/a.rs", &["use crate::old;"], &["use crate::new;"]);
    assert!(scan_diff(&imports).is_empty());
    let docs = file("README.md", &["assert this"], &["it.skip( in prose"]);
    assert!(scan_diff(&docs).is_empty());
    let code = file("src/lib.rs", &["    41"], &["    42"]);
    assert!(scan_diff(&code).is_empty());
    // swarmux's own files are its business.
    let ours = file(".swarmux/x.toml", &["assert"], &["#[ignore]"]);
    assert!(scan_diff(&ours).is_empty());
}

#[test]
fn hunk_lines_that_look_like_headers_are_content() {
    let d = "diff --git a/tests/x.sql b/tests/x.sql\n--- a/tests/x.sql\n+++ b/tests/x.sql\n@@ -1 +1 @@\n--- a comment\n+++ new\n";
    let files = parse_diff(d);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "tests/x.sql");
    assert_eq!(files[0].removed, ["-- a comment"]);
    assert_eq!(files[0].added, ["++ new"]);
}

#[test]
fn small_plain_changes_skip_the_reviewer() {
    let d = file("src/lib.rs", &["    41"], &["    42"]);
    let r = review_risk(&d);
    assert_eq!(r.tier, RiskTier::Routine, "{}", r.summary());
    assert!(!r.warrants_review());
    assert!(r.summary().starts_with("routine"));
    assert!(!review_risk("").warrants_review());
}

#[test]
fn risky_changes_get_the_reviewer() {
    let deps = file("Cargo.toml", &[], &["evil = \"1\""]);
    let r = review_risk(&deps);
    assert!(r.warrants_review(), "{}", r.summary());
    assert!(r.reasons.iter().any(|x| x.contains("Cargo.toml")));

    let tests = file("tests/a.rs", &["    assert!(ok());"], &[]);
    assert!(review_risk(&tests).warrants_review());

    let big: Vec<String> = (0..400).map(|i| format!("    let x{i} = {i};")).collect();
    let big: Vec<&str> = big.iter().map(String::as_str).collect();
    let r = review_risk(&file("src/lib.rs", &[], &big));
    assert!(r.warrants_review(), "{}", r.summary());

    let gone = "diff --git a/src/a.rs b/src/a.rs\ndeleted file mode 100644\n--- a/src/a.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-fn a() {}\n";
    assert!(review_risk(gone).warrants_review());
}

#[test]
fn the_preamble_names_each_exploit() {
    for needle in ["tests", "skip", "special-case", "checks you cannot see"] {
        assert!(ANTI_EXPLOIT_PREAMBLE.contains(needle), "{needle}");
    }
}

#[test]
fn protected_changes_come_from_the_diff() {
    let globs = vec![".swarmux/**".to_string(), "oracles/".to_string()];
    let d = format!(
        "{}{}diff --git a/oracles/a.txt b/moved.txt\nrename from oracles/a.txt\nrename to moved.txt\ndiff --git a/oracles/new.txt b/oracles/new.txt\nnew file mode 100644\n--- /dev/null\n+++ b/oracles/new.txt\n@@ -0,0 +1 @@\n+x\n",
        file(
            ".swarmux/gates.toml",
            &["max_blocks = 3"],
            &["max_blocks = 0"]
        ),
        file(".swarmux/missions/m.toml", &["a"], &["b"]),
    );
    assert_eq!(
        protected_changes(&d, &globs, false),
        [
            PinChange::Changed(".swarmux/gates.toml".into()),
            PinChange::Removed("oracles/a.txt".into()),
            PinChange::Added("oracles/new.txt".into()),
        ]
    );
    assert!(protected_changes(&file("src/a.rs", &["1"], &["2"]), &globs, false).is_empty());
}

#[test]
fn a_repository_is_known_by_what_it_is_not_only_where_it_lives() {
    // One remote in three spellings is one remote.
    let a = repo_identity("https://github.com/Acme/Shop.git", "abc123");
    assert_eq!(a, repo_identity("git@github.com:acme/shop", "abc123"));
    assert_eq!(
        a,
        repo_identity("ssh://git@github.com/acme/shop/", "abc123")
    );
    assert_eq!(a.len(), 12);
    assert!(a.bytes().all(|b| b.is_ascii_hexdigit()));
    // A different root commit, or a different remote, is another repository.
    assert_ne!(a, repo_identity("https://github.com/acme/shop", "def456"));
    assert_ne!(a, repo_identity("https://github.com/other/shop", "abc123"));
    assert_ne!(
        a,
        repo_identity("", "abc123"),
        "no remote is not the same as one"
    );
    assert_eq!(
        normalize_remote("https://user:pw@host.example:8443/x/y.git"),
        "host.example:8443/x/y"
    );
}

#[test]
fn keys_carry_the_folder_name_and_give_it_back() {
    let id = repo_identity("", "abc");
    let key = identity_key("shop", &id);
    assert_eq!(key, format!("shop.id-{id}"));
    assert!(valid_name(&key).is_ok());
    assert_eq!(legacy_name(&key), Some("shop"));
    assert_eq!(
        legacy_name("shop"),
        None,
        "a plain folder name has no identity"
    );
    assert_eq!(legacy_name("shop.id-zzzzzzzzzzzz"), None);
    assert_eq!(legacy_name(".id-0123456789ab"), None);
    // A long folder name is cut to leave room for the identity.
    let long = identity_key(&"x".repeat(64), &id);
    assert!(long.len() <= 64, "{}", long.len());
    assert!(valid_name(&long).is_ok());
    assert_eq!(legacy_name(&long).map(str::len), Some(64 - 4 - 12));
}
