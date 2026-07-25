//! Viewport presets for `codemux browser viewport`.
//!
//! Agents and humans want to test "does my page look right on mobile?"
//! without hand-maintaining the latest iPhone / Pixel / Galaxy model
//! number. Hardware vendors have effectively settled on a small set of
//! CSS-pixel widths over the past ~6 years (320, 390, 430, 768, 1024),
//! so this module exposes those widths as **size-bucket** presets that
//! won't go stale every release cycle.
//!
//! The presets compile down to a `set viewport <w> <h> [scale]`
//! invocation against the underlying `agent-browser` CLI — meaning the
//! browser actually reflows the page at the simulated width and CSS
//! media queries fire correctly, instead of the older iframe trick that
//! lacked DPR, touch, and clean screenshots.
//!
//! ## Stability of the names
//!
//! Phone CSS widths have been stable for years:
//!
//! - iPhone SE class (3-/4-gen) sits at 320px
//! - iPhone 13/14/15/Pixel 7 sit around 390px
//! - Pro Max / Pixel Pro sit around 430px
//! - iPad portrait is 768px, iPad Pro is 1024px
//!
//! Future hardware refreshes mostly bump DPR, not CSS width, so the
//! size-bucket names should remain accurate without per-release edits.

use serde::Serialize;

/// Numeric viewport target after a preset has been resolved.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ViewportSpec {
    pub width: u32,
    pub height: u32,
    /// Device pixel ratio (Retina factor). 1.0 for desktop, 2.0+ for
    /// modern phones/tablets — affects `window.devicePixelRatio`,
    /// `image-set()` asset selection, and `@2x` media features.
    pub dpr: f64,
}

impl ViewportSpec {
    pub const fn new(width: u32, height: u32, dpr: f64) -> Self {
        Self { width, height, dpr }
    }
}

/// One row in the preset table.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Preset {
    pub name: &'static str,
    pub spec: ViewportSpec,
    pub description: &'static str,
}

/// The canonical preset table. Order matters — `list_presets()` returns
/// them in this order so the CLI and MCP listing UX flows mobile → tablet
/// → desktop top-down.
pub const PRESETS: &[Preset] = &[
    Preset {
        name: "mobile-small",
        spec: ViewportSpec::new(320, 568, 2.0),
        description: "Small phones (iPhone SE class, older Androids)",
    },
    Preset {
        name: "mobile",
        spec: ViewportSpec::new(390, 844, 3.0),
        description: "Standard modern phones (iPhone 13/14/15, Pixel 7)",
    },
    Preset {
        name: "mobile-large",
        spec: ViewportSpec::new(430, 932, 3.0),
        description: "Large phones (iPhone Pro Max, Pixel Pro)",
    },
    Preset {
        name: "tablet",
        spec: ViewportSpec::new(768, 1024, 2.0),
        description: "Tablets in portrait (iPad mini/Air)",
    },
    Preset {
        name: "tablet-large",
        spec: ViewportSpec::new(1024, 1366, 2.0),
        description: "Large tablets in portrait (iPad Pro 12.9\")",
    },
    Preset {
        name: "desktop",
        spec: ViewportSpec::new(1280, 800, 1.0),
        description: "Standard desktop / laptop (matches Tailwind `xl:` breakpoint)",
    },
    Preset {
        name: "desktop-large",
        spec: ViewportSpec::new(1920, 1080, 1.0),
        description: "Full-HD desktop monitor",
    },
];

/// The built-in "reset" baseline. This matches the in-app default that
/// `BrowserPane.tsx` initialises with, so issuing `viewport reset` leaves
/// the page rendering at the same baseline as a fresh browser pane.
/// When the user configured `browser.default_viewport` (synced settings),
/// reset lands on that instead — see [`configured_default_spec`].
pub const RESET_SPEC: ViewportSpec = ViewportSpec::new(1280, 800, 1.0);

/// Resolve a raw `browser.default_viewport` setting string into a spec,
/// or `None` when the value is unset or unusable.
///
/// This is the single source of truth for interpreting the setting —
/// pure, so it stays unit-testable without touching the on-disk settings
/// cache. Empty/whitespace values and a literal `"reset"` (which would
/// be self-referential as a *default*) count as unset; any parse
/// failure also yields `None`: the setting syncs across devices, so a
/// bad value must degrade silently rather than break startup.
pub fn resolve_default_setting(raw: Option<&str>) -> Option<ViewportSpec> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("reset"))
        .and_then(|s| parse_spec(s, None).ok())
}

/// Convenience over [`resolve_default_setting`]: same resolution, but
/// falls back to the built-in [`RESET_SPEC`] baseline when the setting
/// is unset or invalid — documenting the fallback contract in one place.
pub fn resolve_default(raw: Option<&str>) -> ViewportSpec {
    resolve_default_setting(raw).unwrap_or(RESET_SPEC)
}

/// Cache-reading wrapper around [`resolve_default_setting`]: reads the
/// user's `browser.default_viewport` from the synced-settings cache and
/// resolves it. `Some` means the user explicitly chose a default (e.g.
/// `"2560x1440"` to match their monitor) — fresh agent-browser daemons
/// get it applied at launch.
pub fn configured_default_viewport() -> Option<ViewportSpec> {
    let cached = crate::settings_sync::load_cache()?;
    resolve_default_setting(cached.browser.default_viewport.as_deref())
}

/// The spec `viewport reset` should return to: the user-configured
/// default when present, the built-in [`RESET_SPEC`] baseline otherwise.
pub fn configured_default_spec() -> ViewportSpec {
    configured_default_viewport().unwrap_or(RESET_SPEC)
}

/// [`parse_spec`], except `"reset"` resolves to the user-configured
/// default viewport ([`configured_default_spec`]) instead of the
/// hard-coded baseline. Use this at the CLI / MCP surfaces where
/// "reset" means "back to *my* default"; keep `parse_spec` for pure,
/// setting-independent parsing (and tests).
pub fn parse_spec_configured(
    input: &str,
    dpr_override: Option<f64>,
) -> Result<ViewportSpec, ParseError> {
    if input.trim().eq_ignore_ascii_case("reset") {
        let mut spec = configured_default_spec();
        if let Some(dpr) = dpr_override {
            validate_dpr(dpr)?;
            spec.dpr = dpr;
        }
        return Ok(spec);
    }
    parse_spec(input, dpr_override)
}

/// Errors `parse_spec` can surface back to the CLI.
#[derive(Debug, PartialEq)]
pub enum ParseError {
    /// User typed a preset name that doesn't exist, e.g. `iphone-25`.
    UnknownPreset(String),
    /// User typed `WxH` but it didn't parse (non-numeric, missing `x`,
    /// zero dimensions, etc.).
    InvalidDimensions(String),
    /// User typed `--dpr <n>` but `n` was zero, negative, or absurdly
    /// large.
    InvalidDpr(f64),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownPreset(name) => {
                let names: Vec<_> = PRESETS.iter().map(|p| p.name).collect();
                write!(
                    f,
                    "Unknown viewport preset '{name}'. Available: {} (or 'reset', or a custom WxH like '390x844').",
                    names.join(", ")
                )
            }
            Self::InvalidDimensions(s) => write!(
                f,
                "Invalid viewport dimensions '{s}'. Expected 'WxH' like '390x844' with positive integers."
            ),
            Self::InvalidDpr(v) => write!(
                f,
                "Invalid --dpr {v}. Must be a positive number between 0.5 and 5.0 (e.g. 1, 2, 3)."
            ),
        }
    }
}

impl std::error::Error for ParseError {}

/// Parse the user-facing `spec` argument into a concrete `ViewportSpec`.
///
/// Accepts:
/// - Any preset name from `PRESETS` (e.g. `"mobile"`, `"desktop-large"`)
/// - `"reset"` → `RESET_SPEC`
/// - A custom `"WxH"` string like `"390x844"` (uses DPR override or 1.0)
///
/// An explicit `dpr_override` always wins over the preset's default
/// DPR, so `viewport mobile --dpr 2` gives mobile dimensions at retina
/// 2× instead of the preset's stock 3×.
pub fn parse_spec(input: &str, dpr_override: Option<f64>) -> Result<ViewportSpec, ParseError> {
    let trimmed = input.trim();

    // "reset" — back to a known-good desktop baseline.
    if trimmed.eq_ignore_ascii_case("reset") {
        let mut spec = RESET_SPEC;
        if let Some(dpr) = dpr_override {
            validate_dpr(dpr)?;
            spec.dpr = dpr;
        }
        return Ok(spec);
    }

    // Preset name — exact match, case-insensitive.
    if let Some(preset) = PRESETS
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(trimmed))
    {
        let mut spec = preset.spec;
        if let Some(dpr) = dpr_override {
            validate_dpr(dpr)?;
            spec.dpr = dpr;
        }
        return Ok(spec);
    }

    // Custom WxH like "390x844". Lowercased so "390X844" works. The
    // `looks_like_wxh` gate prevents preset names that happen to contain
    // an `x` (e.g. "supermax", "phoenix") from being mis-classified as
    // dimension strings — those should surface UnknownPreset, not
    // InvalidDimensions.
    let lowered = trimmed.to_ascii_lowercase();
    if looks_like_wxh(&lowered) {
        let (w_str, h_str) = lowered
            .split_once('x')
            .expect("looks_like_wxh guarantees the separator");
        let w: u32 = w_str
            .parse()
            .map_err(|_| ParseError::InvalidDimensions(trimmed.to_string()))?;
        let h: u32 = h_str
            .parse()
            .map_err(|_| ParseError::InvalidDimensions(trimmed.to_string()))?;
        if w == 0 || h == 0 {
            return Err(ParseError::InvalidDimensions(trimmed.to_string()));
        }
        // Guardrail against typos like `3900x844` (off-by-one zero) or
        // `1x844` (probably meant `1024`). Anything wider than 4K is
        // almost certainly a mistake.
        if w > 7680 || h > 7680 {
            return Err(ParseError::InvalidDimensions(trimmed.to_string()));
        }
        let dpr = match dpr_override {
            Some(d) => {
                validate_dpr(d)?;
                d
            }
            None => 1.0,
        };
        return Ok(ViewportSpec::new(w, h, dpr));
    }

    Err(ParseError::UnknownPreset(trimmed.to_string()))
}

/// Heuristic: does `s` look like a `WxH` dimension string rather than a
/// preset name that happens to contain an `x`?
///
/// We require:
/// - exactly one `x` separator
/// - both halves are non-empty
/// - both halves consist only of ASCII digits
///
/// This deliberately rejects strings like `iphone-25-pro-supermax` (the
/// `x` is inside a word, not separating two numbers) so the parser
/// surfaces `UnknownPreset` with the full preset list rather than a
/// confusing `InvalidDimensions` error.
fn looks_like_wxh(s: &str) -> bool {
    let Some((w, h)) = s.split_once('x') else {
        return false;
    };
    !w.is_empty()
        && !h.is_empty()
        && w.bytes().all(|b| b.is_ascii_digit())
        && h.bytes().all(|b| b.is_ascii_digit())
}

fn validate_dpr(dpr: f64) -> Result<(), ParseError> {
    if !dpr.is_finite() || dpr < 0.5 || dpr > 5.0 {
        Err(ParseError::InvalidDpr(dpr))
    } else {
        Ok(())
    }
}

/// Public preset listing used by `codemux browser viewport-presets` and
/// the MCP `browser_viewport` tool hint. Cheap clone — the preset table
/// is small and static.
pub fn list_presets() -> Vec<Preset> {
    PRESETS.to_vec()
}

/// Build the JSON action payload sent over the control socket to the
/// `agent_browser` viewport handler.
///
/// Both the `codemux browser viewport` CLI subcommand and the MCP
/// `browser_viewport` tool call this — keeping the payload shape in ONE
/// place so the two surfaces can't drift apart. If a future change adds
/// a new field (e.g. `mobile`, `touch`, `user_agent`), it goes here and
/// both surfaces pick it up for free.
///
/// The `kind` field is the discriminator that `agent_browser.rs`'s
/// action dispatch matches against.
pub fn socket_action(spec: ViewportSpec) -> serde_json::Value {
    serde_json::json!({
        "kind": "viewport",
        "width": spec.width,
        "height": spec.height,
        "scale": spec.dpr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_names_are_unique() {
        let mut names: Vec<&str> = PRESETS.iter().map(|p| p.name).collect();
        names.sort();
        let total = names.len();
        names.dedup();
        assert_eq!(names.len(), total, "duplicate preset name in PRESETS");
    }

    #[test]
    fn resolve_default_unset_falls_back_to_reset_spec() {
        assert_eq!(resolve_default(None), RESET_SPEC);
        assert_eq!(resolve_default(Some("")), RESET_SPEC);
        assert_eq!(resolve_default(Some("   ")), RESET_SPEC);
    }

    #[test]
    fn resolve_default_parses_custom_dimensions() {
        let spec = resolve_default(Some("2560x1440"));
        assert_eq!((spec.width, spec.height, spec.dpr), (2560, 1440, 1.0));
    }

    #[test]
    fn resolve_default_accepts_preset_names() {
        let spec = resolve_default(Some("desktop-large"));
        assert_eq!((spec.width, spec.height), (1920, 1080));
    }

    /// Bad synced values (typos, other-device garbage) must degrade to
    /// the baseline, never error — and a literal "reset" would be
    /// self-referential as a default, so it's treated as unset too.
    #[test]
    fn resolve_default_invalid_or_reset_falls_back() {
        assert_eq!(resolve_default(Some("not-a-preset")), RESET_SPEC);
        assert_eq!(resolve_default(Some("0x0")), RESET_SPEC);
        assert_eq!(resolve_default(Some("reset")), RESET_SPEC);
    }

    /// Exercises the shared pure core directly — `resolve_default`,
    /// `configured_default_viewport`, and `configured_default_spec` all
    /// delegate here, so this covers the logic that actually ships.
    #[test]
    fn resolve_default_setting_some_or_none() {
        let spec = resolve_default_setting(Some("2560x1440")).expect("custom WxH should resolve");
        assert_eq!((spec.width, spec.height, spec.dpr), (2560, 1440, 1.0));

        let spec = resolve_default_setting(Some("desktop-large")).expect("preset should resolve");
        assert_eq!((spec.width, spec.height), (1920, 1080));

        // Unset, blank, self-referential "reset", and unparseable values
        // all count as "no configured default".
        assert_eq!(resolve_default_setting(None), None);
        assert_eq!(resolve_default_setting(Some("")), None);
        assert_eq!(resolve_default_setting(Some("   ")), None);
        assert_eq!(resolve_default_setting(Some("reset")), None);
        assert_eq!(resolve_default_setting(Some("not-a-preset")), None);
        assert_eq!(resolve_default_setting(Some("0x0")), None);
    }

    #[test]
    fn parse_known_preset_mobile() {
        let spec = parse_spec("mobile", None).unwrap();
        assert_eq!(spec.width, 390);
        assert_eq!(spec.height, 844);
        assert_eq!(spec.dpr, 3.0);
    }

    #[test]
    fn parse_known_preset_is_case_insensitive() {
        let spec = parse_spec("DESKTOP", None).unwrap();
        assert_eq!(spec.width, 1280);
        let spec = parse_spec("Tablet-Large", None).unwrap();
        assert_eq!(spec.width, 1024);
    }

    #[test]
    fn parse_reset_returns_default() {
        let spec = parse_spec("reset", None).unwrap();
        assert_eq!(spec, RESET_SPEC);
        // Even the literal "RESET" works.
        let spec = parse_spec("RESET", None).unwrap();
        assert_eq!(spec, RESET_SPEC);
    }

    #[test]
    fn parse_custom_wxh() {
        let spec = parse_spec("390x844", None).unwrap();
        assert_eq!(spec.width, 390);
        assert_eq!(spec.height, 844);
        // Default DPR for raw WxH is 1.0 — caller must opt in to retina
        // via --dpr so we don't surprise them.
        assert_eq!(spec.dpr, 1.0);
    }

    #[test]
    fn parse_custom_wxh_uppercase_x() {
        // We accept the literal `X` for forgiving CLI ergonomics.
        let spec = parse_spec("800X600", None).unwrap();
        assert_eq!(spec.width, 800);
        assert_eq!(spec.height, 600);
    }

    #[test]
    fn parse_dpr_override_wins_over_preset_default() {
        // `mobile` defaults to DPR 3.0; explicit `--dpr 2` should win.
        let spec = parse_spec("mobile", Some(2.0)).unwrap();
        assert_eq!(spec.width, 390);
        assert_eq!(spec.dpr, 2.0);
    }

    #[test]
    fn parse_unknown_preset_errors() {
        // Includes an `x` mid-word ("supermax") on purpose — the WxH
        // heuristic must reject this so we surface UnknownPreset (with
        // the preset list in the error message) instead of letting it
        // fall through to InvalidDimensions parsing.
        let err = parse_spec("iphone-25-pro-supermax", None).unwrap_err();
        assert!(matches!(err, ParseError::UnknownPreset(_)),
            "expected UnknownPreset, got: {:?}", err);
        // Error message should list available presets so the user can recover.
        let msg = err.to_string();
        assert!(msg.contains("mobile"));
        assert!(msg.contains("desktop"));
        assert!(msg.contains("WxH"));
    }

    #[test]
    fn parse_word_containing_x_is_unknown_preset_not_dimensions() {
        // Specifically guard the heuristic regression: words like
        // "phoenix" or "max" must NOT be mistaken for WxH dimension
        // strings.
        for word in &["phoenix", "examplexname", "ipxconfig"] {
            let err = parse_spec(word, None).unwrap_err();
            assert!(
                matches!(err, ParseError::UnknownPreset(_)),
                "{word}: expected UnknownPreset, got {:?}",
                err
            );
        }
    }

    #[test]
    fn looks_like_wxh_accepts_dimension_strings() {
        assert!(looks_like_wxh("390x844"));
        assert!(looks_like_wxh("1x1"));
        assert!(looks_like_wxh("9999x9999"));
    }

    #[test]
    fn looks_like_wxh_rejects_words() {
        assert!(!looks_like_wxh("phoenix"));
        assert!(!looks_like_wxh("supermax"));
        assert!(!looks_like_wxh("mobile"));
        // Mixed digit + letter halves don't qualify either.
        assert!(!looks_like_wxh("390xfoo"));
        assert!(!looks_like_wxh("fooxbar"));
        // Empty halves don't qualify.
        assert!(!looks_like_wxh("x"));
        assert!(!looks_like_wxh("390x"));
        assert!(!looks_like_wxh("x844"));
    }

    #[test]
    fn parse_invalid_dimensions_zero() {
        assert!(matches!(
            parse_spec("0x600", None).unwrap_err(),
            ParseError::InvalidDimensions(_)
        ));
        assert!(matches!(
            parse_spec("800x0", None).unwrap_err(),
            ParseError::InvalidDimensions(_)
        ));
    }

    #[test]
    fn parse_non_numeric_is_unknown_preset() {
        // With the strict WxH heuristic, strings like "aaaxbbb" and
        // "800x" fail the all-digits check and fall through to
        // UnknownPreset — better UX, because the error message lists the
        // available presets instead of vaguely complaining about
        // dimensions.
        assert!(matches!(
            parse_spec("aaaxbbb", None).unwrap_err(),
            ParseError::UnknownPreset(_)
        ));
        assert!(matches!(
            parse_spec("800x", None).unwrap_err(),
            ParseError::UnknownPreset(_)
        ));
    }

    #[test]
    fn parse_overflow_dimensions_is_invalid() {
        // 99 999 999 999 exceeds u32::MAX (~4.29B), so even though both
        // halves are all digits and look like WxH, parse fails — that's
        // the path that should surface InvalidDimensions.
        let err = parse_spec("99999999999x600", None).unwrap_err();
        assert!(
            matches!(err, ParseError::InvalidDimensions(_)),
            "expected InvalidDimensions, got: {:?}",
            err
        );
    }

    #[test]
    fn parse_invalid_dimensions_too_large() {
        // Guardrail against accidental 8K+ dimensions (typo like one
        // extra zero). Anything over 7680 (8K) is rejected.
        assert!(matches!(
            parse_spec("99999x600", None).unwrap_err(),
            ParseError::InvalidDimensions(_)
        ));
    }

    #[test]
    fn parse_invalid_dpr_rejected() {
        assert!(matches!(
            parse_spec("mobile", Some(0.0)).unwrap_err(),
            ParseError::InvalidDpr(_)
        ));
        assert!(matches!(
            parse_spec("mobile", Some(-1.0)).unwrap_err(),
            ParseError::InvalidDpr(_)
        ));
        assert!(matches!(
            parse_spec("mobile", Some(100.0)).unwrap_err(),
            ParseError::InvalidDpr(_)
        ));
        assert!(matches!(
            parse_spec("mobile", Some(f64::NAN)).unwrap_err(),
            ParseError::InvalidDpr(_)
        ));
    }

    #[test]
    fn parse_trims_whitespace() {
        let spec = parse_spec("  mobile  ", None).unwrap();
        assert_eq!(spec.width, 390);
    }

    #[test]
    fn every_preset_round_trips_through_parse_spec() {
        for preset in PRESETS {
            let spec = parse_spec(preset.name, None).unwrap();
            assert_eq!(
                spec, preset.spec,
                "preset {} round-trip mismatch",
                preset.name
            );
        }
    }

    #[test]
    fn list_presets_returns_full_table() {
        let listed = list_presets();
        assert_eq!(listed.len(), PRESETS.len());
        assert!(listed.iter().any(|p| p.name == "mobile"));
        assert!(listed.iter().any(|p| p.name == "desktop-large"));
    }

    #[test]
    fn mobile_presets_have_retina_dpr() {
        // Catches accidental DPR=1.0 regressions on the mobile presets,
        // since the whole point of these is realistic mobile rendering.
        for preset in PRESETS {
            if preset.name.starts_with("mobile") || preset.name.starts_with("tablet") {
                assert!(
                    preset.spec.dpr >= 2.0,
                    "mobile/tablet preset {} should have DPR >= 2.0 to simulate retina; got {}",
                    preset.name,
                    preset.spec.dpr
                );
            }
        }
    }

    #[test]
    fn desktop_presets_have_unity_dpr() {
        // Desktop presets at retina would render text small in the
        // screenshot and would not match how users actually see the page
        // — keep them at 1.0.
        for preset in PRESETS {
            if preset.name.starts_with("desktop") {
                assert_eq!(
                    preset.spec.dpr, 1.0,
                    "desktop preset {} should have DPR=1.0; got {}",
                    preset.name, preset.spec.dpr
                );
            }
        }
    }

    #[test]
    fn mobile_widths_match_css_pixel_reality() {
        // Catch a regression where someone "rounds up" mobile widths to
        // device pixels — these are CSS pixels, hardware refreshes do
        // not change them.
        assert_eq!(parse_spec("mobile-small", None).unwrap().width, 320);
        assert_eq!(parse_spec("mobile", None).unwrap().width, 390);
        assert_eq!(parse_spec("mobile-large", None).unwrap().width, 430);
    }

    // -----------------------------------------------------------------
    // Drift-guard tests — keep the CLI and MCP socket payloads in lock-
    // step. Both surfaces MUST build their action via `socket_action`;
    // if a future PR bypasses this helper on one side, the tests below
    // will catch the divergence before the build hits a release.
    // -----------------------------------------------------------------

    #[test]
    fn socket_action_emits_canonical_shape() {
        // The agent_browser viewport handler matches on these exact
        // field names — anything else is silently dropped. If you
        // rename a field here, you must update agent_browser.rs and
        // BROWSER-AGENT-COMMANDS.md in the same commit.
        let spec = ViewportSpec::new(390, 844, 3.0);
        let action = socket_action(spec);
        assert_eq!(action["kind"], "viewport");
        assert_eq!(action["width"], 390);
        assert_eq!(action["height"], 844);
        assert_eq!(action["scale"], 3.0);
        // Lock the exact key set so a stray field ("device", "touch"...)
        // can't sneak in unnoticed.
        let obj = action.as_object().expect("socket_action must return a JSON object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, vec!["height", "kind", "scale", "width"]);
    }

    #[test]
    fn socket_action_round_trips_every_preset() {
        // Every preset must produce an action whose width/height/scale
        // match the preset's spec exactly. Catches a regression where
        // someone "optimises" socket_action and drops a field, or where
        // a new preset is added but its scale is silently dropped on
        // the wire.
        for preset in PRESETS {
            let action = socket_action(preset.spec);
            assert_eq!(action["kind"], "viewport", "kind for {}", preset.name);
            assert_eq!(action["width"].as_u64(), Some(preset.spec.width as u64),
                "width for {}", preset.name);
            assert_eq!(action["height"].as_u64(), Some(preset.spec.height as u64),
                "height for {}", preset.name);
            assert_eq!(action["scale"].as_f64(), Some(preset.spec.dpr),
                "scale for {}", preset.name);
        }
    }

    #[test]
    fn cli_and_mcp_both_delegate_to_socket_action() {
        // Source-grep drift guard. The CLI viewport handler in cli.rs
        // and the MCP browser_viewport dispatch in mcp_server.rs MUST
        // both build their socket action via this module's
        // `socket_action()` helper. A future PR that inlines the JSON
        // shape on one surface would break payload symmetry — this
        // test catches that without needing a running daemon.
        //
        // Reads the sibling source files at test time. CARGO_MANIFEST_DIR
        // points at src-tauri/, so we walk down into src/ from there.
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let cli_src = std::fs::read_to_string(
            std::path::Path::new(manifest_dir).join("src/cli.rs"),
        )
        .expect("cli.rs should be readable for drift check");
        let mcp_src = std::fs::read_to_string(
            std::path::Path::new(manifest_dir).join("src/mcp_server.rs"),
        )
        .expect("mcp_server.rs should be readable for drift check");

        assert!(
            cli_src.contains("browser_viewport::socket_action"),
            "cli.rs must delegate viewport payload construction to \
             browser_viewport::socket_action — otherwise the CLI surface \
             can drift from MCP. Found neither call site."
        );
        assert!(
            mcp_src.contains("browser_viewport::socket_action"),
            "mcp_server.rs must delegate viewport payload construction \
             to browser_viewport::socket_action — otherwise MCP can \
             drift from the CLI. Found neither call site."
        );
    }

    #[test]
    fn socket_action_is_what_cli_and_mcp_must_emit() {
        // This test exists to make the drift contract explicit: anyone
        // grepping for "socket_action" lands here and sees a comment
        // stating that BOTH the CLI Viewport handler in cli.rs AND the
        // MCP browser_viewport dispatch in mcp_server.rs are expected
        // to delegate to this function. Removing either delegation is
        // a regression even if the test still passes here — search the
        // repo for `browser_viewport::socket_action` and confirm at
        // least two call sites in the lib crate (cli.rs + mcp_server.rs).
        //
        // The assertion itself just exercises the happy path so the
        // test still catches signature changes.
        let spec = parse_spec("mobile", None).unwrap();
        let action = socket_action(spec);
        let expected = serde_json::json!({
            "kind": "viewport",
            "width": 390,
            "height": 844,
            "scale": 3.0,
        });
        assert_eq!(action, expected);
    }
}
