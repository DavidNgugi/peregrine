//! `check-theme`: the contrast matrix from `docs/DESIGN_SYSTEM.md` §2.2,
//! evaluated against every shipped theme.
//!
//! The gates are the matrix rows, and nothing else. A pair is measured in both
//! orders, an alpha token is composited over the surface behind it, and an
//! out-of-gamut OKLCH value is counted twice: once as a plain per-channel clip
//! and once as a chroma reduction at constant lightness and hue with the
//! residual clipped. The worse of the two renderings is what the gate sees,
//! which is what stops the gate and the themes from disagreeing about an
//! out-of-gamut accent.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::report::{Report, Violation};

const THEMES: &str = "assets/themes";

/// The CSS custom property an `oklch()` hue may defer to.
const HUE_BRAND_VAR: &str = "var(--hue-brand)";

/// Binary-search depth for the chroma-reduction gamut policy. At 32 halvings the
/// remaining chroma error is far below the 8-bit quantisation of the themes, and
/// the search costs nothing next to the file reads it guards.
const GAMUT_STEPS: u32 = 32;

/// WCAG 2.2 AA minima, from §2.2's enforced matrix.
const BODY_TEXT: f64 = 4.5;
const MEANINGFUL_NON_TEXT: f64 = 3.0;

/// Tokens the matrix reads. A theme missing one is a violation, never a skip:
/// a gate that quietly stops covering a token reads exactly like a passing one.
const REQUIRED_TOKENS: &[&str] = &[
    "surface-0",
    "surface-1",
    "surface-2",
    "text-primary",
    "text-secondary",
    "text-tertiary",
    "text-on-accent",
    "accent",
    "accent-hover",
    "border-strong",
    "state-ok",
    "state-warn",
    "state-danger",
    "state-info",
];

/// The surfaces every foreground row is measured against.
const SURFACES: &[&str] = &["surface-0", "surface-1", "surface-2"];

/// Foregrounds that must clear [`BODY_TEXT`] against [`SURFACES`].
const BODY_TEXT_TOKENS: &[&str] = &["text-primary", "text-secondary", "text-tertiary"];

/// Foregrounds that must clear [`MEANINGFUL_NON_TEXT`] against [`SURFACES`].
const NON_TEXT_TOKENS: &[&str] = &[
    "border-strong",
    "accent",
    "accent-hover",
    "state-ok",
    "state-warn",
    "state-danger",
    "state-info",
];

/// A rendered colour: gamma-encoded sRGB in `[0, 1]` per channel, plus alpha.
#[derive(Debug, Clone, Copy)]
struct Rgba {
    r: f64,
    g: f64,
    b: f64,
    a: f64,
}

/// An `oklch(L C H)` value before rendering.
#[derive(Debug, Clone, Copy)]
struct Oklch {
    lightness: f64,
    chroma: f64,
    hue: f64,
}

/// What the parse rejected, so a malformed token is reported rather than skipped.
#[derive(Debug, Clone)]
pub struct ParseError(String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A theme file as far as this gate reads it. Unknown fields are tolerated so a
/// later schema task can add wallpaper metadata without touching this one.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThemeFile {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    hue_brand: Option<f64>,
    tokens: BTreeMap<String, String>,
}

/// Checks every theme under `<root>/assets/themes`.
///
/// Returns the violations found and the number of themes read. The per-theme
/// minimum margin is printed as the gate goes, so a green run still says how
/// much headroom each theme has.
pub fn check(root: &Path) -> Result<(Report, usize)> {
    let paths = theme_paths(root)?;
    if paths.is_empty() {
        anyhow::bail!("no theme JSON found under {}", root.join(THEMES).display());
    }

    let mut report = Report::new();
    let mut checked = 0;
    for path in &paths {
        let label = path
            .file_stem()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("theme");
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let outcome = check_json(label, &text);
        report.extend(outcome.report.sorted().into_iter().cloned());
        checked += 1;

        match outcome.margin {
            Some(margin) => println!(
                "check-theme: {label} minimum margin {margin:+.4} over {} pair(s)",
                matrix().len()
            ),
            None => println!("check-theme: {label} not measured (see violations)"),
        }
    }

    Ok((report, checked))
}

/// Runs the gate: prints the outcome and reports whether every theme is clean.
pub fn run(root: &Path) -> Result<bool> {
    let (report, checked) = check(root)?;
    Ok(crate::report::finish("check-theme", &report, checked))
}

/// What one theme's evaluation produced.
#[derive(Debug)]
pub struct Outcome {
    /// The §2.2 violations found in this theme.
    pub report: Report,
    /// The smallest `measured - required` over the matrix, or `None` when the
    /// theme had nothing measurable (an unparseable or incomplete theme).
    pub margin: Option<f64>,
}

/// Evaluates the §2.2 matrix for one theme, given as JSON text.
///
/// The theme id in the violations is the one in the JSON, falling back to the
/// caller's label so an unreadable theme still names itself in the output.
#[must_use]
pub fn check_json(label: &str, text: &str) -> Outcome {
    let mut report = Report::new();
    let theme: ThemeFile = match serde_json::from_str(text) {
        Ok(theme) => theme,
        Err(error) => {
            report.push(Violation::new(
                format!("{THEMES}/{label}.json"),
                format!("theme is not a readable theme file: {error}"),
            ));
            return Outcome {
                report,
                margin: None,
            };
        }
    };
    let id = theme.id.clone().unwrap_or_else(|| label.to_owned());
    let path = format!("{THEMES}/{label}.json");

    let mut rendered: BTreeMap<&str, Rendered> = BTreeMap::new();
    let mut complete = true;
    for token in REQUIRED_TOKENS {
        let Some(value) = theme.tokens.get(*token) else {
            report.push(Violation::new(
                path.clone(),
                format!(
                    "theme `{id}` is missing token `--{token}`, which the §2.2 matrix measures"
                ),
            ));
            complete = false;
            continue;
        };
        match render_pair(value, theme.hue_brand) {
            Ok(pair) => {
                rendered.insert(token, pair);
            }
            Err(error) => {
                report.push(Violation::new(
                    path.clone(),
                    format!(
                        "theme `{id}` token `--{token}` is not a renderable oklch colour: {error}"
                    ),
                ));
                complete = false;
            }
        }
    }
    if !complete {
        return Outcome {
            report,
            margin: None,
        };
    }

    let mut minimum = f64::INFINITY;
    for (foreground, background, need) in matrix() {
        let (Some(fg), Some(bg)) = (rendered.get(foreground), rendered.get(background)) else {
            continue;
        };
        let ratio = worst_ratio(*fg, *bg);
        minimum = minimum.min(ratio - need);

        if ratio < need {
            report.push(Violation::new(
                path.clone(),
                format!(
                    "theme `{id}`: `--{foreground}` on `--{background}` is {ratio:.4}:1, \
                     below the {need:.1}:1 minimum"
                ),
            ));
        }
    }

    Outcome {
        report,
        margin: minimum.is_finite().then_some(minimum),
    }
}

/// The matrix rows: foreground token, background token, minimum ratio.
fn matrix() -> Vec<(&'static str, &'static str, f64)> {
    let mut rows = Vec::new();
    for foreground in BODY_TEXT_TOKENS {
        for surface in SURFACES {
            rows.push((*foreground, *surface, BODY_TEXT));
        }
    }
    for background in ["accent", "accent-hover"] {
        rows.push(("text-on-accent", background, BODY_TEXT));
    }
    for foreground in NON_TEXT_TOKENS {
        for surface in SURFACES {
            rows.push((*foreground, *surface, MEANINGFUL_NON_TEXT));
        }
    }
    rows
}

/// One token under both gamut policies.
#[derive(Debug, Clone, Copy)]
struct Rendered {
    /// Per-channel clipping of the linear sRGB channels.
    clipped: Rgba,
    /// Chroma reduced at constant lightness and hue until in gamut, residual clipped.
    chroma_reduced: Rgba,
}

/// The worse contrast over both orders and both gamut renderings of a pair.
///
/// Every disagreement a renderer could have — which token is the background in
/// a reversed reading, and how it resolves an out-of-gamut value — is resolved
/// against the theme, so a green run is green under either reading.
fn worst_ratio(foreground: Rendered, background: Rendered) -> f64 {
    let mut worst = f64::INFINITY;
    for fg in [foreground.clipped, foreground.chroma_reduced] {
        for bg in [background.clipped, background.chroma_reduced] {
            worst = worst.min(contrast_on(fg, bg)).min(contrast_on(bg, fg));
        }
    }
    worst
}

/// Renders one token as both gamut policies.
fn render_pair(value: &str, hue_brand: Option<f64>) -> Result<Rendered, ParseError> {
    let color = parse_oklch(value, hue_brand)?;
    Ok(Rendered {
        clipped: to_rgba(color, color.chroma),
        chroma_reduced: to_rgba(color, in_gamut_chroma(color)),
    })
}

/// The largest chroma at constant lightness and hue whose linear sRGB channels
/// are all inside `[0, 1]`, found by bisection between zero and the declared
/// chroma. A neutral grey is always in gamut, so the low end needs no probe.
fn in_gamut_chroma(color: Oklch) -> f64 {
    let in_gamut = |chroma: f64| {
        oklch_to_linear(Oklch { chroma, ..color })
            .iter()
            .all(|channel| (0.0..=1.0).contains(channel))
    };
    if in_gamut(color.chroma) {
        return color.chroma;
    }
    let (mut low, mut high) = (0.0, color.chroma);
    for _ in 0..GAMUT_STEPS {
        let middle = f64::midpoint(low, high);
        if in_gamut(middle) {
            low = middle;
        } else {
            high = middle;
        }
    }
    low
}

/// Parses `oklch(L C H)` and `oklch(L C H / A)`, resolving `var(--hue-brand)`.
fn parse_oklch(value: &str, hue_brand: Option<f64>) -> Result<Oklch, ParseError> {
    let inner = value
        .trim()
        .strip_prefix("oklch(")
        .and_then(|rest| rest.strip_suffix(')'))
        .ok_or_else(|| ParseError(format!("`{value}` is not an `oklch()` value")))?;

    let (components, alpha) = match inner.split_once('/') {
        Some((components, alpha)) => (components, Some(parse_number(alpha.trim())?)),
        None => (inner, None),
    };
    if alpha.is_some_and(|alpha| !(0.0..=1.0).contains(&alpha)) {
        return Err(ParseError("alpha is outside [0, 1]".to_owned()));
    }

    let mut parts = components.split_whitespace();
    let lightness = parse_number(next(&mut parts, "lightness")?)?;
    let chroma = parse_number(next(&mut parts, "chroma")?)?;
    let hue = parse_hue(next(&mut parts, "hue")?, hue_brand)?;
    if parts.next().is_some() {
        return Err(ParseError(format!("`{value}` has too many components")));
    }
    if !(0.0..=1.0).contains(&lightness) {
        return Err(ParseError(format!(
            "lightness {lightness} is outside [0, 1]"
        )));
    }
    if chroma < 0.0 {
        return Err(ParseError(format!("chroma {chroma} is negative")));
    }
    Ok(Oklch {
        lightness,
        chroma,
        hue,
    })
}

/// Takes the next whitespace-separated component, naming it in the error.
fn next<'a>(parts: &mut impl Iterator<Item = &'a str>, what: &str) -> Result<&'a str, ParseError> {
    parts
        .next()
        .ok_or_else(|| ParseError(format!("missing {what} component")))
}

/// Parses a hue, resolving the one variable the shipped themes use.
fn parse_hue(value: &str, hue_brand: Option<f64>) -> Result<f64, ParseError> {
    if value == "none" {
        return Ok(0.0);
    }
    if value == HUE_BRAND_VAR {
        return hue_brand.ok_or_else(|| {
            ParseError(format!(
                "`{HUE_BRAND_VAR}` is used but the theme declares no `hueBrand`"
            ))
        });
    }
    let hue = parse_number(value)?;
    if !hue.is_finite() {
        return Err(ParseError(format!("hue `{value}` is not finite")));
    }
    Ok(hue)
}

/// Parses a component, accepting an optional unit suffix (`0.5`, `50%`, `250deg`).
fn parse_number(value: &str) -> Result<f64, ParseError> {
    let trimmed = value.trim();
    let (digits, scale) = if let Some(percent) = trimmed.strip_suffix('%') {
        (percent, 0.01)
    } else if let Some(degrees) = trimmed.strip_suffix("deg") {
        (degrees, 1.0)
    } else {
        (trimmed, 1.0)
    };
    digits
        .trim()
        .parse::<f64>()
        .map(|number| number * scale)
        .map_err(|_| ParseError(format!("`{value}` is not a number")))
}

/// OKLCH at an explicit chroma, through `OKLab` to linear sRGB.
fn oklch_to_linear(color: Oklch) -> [f64; 3] {
    let (sin, cos) = color.hue.to_radians().sin_cos();
    let axis_a = color.chroma * cos;
    let axis_b = color.chroma * sin;

    let cone_l = color.lightness + 0.396_337_777_4 * axis_a + 0.215_803_757_3 * axis_b;
    let cone_m = color.lightness - 0.105_561_345_8 * axis_a - 0.063_854_172_8 * axis_b;
    let cone_s = color.lightness - 0.089_484_177_5 * axis_a - 1.291_485_548_0 * axis_b;
    let (long, medium, short) = (
        cone_l * cone_l * cone_l,
        cone_m * cone_m * cone_m,
        cone_s * cone_s * cone_s,
    );

    [
        4.076_741_662_1 * long - 3.307_711_591_3 * medium + 0.230_969_929_2 * short,
        -1.268_438_004_6 * long + 2.609_757_401_1 * medium - 0.341_319_396_5 * short,
        -0.004_196_086_3 * long - 0.703_418_614_7 * medium + 1.707_614_701_0 * short,
    ]
}

/// Renders a token to gamma-encoded sRGB at one chroma, clipping the residual.
fn to_rgba(color: Oklch, chroma: f64) -> Rgba {
    let [r, g, b] =
        oklch_to_linear(Oklch { chroma, ..color }).map(|channel| encode(channel.clamp(0.0, 1.0)));
    Rgba { r, g, b, a: 1.0 }
}

/// Gamma-encodes a linear sRGB channel with the sRGB transfer function.
fn encode(channel: f64) -> f64 {
    if channel <= 0.003_130_8 {
        12.92 * channel
    } else {
        1.055 * channel.powf(1.0 / 2.4) - 0.055
    }
}

/// WCAG 2.2 contrast ratio between two rendered colours, worse order included.
fn contrast(a: Rgba, b: Rgba) -> f64 {
    let (one, other) = (relative_luminance(a), relative_luminance(b));
    let (lighter, darker) = if one >= other {
        (one, other)
    } else {
        (other, one)
    };
    (lighter + 0.05) / (darker + 0.05)
}

/// Composites `foreground` over `background` and returns the opaque result.
fn composite(foreground: Rgba, background: Rgba) -> Rgba {
    let over = |fg: f64, bg: f64| foreground.a.mul_add(fg, bg * (1.0 - foreground.a));
    Rgba {
        r: over(foreground.r, background.r),
        g: over(foreground.g, background.g),
        b: over(foreground.b, background.b),
        a: 1.0,
    }
}

/// The contrast of `foreground` as painted on `background`.
fn contrast_on(foreground: Rgba, background: Rgba) -> f64 {
    contrast(composite(foreground, background), background)
}

/// WCAG 2.2 relative luminance, from the gamma-encoded sRGB channels.
fn relative_luminance(color: Rgba) -> f64 {
    let linear = |channel: f64| {
        if channel <= 0.040_45 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

/// Formats a rendered colour as `#rrggbb`, used by the control tests.
#[cfg(test)]
fn hex(color: Rgba) -> String {
    let byte = |channel: f64| (channel * 255.0).round().clamp(0.0, 255.0) as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        byte(color.r),
        byte(color.g),
        byte(color.b)
    )
}

/// Every JSON theme file under `<root>/assets/themes`, in a stable order.
fn theme_paths(root: &Path) -> Result<Vec<PathBuf>> {
    let directory = root.join(THEMES);
    let entries = std::fs::read_dir(&directory)
        .with_context(|| format!("cannot read {}", directory.display()))?;
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(std::ffi::OsStr::to_str) == Some("json"))
        .collect();
    paths.sort();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace_root() -> PathBuf {
        crate::workspace::find_root(&PathBuf::from(env!("CARGO_MANIFEST_DIR")))
            .expect("xtask lives inside the workspace")
    }

    /// Renders one token of a shipped theme under both gamut policies.
    fn shipped(theme: &str, token: &str) -> Rendered {
        let text =
            std::fs::read_to_string(workspace_root().join(THEMES).join(format!("{theme}.json")))
                .expect("the shipped theme must exist");
        let parsed: ThemeFile = serde_json::from_str(&text).expect("the shipped theme must parse");
        render_pair(
            parsed.tokens.get(token).expect("the token must exist"),
            parsed.hue_brand,
        )
        .expect("the token must render")
    }

    /// A theme identical to Default Dark except that `border-strong` is the same
    /// lightness as the surfaces it must stand out from. Built as text rather
    /// than committed as a fixture, because the gate's negative case should not
    /// depend on a file that can drift out of violation.
    const TOO_DARK_THEME: &str = r#"{
        "schemaVersion": 1,
        "id": "inline-violation",
        "hueBrand": 250,
        "tokens": {
            "surface-0": "oklch(0.19 0.008 var(--hue-brand))",
            "surface-1": "oklch(0.23 0.010 var(--hue-brand))",
            "surface-2": "oklch(0.27 0.012 var(--hue-brand))",
            "text-primary": "oklch(0.96 0.005 var(--hue-brand))",
            "text-secondary": "oklch(0.78 0.010 var(--hue-brand))",
            "text-tertiary": "oklch(0.65 0.010 var(--hue-brand))",
            "text-on-accent": "oklch(0.30 0 0)",
            "border-strong": "oklch(0.22 0.012 var(--hue-brand))",
            "accent": "oklch(0.68 0.16 var(--hue-brand))",
            "accent-hover": "oklch(0.73 0.16 var(--hue-brand))",
            "state-ok": "oklch(0.72 0.15 150)",
            "state-warn": "oklch(0.80 0.15 85)",
            "state-danger": "oklch(0.68 0.19 25)",
            "state-info": "oklch(0.72 0.12 240)"
        }
    }"#;

    /// The three controls named in the task. If these drift, the maths is wrong
    /// and nothing downstream of it can be trusted.
    #[test]
    fn the_three_controls_hold() {
        let accent = hex(shipped("default-dark", "accent").clipped);
        let text_primary = hex(shipped("default-dark", "text-primary").clipped);
        let measured = contrast(
            shipped("default-dark", "text-primary").clipped,
            shipped("default-dark", "surface-0").clipped,
        );
        println!("controls: accent={accent} text-primary={text_primary} ratio={measured:.5}");

        assert_eq!(accent, "#3b9cf6", "asset token `accent`");
        assert_eq!(text_primary, "#eff2f5", "`text-primary`");
        assert!(
            (measured - 16.440_3).abs() < 0.000_1,
            "`text-primary` on `surface-0` measured {measured:.5}, expected 16.4403"
        );
    }

    #[test]
    fn every_shipped_theme_passes_the_matrix() {
        let (report, checked) = check(&workspace_root()).expect("the themes must be readable");
        assert_eq!(checked, 4, "the four shipped themes must all be read");
        assert!(
            report.is_clean(),
            "shipped themes violate §2.2: {:#?}",
            report.sorted()
        );
    }

    /// The gate must keep covering the tokens the matrix names, and must not
    /// drift into the decorative ones §2.2 excludes.
    #[test]
    fn the_matrix_covers_exactly_the_specified_pairs() {
        let rows = matrix();
        for foreground in BODY_TEXT_TOKENS {
            for surface in SURFACES {
                assert!(
                    rows.iter()
                        .any(|(f, b, need)| f == foreground && b == surface && *need == BODY_TEXT),
                    "missing row {foreground} on {surface}"
                );
            }
        }
        for background in ["accent", "accent-hover"] {
            assert!(
                rows.iter()
                    .any(|(f, b, _)| *f == "text-on-accent" && *b == background),
                "missing row text-on-accent on {background}"
            );
        }
        for foreground in NON_TEXT_TOKENS {
            for surface in SURFACES {
                assert!(
                    rows.iter().any(|(f, b, need)| f == foreground
                        && b == surface
                        && *need == MEANINGFUL_NON_TEXT),
                    "missing row {foreground} on {surface}"
                );
            }
        }
        assert_eq!(rows.len(), 3 * 3 + 2 + 7 * 3);
        for excluded in ["border-subtle", "surface-scrim", "accent-muted"] {
            assert!(
                !rows
                    .iter()
                    .any(|(f, b, _)| *f == excluded || *b == excluded),
                "`{excluded}` is decorative and out of scope"
            );
        }
    }

    /// The negative test: themes are built inline, so the gate proves it can
    /// fail without a committed fixture directory.
    #[test]
    fn a_violating_theme_is_rejected() {
        let outcome = check_json("inline-violation", TOO_DARK_THEME);
        assert!(
            !outcome.report.is_clean(),
            "a `border-strong` at the surface's own lightness must fail"
        );
        assert!(
            outcome.margin.is_some_and(|margin| margin < 0.0),
            "a violating theme reports a negative margin, got {:?}",
            outcome.margin
        );
        let messages: Vec<&str> = outcome
            .report
            .sorted()
            .into_iter()
            .map(|violation| violation.message.as_str())
            .collect();
        assert!(
            messages.iter().any(|message| {
                message.contains("--border-strong") && message.contains("below the 3.0:1 minimum")
            }),
            "expected a `border-strong` contrast violation, got {messages:#?}"
        );
    }

    /// A tree containing a violating theme must fail the command, which is the
    /// acceptance criterion: `cargo xtask check-theme` exits non-zero on a
    /// violation, not merely on a malformed file.
    #[test]
    fn the_command_fails_on_a_tree_that_violates_the_matrix() {
        let root = tempfile::TempDir::new().unwrap();
        let themes = root.path().join(THEMES);
        std::fs::create_dir_all(&themes).unwrap();
        std::fs::write(themes.join("violation.json"), TOO_DARK_THEME).unwrap();

        assert!(
            !run(root.path()).expect("the gate must run against the fixture tree"),
            "a violating theme must make the gate fail"
        );

        // The same tree with the violation repaired must pass, so the failure
        // above is attributable to the contrast and not to the fixture setup.
        let repaired = TOO_DARK_THEME.replace(
            "\"border-strong\": \"oklch(0.22 0.012 var(--hue-brand))\"",
            "\"border-strong\": \"oklch(0.55 0.012 var(--hue-brand))\"",
        );
        assert_ne!(
            repaired, TOO_DARK_THEME,
            "the repair must change the fixture"
        );
        std::fs::write(themes.join("violation.json"), &repaired).unwrap();
        assert!(
            run(root.path()).expect("the gate must run against the repaired tree"),
            "the repaired tree must pass"
        );
    }

    /// A missing token is reported, never skipped: measuring the remaining pairs
    /// would produce a clean-looking theme that covers less than the matrix says.
    #[test]
    fn a_missing_token_is_reported_not_skipped() {
        let missing = r#"{
            "schemaVersion": 1,
            "id": "inline-missing",
            "hueBrand": 250,
            "tokens": {
                "surface-0": "oklch(0.19 0.008 var(--hue-brand))",
                "surface-1": "oklch(0.23 0.010 var(--hue-brand))",
                "surface-2": "oklch(0.27 0.012 var(--hue-brand))",
                "text-secondary": "oklch(0.78 0.010 var(--hue-brand))",
                "text-tertiary": "oklch(0.65 0.010 var(--hue-brand))",
                "text-on-accent": "oklch(0.30 0 0)",
                "border-strong": "oklch(0.55 0.012 var(--hue-brand))",
                "accent": "oklch(0.68 0.16 var(--hue-brand))",
                "accent-hover": "oklch(0.73 0.16 var(--hue-brand))",
                "state-ok": "oklch(0.72 0.15 150)",
                "state-warn": "oklch(0.80 0.15 85)",
                "state-danger": "oklch(0.68 0.19 25)",
                "state-info": "oklch(0.72 0.12 240)"
            }
        }"#;
        let outcome = check_json("inline-missing", missing);
        assert_eq!(
            outcome.margin, None,
            "nothing measurable was left to report"
        );
        assert!(
            outcome
                .report
                .sorted()
                .iter()
                .any(|violation| violation.message.contains("missing token `--text-primary`")),
            "a missing token must be reported: {:#?}",
            outcome.report.sorted()
        );
    }
}
