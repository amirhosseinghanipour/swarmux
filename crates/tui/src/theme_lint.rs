//! Lints over every built-in theme: contrast against the background, and how
//! the semantic colours hold up under simulated colour-vision deficiency.

use ratatui::style::Color;

use crate::theme::{THEMES, Theme, rgb_of};

fn channels(c: Color) -> [f64; 3] {
    let v = rgb_of(c);
    [
        f64::from((v >> 16) as u8),
        f64::from((v >> 8) as u8),
        f64::from(v as u8),
    ]
}

fn linear(v: f64) -> f64 {
    let s = v / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

/// WCAG relative luminance.
fn luminance(c: Color) -> f64 {
    let [r, g, b] = channels(c);
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

/// WCAG contrast ratio, 1..21.
pub(crate) fn contrast(a: Color, b: Color) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Deuteranopia (Machado et al. 2009, severity 1.0) on linear RGB.
pub(crate) fn deuteranopia(c: Color) -> [f64; 3] {
    const M: [[f64; 3]; 3] = [
        [0.367_322, 0.860_646, -0.227_968],
        [0.280_085, 0.672_501, 0.047_413],
        [-0.011_820, 0.042_940, 0.968_881],
    ];
    let [r, g, b] = channels(c);
    let lin = [linear(r), linear(g), linear(b)];
    let out = |row: [f64; 3]| (row[0] * lin[0] + row[1] * lin[1] + row[2] * lin[2]).clamp(0.0, 1.0);
    [out(M[0]), out(M[1]), out(M[2])]
}

fn to_hex(lin: [f64; 3]) -> String {
    let enc = |v: f64| {
        let s = if v <= 0.003_130_8 {
            v * 12.92
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        };
        (s * 255.0).round() as u8
    };
    format!("#{:02x}{:02x}{:02x}", enc(lin[0]), enc(lin[1]), enc(lin[2]))
}

/// CIE76 colour difference between two linear-RGB colours.
fn delta_e(a: [f64; 3], b: [f64; 3]) -> f64 {
    let lab = |[r, g, bl]: [f64; 3]| {
        let x = (0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * bl) / 0.950_47;
        let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175 * bl;
        let z = (0.019_333_9 * r + 0.119_192 * g + 0.950_304_1 * bl) / 1.088_83;
        let f = |t: f64| {
            if t > 0.008_856 {
                t.cbrt()
            } else {
                7.787 * t + 16.0 / 116.0
            }
        };
        (
            116.0 * f(y) - 16.0,
            500.0 * (f(x) - f(y)),
            200.0 * (f(y) - f(z)),
        )
    };
    let (l1, a1, b1) = lab(a);
    let (l2, a2, b2) = lab(b);
    ((l1 - l2).powi(2) + (a1 - a2).powi(2) + (b1 - b2).powi(2)).sqrt()
}

fn semantic(t: &Theme) -> [(&'static str, Color); 4] {
    [
        ("error", t.error),
        ("warn", t.warn),
        ("ok", t.ok),
        ("info", t.haiku),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Body text must clear 4.5:1; anything that carries information 3:1.
    #[test]
    fn contrast_lint_every_theme() {
        let mut failures = Vec::new();
        for t in THEMES {
            for (surface, bg) in [("bg", t.bg), ("panel", t.panel_bg)] {
                for (role, c) in [("text", t.text), ("title", t.title)] {
                    let r = contrast(c, bg);
                    if r < 4.5 {
                        failures.push(format!("{} {role} on {surface}: {r:.2} < 4.5", t.name));
                    }
                }
                let informational = [
                    ("muted", t.muted),
                    ("label", t.label),
                    ("lead", t.lead),
                    ("builder", t.builder),
                    ("haiku", t.haiku),
                    ("guard", t.guard),
                    ("reviewer", t.reviewer),
                    ("ok", t.ok),
                    ("warn", t.warn),
                    ("error", t.error),
                    ("border_focus", t.border_focus),
                    ("gauge0", t.gauge[0]),
                    ("gauge1", t.gauge[1]),
                    ("gauge2", t.gauge[2]),
                    ("chart0", t.chart[0]),
                    ("chart1", t.chart[1]),
                    ("chart2", t.chart[2]),
                ];
                for (role, c) in informational {
                    let r = contrast(c, bg);
                    if r < 3.0 {
                        failures.push(format!("{} {role} on {surface}: {r:.2} < 3", t.name));
                    }
                }
            }
            // Text on an accent chip (the active tab, a P1 badge).
            for (role, accent) in [
                ("lead", t.lead),
                ("warn", t.warn),
                ("focus", t.border_focus),
            ] {
                let r = contrast(t.on_accent, accent);
                if r < 3.0 {
                    failures.push(format!("{} on_accent over {role}: {r:.2} < 3", t.name));
                }
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    /// The lowest body and informational ratios per theme, for the record
    /// (`cargo test contrast_summary -- --nocapture`).
    #[test]
    fn contrast_summary() {
        for t in THEMES {
            let mut body = f64::MAX;
            let mut info = f64::MAX;
            for bg in [t.bg, t.panel_bg] {
                for c in [t.text, t.title] {
                    body = body.min(contrast(c, bg));
                }
                for c in [
                    t.muted, t.label, t.lead, t.builder, t.haiku, t.guard, t.reviewer,
                ]
                .into_iter()
                .chain([t.ok, t.warn, t.error, t.border_focus])
                .chain(t.gauge)
                .chain(t.chart)
                {
                    info = info.min(contrast(c, bg));
                }
            }
            println!("{:<17} body>={body:5.2} info>={info:5.2}", t.name);
            assert!(body >= 4.5 && info >= 3.0, "{}", t.name);
        }
    }

    /// The pairs that must never collapse for a deuteranope: error (P1),
    /// warn (P2) and ok.
    fn status_gaps(t: &Theme) -> [(&'static str, f64); 3] {
        let sim = |c| deuteranopia(c);
        [
            ("error-warn", delta_e(sim(t.error), sim(t.warn))),
            ("error-ok", delta_e(sim(t.error), sim(t.ok))),
            ("warn-ok", delta_e(sim(t.warn), sim(t.ok))),
        ]
    }

    /// Golden values: the default theme's status colours as a deuteranope
    /// sees them. A change here is a change to what the default looks like.
    #[test]
    fn deuteranopia_snapshot_of_the_default() {
        let t = crate::theme::REFERENCE;
        let seen: Vec<String> = semantic(&t)
            .iter()
            .map(|(n, c)| format!("{n}={}", to_hex(deuteranopia(*c))))
            .collect();
        assert_eq!(
            seen,
            [
                "error=#a19456",
                "warn=#bdab46",
                "ok=#bcb286",
                "info=#7399e9"
            ]
        );
        for (pair, gap) in status_gaps(&t) {
            assert!(gap >= 15.0, "reference {pair}: {gap:.1}");
        }
    }

    #[test]
    fn okabe_ito_stays_apart_for_a_deuteranope() {
        let t = crate::theme::OKABE_ITO;
        for (pair, gap) in status_gaps(&t) {
            assert!(gap >= 30.0, "okabe-ito {pair}: {gap:.1}");
        }
    }

    /// Every dark theme keeps P1 apart from P2 and from ok; the pairs it
    /// doesn't are reported, since glyphs and position carry state too.
    #[test]
    fn deuteranopia_error_stays_apart_in_every_theme() {
        for t in THEMES {
            for (pair, gap) in status_gaps(&t) {
                println!("{:<17} {pair:<10} {gap:5.1}", t.name);
            }
            let [(_, error_warn), (_, error_ok), _] = status_gaps(&t);
            assert!(error_warn >= 15.0, "{} error-warn", t.name);
            assert!(error_ok >= 9.0, "{} error-ok", t.name);
        }
    }

    #[test]
    fn deuteranopia_report() {
        for t in THEMES {
            let s = semantic(&t);
            let mut line = format!("{:<17}", t.name);
            for i in 0..4 {
                for j in i + 1..4 {
                    let d = delta_e(deuteranopia(s[i].1), deuteranopia(s[j].1));
                    line += &format!(" {}-{}:{d:5.1}", s[i].0, s[j].0);
                }
            }
            println!("{line}");
            println!(
                "   sim: {}",
                s.iter()
                    .map(|(n, c)| format!("{n}={}", to_hex(deuteranopia(*c))))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
    }
}
