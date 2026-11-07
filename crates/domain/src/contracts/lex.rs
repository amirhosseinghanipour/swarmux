//! A small tokenizer for C-like code and Python: identifiers, punctuation
//! and string literals. Comments are dropped and a string's content is
//! never read as code, so `// .from('x')` and `"call .from('x')"` find
//! nothing. It does not parse: regular-expression literals and JSX text
//! with a stray quote can confuse it (an unterminated quote ends at the
//! end of its line, so the damage stays on one line).

/// What a token is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tok {
    Ident(String),
    Punct(char),
    /// A string literal's text. `interp`: it had `${…}` / `$name`, each
    /// turned into `{}`.
    Str {
        text: String,
        interp: bool,
    },
}

/// A token and the line it starts on (from 1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub tok: Tok,
    pub line: u32,
}

/// The lexical rules of a language family.
#[derive(Clone, Copy, Debug)]
pub struct Syntax {
    /// `// …` and `/* … */`.
    pub slash_comments: bool,
    /// `# …` to the end of the line.
    pub hash_comments: bool,
    /// `` `…` `` is a string (with `${…}`).
    pub backtick: bool,
    /// `'''…'''` and `"""…"""`.
    pub triple: bool,
    /// `$name` and `${…}` inside quotes are interpolations (Dart).
    pub dollar: bool,
}

impl Syntax {
    pub const JS: Syntax = Syntax {
        slash_comments: true,
        hash_comments: false,
        backtick: true,
        triple: false,
        dollar: false,
    };
    pub const DART: Syntax = Syntax {
        slash_comments: true,
        hash_comments: false,
        backtick: false,
        triple: true,
        dollar: true,
    };
    pub const PYTHON: Syntax = Syntax {
        slash_comments: false,
        hash_comments: true,
        backtick: false,
        triple: true,
        dollar: false,
    };
}

/// The tokens of `src`.
pub fn lex(src: &str, syn: &Syntax) -> Vec<Token> {
    let c: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1u32;
    while i < c.len() {
        let ch = c[i];
        if ch == '\n' {
            line += 1;
            i += 1;
        } else if ch.is_whitespace() {
            i += 1;
        } else if syn.slash_comments && ch == '/' && c.get(i + 1) == Some(&'/') {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
        } else if syn.slash_comments && ch == '/' && c.get(i + 1) == Some(&'*') {
            i += 2;
            while i < c.len() && !(c[i] == '*' && c.get(i + 1) == Some(&'/')) {
                if c[i] == '\n' {
                    line += 1;
                }
                i += 1;
            }
            i = (i + 2).min(c.len());
        } else if syn.hash_comments && ch == '#' {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
        } else if ch == '\'' || ch == '"' || (syn.backtick && ch == '`') {
            let start = line;
            let triple =
                syn.triple && ch != '`' && c.get(i + 1) == Some(&ch) && c.get(i + 2) == Some(&ch);
            let (text, interp, next, lines) = string(&c, i, ch, triple, syn);
            line += lines;
            i = next;
            out.push(Token {
                tok: Tok::Str { text, interp },
                line: start,
            });
        } else if ch.is_alphabetic() || ch == '_' || ch == '$' {
            let from = i;
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_' || c[i] == '$') {
                i += 1;
            }
            out.push(Token {
                tok: Tok::Ident(c[from..i].iter().collect()),
                line,
            });
        } else if ch.is_ascii_digit() {
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '.' || c[i] == '_') {
                i += 1;
            }
        } else {
            out.push(Token {
                tok: Tok::Punct(ch),
                line,
            });
            i += 1;
        }
    }
    out
}

/// Reads the string starting at `c[at]`; returns its text, whether it
/// interpolates, where reading goes on and how many newlines it spanned.
fn string(
    c: &[char],
    at: usize,
    q: char,
    triple: bool,
    syn: &Syntax,
) -> (String, bool, usize, u32) {
    let mut i = at + if triple { 3 } else { 1 };
    let mut text = String::new();
    let mut interp = false;
    let mut lines = 0;
    let interpolates = q == '`' || syn.dollar;
    while i < c.len() {
        let ch = c[i];
        if triple {
            if ch == q && c.get(i + 1) == Some(&q) && c.get(i + 2) == Some(&q) {
                return (text, interp, i + 3, lines);
            }
        } else if ch == q {
            return (text, interp, i + 1, lines);
        } else if ch == '\n' && q != '`' {
            // Unterminated: end it here.
            return (text, interp, i, lines);
        }
        if ch == '\\' && i + 1 < c.len() {
            if c[i + 1] == '\n' {
                lines += 1;
            }
            text.push(c[i + 1]);
            i += 2;
            continue;
        }
        if ch == '\n' {
            lines += 1;
        }
        if interpolates && ch == '$' && c.get(i + 1) == Some(&'{') {
            let mut depth = 0;
            while i < c.len() {
                match c[i] {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            i += 1;
                            break;
                        }
                    }
                    '\n' => lines += 1,
                    _ => {}
                }
                i += 1;
            }
            text.push_str("{}");
            interp = true;
            continue;
        }
        if syn.dollar && ch == '$' && c.get(i + 1).is_some_and(|n| n.is_alphabetic() || *n == '_') {
            i += 1;
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') {
                i += 1;
            }
            text.push_str("{}");
            interp = true;
            continue;
        }
        text.push(ch);
        i += 1;
    }
    (text, interp, i, lines)
}
