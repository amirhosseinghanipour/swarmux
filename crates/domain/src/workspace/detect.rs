//! What a repository is built with and which commands it offers, read
//! from a listing of its files. No I/O: the caller lists the files and
//! reads the few manifests named in [`MANIFESTS`].

use super::{RepoCommand, Stack};
use crate::json::Json;

/// Files whose text detection reads, relative to the repository root.
pub const MANIFESTS: [&str; 9] = [
    "package.json",
    "pubspec.yaml",
    "pyproject.toml",
    "Makefile",
    "makefile",
    "GNUmakefile",
    "justfile",
    "Justfile",
    ".justfile",
];

/// A repository's files, as detection sees them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepoListing {
    /// Relative paths with `/`, folders included with a trailing `/`.
    /// A few levels deep is enough.
    pub files: Vec<String>,
    /// The texts of the [`MANIFESTS`] that exist, by relative path.
    pub texts: Vec<(String, String)>,
}

impl RepoListing {
    fn has(&self, path: &str) -> bool {
        self.files.iter().any(|f| f == path)
    }

    fn has_dir(&self, dir: &str) -> bool {
        let dir = format!("{}/", dir.trim_end_matches('/'));
        self.files.iter().any(|f| f.starts_with(&dir))
    }

    /// A file named `name` at most `depth` folders down.
    fn has_named(&self, depth: usize, pred: impl Fn(&str) -> bool) -> bool {
        self.files.iter().any(|f| {
            !f.ends_with('/')
                && f.matches('/').count() <= depth
                && pred(f.rsplit('/').next().unwrap_or(f))
        })
    }

    fn text(&self, path: &str) -> Option<&str> {
        self.texts
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, t)| t.as_str())
    }
}

/// What detection found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Detected {
    pub stacks: Vec<Stack>,
    pub commands: Vec<RepoCommand>,
}

impl Detected {
    fn stack(&mut self, s: Stack) {
        if !self.stacks.contains(&s) {
            self.stacks.push(s);
        }
    }

    fn command(&mut self, name: &str, line: String, source: &str) {
        if self.commands.iter().any(|c| c.line == line) {
            return;
        }
        self.commands.push(RepoCommand {
            name: name.to_string(),
            line,
            source: source.to_string(),
        });
    }
}

/// The stacks and commands of the repository `listing` describes.
pub fn detect(listing: &RepoListing) -> Detected {
    let mut d = Detected::default();
    if listing.has("pubspec.yaml") {
        let flutter = listing
            .text("pubspec.yaml")
            .is_some_and(|t| t.contains("sdk: flutter") || t.lines().any(|l| l == "flutter:"));
        if flutter {
            d.stack(Stack::Flutter);
            d.command("analyze", "flutter analyze".into(), "flutter");
            d.command("test", "flutter test".into(), "flutter");
        } else {
            d.stack(Stack::Dart);
            d.command("analyze", "dart analyze".into(), "dart");
            d.command("test", "dart test".into(), "dart");
        }
    }
    if listing.has("package.json") {
        d.stack(Stack::Node);
        node_scripts(listing, &mut d);
    }
    if listing.has("Cargo.toml") {
        d.stack(Stack::Rust);
        d.command("build", "cargo build".into(), "rust");
        d.command("test", "cargo test".into(), "rust");
    }
    let python = [
        "pyproject.toml",
        "requirements.txt",
        "setup.py",
        "setup.cfg",
    ];
    if python.iter().any(|f| listing.has(f)) {
        d.stack(Stack::Python);
        let pytest = listing.has("pytest.ini")
            || listing.has_dir("tests")
            || listing
                .text("pyproject.toml")
                .is_some_and(|t| t.contains("[tool.pytest"));
        if pytest {
            d.command("test", "python -m pytest".into(), "python");
        }
    }
    if listing.has("go.mod") {
        d.stack(Stack::Go);
        d.command("build", "go build ./...".into(), "go");
        d.command("test", "go test ./...".into(), "go");
    }
    if listing.has("supabase/config.toml") {
        d.stack(Stack::Supabase);
        d.command("lint", "supabase db lint".into(), "supabase");
        if listing.has_dir("supabase/tests") {
            d.command("test", "supabase test db".into(), "supabase");
        }
    }
    if listing.has("Gemfile") {
        d.stack(Stack::Ruby);
        if listing.has_dir("spec") {
            d.command("test", "bundle exec rspec".into(), "ruby");
        } else if listing.has("Rakefile") {
            d.command("test", "bundle exec rake test".into(), "ruby");
        }
    }
    let dotnet = |n: &str| n.ends_with(".csproj") || n.ends_with(".fsproj") || n.ends_with(".sln");
    if listing.has_named(2, dotnet) {
        d.stack(Stack::DotNet);
        d.command("build", "dotnet build".into(), "dotnet");
        d.command("test", "dotnet test".into(), "dotnet");
    }
    if listing.has("ProjectSettings/ProjectVersion.txt") {
        d.stack(Stack::Unity);
    }
    if listing.has("project.godot") {
        d.stack(Stack::Godot);
    }
    for file in ["GNUmakefile", "makefile", "Makefile"] {
        if let Some(text) = listing.text(file) {
            for target in make_targets(text) {
                d.command(&target, format!("make {target}"), "Makefile");
            }
            break;
        }
    }
    for file in ["justfile", "Justfile", ".justfile"] {
        if let Some(text) = listing.text(file) {
            for recipe in just_recipes(text) {
                d.command(&recipe, format!("just {recipe}"), "justfile");
            }
            break;
        }
    }
    d.stacks.sort();
    d
}

/// `package.json`'s scripts, run with the package manager its lockfile
/// names.
fn node_scripts(listing: &RepoListing, d: &mut Detected) {
    let Some(json) = listing
        .text("package.json")
        .and_then(|t| Json::parse(t).ok())
    else {
        return;
    };
    let Some(Json::Obj(scripts)) = json.get("scripts") else {
        return;
    };
    let manager = if listing.has("pnpm-lock.yaml") {
        "pnpm"
    } else if listing.has("yarn.lock") {
        "yarn"
    } else if listing.has("bun.lockb") || listing.has("bun.lock") {
        "bun"
    } else {
        "npm"
    };
    for (name, _) in scripts {
        let line = match (manager, name.as_str()) {
            ("npm", "test") => "npm test".to_string(),
            ("yarn", _) => format!("yarn {name}"),
            (m, _) => format!("{m} run {name}"),
        };
        d.command(name, line, "package.json");
    }
}

/// Whether `name` could be a make target or just recipe a person runs.
fn plain_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// The explicit targets of a Makefile, in order: no special (`.PHONY`),
/// pattern (`%.o`) or variable (`X := 1`) lines.
pub(super) fn make_targets(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        if line.starts_with(['\t', ' ', '#']) {
            continue;
        }
        let Some((head, rest)) = line.split_once(':') else {
            continue;
        };
        if rest.starts_with('=') || head.contains(['=', '$', '%']) {
            continue;
        }
        for target in head.split_whitespace() {
            if plain_name(target) && !out.iter().any(|t| t == target) {
                out.push(target.to_string());
            }
        }
    }
    out
}

/// The recipes of a justfile, in order: no settings, aliases, variables
/// or private (`_x`) recipes.
pub(super) fn just_recipes(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        if line.starts_with([' ', '\t', '#', '[', '@']) {
            continue;
        }
        let first = line.split_whitespace().next().unwrap_or_default();
        if matches!(first, "set" | "alias" | "export" | "import" | "mod") {
            continue;
        }
        let Some((head, rest)) = line.split_once(':') else {
            continue;
        };
        if rest.starts_with('=') {
            continue;
        }
        let Some(name) = head.split_whitespace().next() else {
            continue;
        };
        if plain_name(name) && !out.iter().any(|t| t == name) {
            out.push(name.to_string());
        }
    }
    out
}
