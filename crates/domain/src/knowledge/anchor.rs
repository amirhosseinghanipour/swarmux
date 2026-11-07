//! Anchors: the code an item talks about, and the content hashes that
//! tell when it changed. Deterministic and free: no model is asked.

use crate::ledger::{hex, sha256};

/// The hash of a file (or symbol) that isn't there.
pub const MISSING: &str = "missing";

/// A file, optionally one symbol in it, and its content hash.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Anchor {
    /// Relative to the home's root, `/`-separated.
    pub path: String,
    pub symbol: Option<String>,
    /// [`content_hash`] when it was last confirmed; empty before the first.
    pub hash: String,
}

impl Anchor {
    /// `path` or `path#symbol`.
    pub fn parse(spec: &str) -> Anchor {
        let spec = spec.trim().replace('\\', "/");
        let (path, symbol) = match spec.split_once('#') {
            Some((p, s)) if !s.trim().is_empty() => (p.to_string(), Some(s.trim().to_string())),
            Some((p, _)) => (p.to_string(), None),
            None => (spec.clone(), None),
        };
        Anchor {
            path: path.trim_start_matches("./").to_string(),
            symbol,
            hash: String::new(),
        }
    }

    /// `path` or `path#symbol`.
    pub fn spec(&self) -> String {
        match &self.symbol {
            Some(s) => format!("{}#{s}", self.path),
            None => self.path.clone(),
        }
    }

    /// The same anchor with `content` hashed.
    pub fn hashed(&self, content: Option<&str>) -> Anchor {
        Anchor {
            hash: content_hash(content, self.symbol.as_deref()),
            ..self.clone()
        }
    }
}

/// The hash an anchor keeps of `content` (`None`: the file is missing).
///
/// Line endings are normalised, so a checkout on Windows hashes like one
/// elsewhere. With a symbol, only the lines that mention it count: the
/// item goes stale when the symbol's declaration or uses change, not when
/// something else in the file does. A symbol no line mentions is missing.
pub fn content_hash(content: Option<&str>, symbol: Option<&str>) -> String {
    let Some(content) = content else {
        return MISSING.to_string();
    };
    let text = content.replace("\r\n", "\n");
    let text = match symbol {
        Some(s) => {
            let lines: Vec<&str> = text
                .lines()
                .filter(|l| l.contains(s))
                .map(str::trim)
                .collect();
            if lines.is_empty() {
                return MISSING.to_string();
            }
            lines.join("\n")
        }
        None => text,
    };
    hex(&sha256(text.as_bytes()))
}

/// The hash over all anchors an item is valid for; empty without anchors.
pub fn valid_for(anchors: &[Anchor]) -> String {
    if anchors.is_empty() {
        return String::new();
    }
    let mut sorted: Vec<&Anchor> = anchors.iter().collect();
    sorted.sort();
    let mut bytes = Vec::new();
    for a in sorted {
        bytes.extend_from_slice(a.path.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(a.symbol.as_deref().unwrap_or("").as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(a.hash.as_bytes());
        bytes.push(b'\n');
    }
    hex(&sha256(&bytes))
}

/// The anchors of `anchors` whose content no longer hashes the same, with
/// `read` giving a path's content now.
pub fn changed<'a>(
    anchors: &'a [Anchor],
    read: &dyn Fn(&str) -> Option<String>,
) -> Vec<&'a Anchor> {
    anchors
        .iter()
        .filter(|a| content_hash(read(&a.path).as_deref(), a.symbol.as_deref()) != a.hash)
        .collect()
}
