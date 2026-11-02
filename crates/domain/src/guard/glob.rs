//! Path normalisation and glob matching for path rules.
//!
//! Paths are compared with `/` separators. A path that looks like a Windows
//! path (`C:/…`) is compared without regard to ASCII case, as the file system
//! does.

/// `\` → `/`, `.` and `..` resolved lexically, repeated `/` collapsed.
/// Returns `None` when `..` climbs above the start of a relative path.
pub fn normalize(path: &str) -> Option<String> {
    let path = path.replace('\\', "/");
    let (root, rest) = split_root(&path);
    let mut parts: Vec<&str> = Vec::new();
    for part in rest.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            p => parts.push(p),
        }
    }
    Some(format!("{root}{}", parts.join("/")))
}

/// Splits `/x`, `C:/x` and `//server/x` style roots from the rest.
fn split_root(path: &str) -> (&str, &str) {
    let b = path.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        let end = if b.get(2) == Some(&b'/') { 3 } else { 2 };
        return path.split_at(end);
    }
    let slashes = b.iter().take_while(|&&c| c == b'/').count();
    path.split_at(slashes.min(1))
}

/// True for `/x` and `C:/x` (after normalisation).
pub fn is_absolute(path: &str) -> bool {
    let b = path.as_bytes();
    b.first() == Some(&b'/') || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
}

/// True when the path is a Windows path and must be compared case-insensitively.
pub fn is_windows(path: &str) -> bool {
    let b = path.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// Absolute form of `path` (resolved against `cwd` when relative).
pub fn absolute(path: &str, cwd: &str) -> Option<String> {
    let path = path.replace('\\', "/");
    if is_absolute(&path) {
        normalize(&path)
    } else {
        normalize(&format!("{}/{path}", cwd.replace('\\', "/")))
    }
}

/// `path` relative to `cwd`, or `None` when it lies outside `cwd`.
pub fn relative(path: &str, cwd: &str) -> Option<String> {
    let abs = absolute(path, cwd)?;
    let base = normalize(cwd)?;
    let fold = is_windows(&base);
    let base = base.trim_end_matches('/');
    if eq(&abs, base, fold) {
        return Some(String::new());
    }
    let prefix = format!("{base}/");
    (abs.len() > prefix.len() && eq(&abs[..prefix.len()], &prefix, fold))
        .then(|| abs[prefix.len()..].to_string())
}

fn eq(a: &str, b: &str, fold: bool) -> bool {
    if fold {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// Matches `path` against `pattern`, both with `/` separators.
///
/// `**` as a whole segment matches any number of segments (including none),
/// `*` any run of characters within a segment and `?` one character.
pub fn glob_match(pattern: &str, path: &str, fold: bool) -> bool {
    let pat: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match_segments(&pat, &segs, fold)
}

fn match_segments(pat: &[&str], segs: &[&str], fold: bool) -> bool {
    match pat.split_first() {
        None => segs.is_empty(),
        Some((&"**", rest)) => (0..=segs.len()).any(|i| match_segments(rest, &segs[i..], fold)),
        Some((p, rest)) => match segs.split_first() {
            Some((s, tail)) => match_segment(p, s, fold) && match_segments(rest, tail, fold),
            None => false,
        },
    }
}

/// `*` and `?` wildcards within one segment.
pub fn match_segment(pattern: &str, text: &str, fold: bool) -> bool {
    let norm = |s: &str| -> Vec<char> {
        if fold {
            s.chars().map(|c| c.to_ascii_lowercase()).collect()
        } else {
            s.chars().collect()
        }
    };
    let (p, t) = (norm(pattern), norm(text));
    wildcard(&p, &t)
}

/// Iterative wildcard match with backtracking to the last `*`.
fn wildcard(p: &[char], t: &[char]) -> bool {
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_resolves_dots_and_backslashes() {
        assert_eq!(normalize(r"C:\a\.\b\..\c").as_deref(), Some("C:/a/c"));
        assert_eq!(normalize("/a//b/").as_deref(), Some("/a/b"));
        assert_eq!(normalize("a/../b").as_deref(), Some("b"));
        assert_eq!(normalize("../x"), None);
        assert_eq!(normalize("/..").as_deref(), None);
    }

    #[test]
    fn relative_to_cwd() {
        assert_eq!(relative("src/a.rs", "/w").as_deref(), Some("src/a.rs"));
        assert_eq!(relative("/w/src/a.rs", "/w").as_deref(), Some("src/a.rs"));
        assert_eq!(relative("/w", "/w").as_deref(), Some(""));
        assert_eq!(relative("/wx/a", "/w"), None);
        assert_eq!(relative("/w/../etc/passwd", "/w"), None);
        assert_eq!(relative("src/../../x", "/w"), None);
    }

    #[test]
    fn windows_paths_ignore_case_and_separators() {
        assert_eq!(
            relative(r"c:\Users\Me\Proj\src\A.rs", r"C:\Users\me\proj").as_deref(),
            Some("src/A.rs")
        );
        assert_eq!(relative(r"D:\proj\a", r"C:\proj"), None);
    }

    #[test]
    fn glob_segments() {
        assert!(glob_match("src/**", "src/a/b.rs", false));
        assert!(glob_match("src/**", "src", false));
        assert!(!glob_match("src/**", "srcx/a", false));
        assert!(glob_match("**/*.md", "README.md", false));
        assert!(glob_match("**/*.md", "a/b/c.md", false));
        assert!(!glob_match("**/*.md", "a/b/c.mdx", false));
        assert!(glob_match("src/*.rs", "src/main.rs", false));
        assert!(!glob_match("src/*.rs", "src/x/main.rs", false));
        assert!(glob_match("a/?.txt", "a/b.txt", false));
        assert!(glob_match("**", "", false));
    }

    #[test]
    fn glob_case_folding() {
        assert!(glob_match("SRC/*.RS", "src/a.rs", true));
        assert!(!glob_match("SRC/*.RS", "src/a.rs", false));
    }

    #[test]
    fn wildcard_backtracks() {
        assert!(match_segment("*a*b", "xxaxxb", false));
        assert!(!match_segment("*a*b", "xxaxxc", false));
        assert!(match_segment("*", "", false));
    }
}
