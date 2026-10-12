//! `/cd` disclosure: name the new directory's config that is NOT applied (#869),
//! and say that its `[permissions]` deny entries now ARE (Day 218, `cd_deny.rs`),
//! and its `[directories]` deny entries too (Day 226, file tools, below).
//!
//! After `/cd`, the launch directory's `[permissions]`, `[directories]`, hooks and
//! MCP servers stay in force and the new directory's `.yoyo.toml` is never read.
//! Reloading is the real fix and is still open (write-once `OnceLock`, and a naive
//! reload could *widen* the fence the user lives inside). This module is only the
//! disclosure half: it READS the new directory's file and says, specifically,
//! which safety-relevant sections it sets that this session is ignoring.
//!
//! **Disclosure is not a control.** Nothing here applies, merges or executes any
//! value from the file, and nothing here touches trust, permissions or globals.
//! Day 218: one value IS applied, by the `/cd` arm in `dispatch.rs` rather than
//! here — the reading's `deny` entries, appended to `cd_deny`'s session list,
//! which both bash executors read at call time. Deny only narrows, so it needs
//! no trust check. This module reports it from the same reading `dispatch.rs`
//! appended, so the note cannot disagree with what was applied.
//! Day 226: the reading's `[directories] deny` entries are applied the same way,
//! resolved against the `/cd` target at append time and kept in this module's
//! append-only list, which both path guards in `tool_wrappers.rs` read at call
//! time (`path_guard_refusal`). `[directories] allow` is NOT applied: it would
//! have to be intersected with the startup allow list, which is a design call.
//!
//! The file is read through yoyo's own config parsers — the same doors startup
//! uses — so "does this file set X" has exactly one statement per section and
//! cannot disagree with what a restart in that directory would actually load.

use std::path::Path;

/// What reading the new directory's `.yoyo.toml` produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CdConfigReading {
    /// No `.yoyo.toml` there — the common case; prints nothing.
    Absent,
    /// The file exists but could not be read (permissions, not UTF-8). This is
    /// deliberately distinct from "sets nothing": we cannot tell what it sets.
    Unreadable,
    /// The file was read: the NOT-applied sections it sets (possibly none), its
    /// `[permissions] deny` entries, which `/cd` does apply (bash only), and its
    /// `[directories] deny` entries as written, which `/cd` applies to the file
    /// tools after resolving them against the new directory (Day 226).
    Sets(Vec<&'static str>, Vec<String>, Vec<String>),
}

/// Section labels, in the fixed order they are reported.
/// Only the allow halves: both deny halves are applied on `/cd` (Days 218, 226).
const PERMISSIONS: &str = "[permissions] allow";
const DIRECTORIES: &str = "[directories] allow";
const HOOKS: &str = "hooks";
const MCP: &str = "MCP servers";

/// Every section this module can name. All four decide what the session may do
/// (what is denied, where it may write, what runs around a tool call, which
/// external tools exist), so a note naming any of them is a WARNING, not chrome.
/// Read by `note_volume`; built from the constants above, never re-spelled.
const SAFETY_SECTIONS: [&str; 4] = [PERMISSIONS, DIRECTORIES, HOOKS, MCP];

/// Which safety-relevant sections `toml_text` sets, in a fixed order:
/// `[permissions] allow`, `[directories] allow`, `hooks.<phase>.<tool>`
/// keys, and MCP servers (`mcp = [...]` or `[mcp_servers.*]`).
///
/// Uses the startup parsers rather than a second grammar. The hook check mirrors
/// `parse_hooks_from_config`'s key shape without calling it, because that function
/// prints unknown-phase warnings — about a file this session is not loading.
pub(crate) fn unapplied_config_sections(toml_text: &str) -> Vec<&'static str> {
    let mut out = Vec::new();

    let perms = crate::config::parse_permissions_from_config(toml_text);
    if !perms.allow.is_empty() {
        out.push(PERMISSIONS);
    }

    let dirs = crate::config::parse_directories_from_config(toml_text);
    // Allow only: deny narrows and is applied on /cd. Allow would need an
    // intersection with the startup allow list, which is a design question.
    if !dirs.allow.is_empty() {
        out.push(DIRECTORIES);
    }

    let flat = crate::config::parse_config_file(toml_text);
    let has_hook = flat.iter().any(|(k, v)| {
        k.strip_prefix("hooks.")
            .and_then(|rest| rest.split_once('.'))
            .is_some_and(|(phase, tool)| {
                crate::hooks::HookPhase::parse(phase).is_some()
                    && !tool.is_empty()
                    && !v.trim().is_empty()
            })
    });
    if has_hook {
        out.push(HOOKS);
    }

    let flat_mcp = flat
        .get("mcp")
        .map(|raw| crate::config::parse_toml_array(raw))
        .unwrap_or_default();
    if !flat_mcp.is_empty() || !crate::config::parse_mcp_servers_from_config(toml_text).is_empty() {
        out.push(MCP);
    }

    out
}

/// Classify the result of reading `<dir>/.yoyo.toml`. `NotFound` is `Absent`;
/// any other error is `Unreadable` (never silently "sets nothing").
pub(crate) fn classify_read(read: std::io::Result<String>) -> CdConfigReading {
    match read {
        Ok(text) => CdConfigReading::Sets(
            unapplied_config_sections(&text),
            crate::config::parse_permissions_from_config(&text).deny,
            crate::config::parse_directories_from_config(&text).deny,
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => CdConfigReading::Absent,
        Err(_) => CdConfigReading::Unreadable,
    }
}

/// The one-line note, or `None` when there is nothing to disclose — no file, or
/// a file setting none of the four sections. `None` is the regression surface:
/// every `/cd` into a directory without such config prints exactly what it did before.
/// Glyph-free when `plain`. The path is repository-controlled, so it is sanitized.
pub(crate) fn cd_config_note(
    new_dir_display: &str,
    reading: &CdConfigReading,
    plain: bool,
) -> Option<String> {
    let dir = crate::cli::sanitize_for_display(new_dir_display);
    let sep = if plain { ":" } else { " —" };
    match reading {
        CdConfigReading::Absent => None,
        CdConfigReading::Sets(sections, deny, dir_deny)
            if !deny.is_empty() || !dir_deny.is_empty() =>
        {
            let mut applied = Vec::new();
            // Each applied list names the executor it fences and the one it
            // does not: neither list reaches the other kind of tool.
            for (n, what, one, many, scope, other) in [
                (deny.len(), "[permissions] deny", "pattern", "patterns", "bash commands", "file tools never read"),
                (dir_deny.len(), "[directories] deny", "entry", "entries", "the file tools", "bash never reads"),
            ] {
                let (noun, verb, them) = if n == 1 { (one, "is", "it") } else { (many, "are", "them") };
                if n > 0 {
                    applied.push(format!(
                        "its {n} {what} {noun} {verb} now enforced on {scope} for the rest of \
                         this session (another /cd does not remove {them}; {other} this list)"
                    ));
                }
            }
            let tail = if sections.is_empty() {
                "Its [permissions] allow, [directories] allow, hooks and MCP servers would still \
                 not be reloaded on /cd (#869)."
                    .to_string()
            } else {
                format!(
                    "It also sets {} and they are NOT applied{sep} the launch directory's \
                     settings stay in force. Restart yoyo here to use them (#869).",
                    sections.join(", ")
                )
            };
            Some(format!("  {dir}/.yoyo.toml: {}. {tail}", applied.join("; ")))
        }
        CdConfigReading::Sets(sections, _, _) if sections.is_empty() => None,
        CdConfigReading::Sets(sections, _, _) => Some(format!(
            "  {dir}/.yoyo.toml sets {} and they are NOT applied in this session{sep} \
             the launch directory's settings stay in force. Restart yoyo here to use them (#869).",
            sections.join(", ")
        )),
        CdConfigReading::Unreadable => Some(format!(
            "  {dir}/.yoyo.toml exists but could not be read, and NONE of it is applied in this \
             session{sep} the launch directory's settings stay in force. Restart yoyo here to use it (#869)."
        )),
    }
}

/// How loud the note is. Chosen from the note's own severity, never copied from
/// the DIM "project context is not reloaded" line printed just above it (#869,
/// Day 214): DIM reads as "skip this", and "your deny list is not in force"
/// is the one thing on that screen a user must not skip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoteVolume {
    /// Names at least one of `SAFETY_SECTIONS`: yellow, `warning:`-prefixed.
    Warning,
    /// Everything else: DIM, byte-identical to the Day-213 rendering.
    Chrome,
}

/// `Warning` iff the reading names a safety section. `Unreadable` names none —
/// we cannot tell what the file sets — so it stays `Chrome`, byte-identical to
/// before; raising it would warn on content we have not seen.
pub(crate) fn note_volume(reading: &CdConfigReading) -> NoteVolume {
    match reading {
        CdConfigReading::Sets(sections, deny, dir_deny)
            if !deny.is_empty()
                || !dir_deny.is_empty()
                || sections.iter().any(|s| SAFETY_SECTIONS.contains(s)) =>
        {
            NoteVolume::Warning
        }
        _ => NoteVolume::Chrome,
    }
}

/// The exact bytes written to stderr for `note` at `volume`. Pure apart from
/// the colour constants' own NO_COLOR handling. Under `plain` a warning carries
/// no glyph and no escape bytes; chrome is rendered exactly as Day 213 did.
pub(crate) fn render_cd_config_note(note: &str, volume: NoteVolume, plain: bool) -> String {
    use crate::format::{DIM, RESET, YELLOW};
    match volume {
        NoteVolume::Chrome => format!("{DIM}{note}{RESET}"),
        NoteVolume::Warning if plain => format!("warning: {}", note.trim_start()),
        NoteVolume::Warning => format!("{YELLOW}⚠ warning: {}{RESET}", note.trim_start()),
    }
}

/// Read and classify `<target>/.yoyo.toml`. Read-only; one read per `/cd`, so
/// the deny entries applied and the note printed come from the same bytes.
pub(crate) fn read_cd_config(target: &Path) -> CdConfigReading {
    classify_read(std::fs::read_to_string(target.join(".yoyo.toml")))
}

/// The `[permissions] deny` entries `/cd` applies; empty unless the file was read.
pub(crate) fn deny_entries(reading: &CdConfigReading) -> &[String] {
    match reading {
        CdConfigReading::Sets(_, deny, _) => deny,
        _ => &[],
    }
}

/// The `[directories] deny` entries `/cd` applies, resolved against `target`
/// NOW: a relative `deny = ["secrets"]` means `<target>/secrets`, and must keep
/// meaning that after a later `/cd` moves the process cwd elsewhere. Absolute
/// and `~` entries are kept as written (`check_path` expands `~` itself).
pub(crate) fn dir_deny_entries(target: &Path, reading: &CdConfigReading) -> Vec<String> {
    let CdConfigReading::Sets(_, _, dir_deny) = reading else {
        return Vec::new();
    };
    dir_deny
        .iter()
        .map(|e| {
            if e.starts_with('~') || Path::new(e).is_absolute() {
                e.clone()
            } else {
                target.join(e).to_string_lossy().into_owned()
            }
        })
        .collect()
}

/// Directories denied by `/cd` (#869, Day 226): process-wide, append-only,
/// read at call time by both path guards in `tool_wrappers.rs`. Same shape and
/// seam as `cd_deny`'s bash list; tests drive a local handle, never this one.
static CD_DENIED_DIRS: std::sync::LazyLock<crate::cd_deny::DenyAdditions> =
    std::sync::LazyLock::new(Default::default);

/// The `/cd` consumer: resolve and append. Never removes. Returns new count.
pub(crate) fn append_cd_dir_deny(target: &Path, reading: &CdConfigReading) -> usize {
    crate::cd_deny::append_deny_with(&CD_DENIED_DIRS, &dir_deny_entries(target, reading))
}

/// Refusal from the `/cd`-added directory list, or `None`. An empty list is
/// `None` with no filesystem call. Matching is `DirectoryRestrictions::check_path`
/// itself (symlinks, wildcards, unresolvable paths) — never a second matcher.
pub(crate) fn cd_denied_dir_refusal_with(
    list: &crate::cd_deny::DenyAdditions,
    path: &str,
) -> Option<String> {
    let deny = crate::sync_util::lock_or_recover(list).clone();
    if deny.is_empty() {
        return None;
    }
    let r = crate::config::DirectoryRestrictions {
        allow: vec![],
        deny,
    };
    r.check_path(path).err()
}

/// What both path guards ask: the startup fence first, byte-identical to
/// before, then whatever `/cd` has added so far.
pub(crate) fn path_guard_refusal_with(
    list: &crate::cd_deny::DenyAdditions,
    startup: &crate::config::DirectoryRestrictions,
    path: &str,
) -> Option<String> {
    startup
        .check_path(path)
        .err()
        .or_else(|| cd_denied_dir_refusal_with(list, path))
}

/// A clone of the process-wide `/cd` directory-deny handle, for readers outside
/// this module that compose their own refusal wording (`commands_file_fence`'s
/// `@path`/`/add`/`/explain`/`--image` doors, #869). Mirrors
/// `cd_deny::cd_added_deny_handle`.
pub(crate) fn cd_denied_dirs_handle() -> crate::cd_deny::DenyAdditions {
    std::sync::Arc::clone(&CD_DENIED_DIRS)
}

/// The production guard call, reading the process-wide `/cd` list.
pub(crate) fn path_guard_refusal(
    startup: &crate::config::DirectoryRestrictions,
    path: &str,
) -> Option<String> {
    path_guard_refusal_with(&CD_DENIED_DIRS, startup, path)
}

/// Print the note for an already-read `reading` to stderr.
pub(crate) fn print_cd_config_note(target: &Path, reading: &CdConfigReading) {
    let plain = crate::format::is_plain_output();
    let note = cd_config_note(&target.display().to_string(), reading, plain);
    // Deliberately NOT gated on quiet mode, for either volume. A warning is not
    // chrome: `--quiet` is auto-on whenever stdin and stdout are both piped, so a
    // quiet-gated warning is silent for exactly the runs nobody watches live —
    // the reason the max_tokens ceiling warning was un-gated on Day 211 (#964).
    // (The DIM note was never quiet-gated either; it stays as it was.)
    if let Some(note) = note {
        eprintln!(
            "{}",
            render_cd_config_note(&note, note_volume(reading), plain)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PERMS_TOML: &str = "model = \"x\"\n\n[permissions]\ndeny = [\"rm -rf *\"]\n";
    const ALLOW_TOML: &str = "[permissions]\nallow = [\"git *\"]\n";
    const DIR_DENY_TOML: &str = "[directories]\ndeny = [\"secrets\", \"/abs/vault\"]\n";

    fn sets(text: &str) -> CdConfigReading {
        classify_read(Ok(text.to_string()))
    }

    #[test]
    fn permissions_deny_is_named_as_enforced() {
        // Anti-vacuous: the fixture really carries a deny entry.
        assert!(PERMS_TOML.contains("deny"));
        // Day 218: deny is applied on /cd, so it is no longer an unapplied section.
        assert_eq!(unapplied_config_sections(PERMS_TOML), Vec::<&str>::new());
        let reading = sets(PERMS_TOML);
        assert_eq!(deny_entries(&reading), ["rm -rf *".to_string()]);
        let note = cd_config_note("/repo", &reading, false).expect("note");
        assert_eq!(
            note,
            "  /repo/.yoyo.toml: its 1 [permissions] deny pattern is now enforced on bash \
             commands for the rest of this session (another /cd does not remove it; file tools \
             never read this list). Its [permissions] allow, [directories] allow, hooks and MCP \
             servers would still not be reloaded on /cd (#869)."
        );
    }

    #[test]
    fn deny_plus_unapplied_sections_names_both() {
        let text = "[permissions]\nallow = [\"git *\"]\ndeny = [\"a *\", \"b *\"]\n\
                    hooks.pre.bash = \"echo\"\n";
        let reading = sets(text);
        assert_eq!(note_volume(&reading), NoteVolume::Warning);
        let note = cd_config_note("/r", &reading, true).unwrap();
        assert_eq!(
            note,
            "  /r/.yoyo.toml: its 2 [permissions] deny patterns are now enforced on bash \
             commands for the rest of this session (another /cd does not remove them; file tools \
             never read this list). It also sets [permissions] allow, hooks and they are NOT \
             applied: the launch directory's settings stay in force. Restart yoyo here to use \
             them (#869)."
        );
    }

    #[test]
    fn allow_only_keeps_the_day_213_wording() {
        // Near-miss: no deny entry, so nothing is applied and the old text stands.
        let reading = sets(ALLOW_TOML);
        assert!(deny_entries(&reading).is_empty());
        assert_eq!(
            cd_config_note("/repo", &reading, false).unwrap(),
            "  /repo/.yoyo.toml sets [permissions] allow and they are NOT applied in this \
             session — the launch directory's settings stay in force. Restart yoyo here to use \
             them (#869)."
        );
    }

    #[test]
    fn model_only_config_sets_nothing_and_prints_nothing() {
        let text = "provider = \"anthropic\"\nmodel = \"x\"\n";
        assert_eq!(unapplied_config_sections(text), Vec::<&str>::new());
        assert_eq!(sets(text), CdConfigReading::Sets(vec![], vec![], vec![]));
        assert_eq!(cd_config_note("/repo", &sets(text), false), None);
        assert_eq!(cd_config_note("/repo", &sets(text), true), None);
    }

    #[test]
    fn absent_file_prints_nothing() {
        let absent = classify_read(Err(std::io::Error::from(std::io::ErrorKind::NotFound)));
        assert_eq!(absent, CdConfigReading::Absent);
        assert_eq!(cd_config_note("/repo", &absent, false), None);
    }

    #[test]
    fn all_four_sections_in_fixed_order() {
        let text = "mcp = [\"npx server\"]\n\
                    hooks.pre.bash = \"echo hi\"\n\
                    [directories]\nallow = [\"src\"]\ndeny = [\"secrets\"]\n\
                    [permissions]\nallow = [\"git *\"]\n";
        assert_eq!(
            unapplied_config_sections(text),
            vec![
                "[permissions] allow",
                "[directories] allow",
                "hooks",
                "MCP servers"
            ]
        );
        let note = cd_config_note("/r", &sets(text), false).unwrap();
        assert!(
            note.contains(
                "sets [permissions] allow, [directories] allow, hooks, MCP servers and they are NOT"
            ),
            "{note}"
        );
        // Day 226: the deny half of the same section is reported as APPLIED.
        assert!(
            note.starts_with(
                "  /r/.yoyo.toml: its 1 [directories] deny entry is now enforced on the file tools"
            ),
            "{note}"
        );
    }

    #[test]
    fn structured_mcp_servers_section_counts() {
        let text = "[mcp_servers.fs]\ncommand = \"npx\"\n";
        assert_eq!(unapplied_config_sections(text), vec!["MCP servers"]);
    }

    #[test]
    fn unknown_hook_phase_is_not_a_hook() {
        // Near-miss: a key with the hooks prefix that startup would refuse.
        let text = "hooks.prr.bash = \"echo\"\n";
        assert_eq!(unapplied_config_sections(text), Vec::<&str>::new());
    }

    #[test]
    fn unreadable_is_distinct_from_empty() {
        let bad = classify_read(Err(std::io::Error::from(
            std::io::ErrorKind::PermissionDenied,
        )));
        assert_eq!(bad, CdConfigReading::Unreadable);
        assert_ne!(bad, CdConfigReading::Sets(vec![], vec![], vec![]));
        assert!(deny_entries(&bad).is_empty()); // nothing applied from unread bytes
        let note = cd_config_note("/repo", &bad, false).expect("unreadable must be reported");
        assert!(note.contains("could not be read"), "{note}");
    }

    #[test]
    fn non_utf8_file_on_disk_reads_as_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".yoyo.toml");
        std::fs::write(&path, [0xff, 0xfe, 0x00, 0x80]).unwrap();
        assert_eq!(
            classify_read(std::fs::read_to_string(&path)),
            CdConfigReading::Unreadable
        );
        // And a directory without the file is Absent.
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(
            classify_read(std::fs::read_to_string(empty.path().join(".yoyo.toml"))),
            CdConfigReading::Absent
        );
    }

    #[test]
    fn plain_output_is_glyph_free() {
        for reading in [sets(PERMS_TOML), CdConfigReading::Unreadable] {
            let note = cd_config_note("/repo", &reading, true).unwrap();
            assert!(!note.contains('—'), "{note}");
            assert!(!note.contains('•'), "{note}");
        }
    }

    #[test]
    fn hostile_path_is_sanitized() {
        let hostile = "/repo\x1b[2J";
        assert!(hostile.as_bytes().contains(&0x1b)); // anti-vacuous
        let note = cd_config_note(hostile, &sets(PERMS_TOML), false).unwrap();
        assert!(!note.as_bytes().contains(&0x1b), "{note:?}");
    }
    // --- Day 214: volume follows severity (#869) ---

    /// One fixture per safety section, each parsed through the real detector.
    const SAFETY_FIXTURES: [&str; 4] = [
        ALLOW_TOML,
        "[directories]\nallow = [\"src\"]\n",
        "hooks.pre.bash = \"echo hi\"\n",
        "[mcp_servers.fs]\ncommand = \"npx\"\n",
    ];

    #[test]
    fn every_safety_section_is_detected_and_warns() {
        assert!(!SAFETY_SECTIONS.is_empty()); // anti-vacuous
        let mut seen = Vec::new();
        for text in SAFETY_FIXTURES {
            let reading = sets(text);
            let CdConfigReading::Sets(found, _, _) = &reading else {
                panic!("{text:?} did not read as Sets");
            };
            assert_eq!(found.len(), 1, "{text:?} -> {found:?}"); // anti-vacuous
            seen.push(found[0]);
            assert_eq!(note_volume(&reading), NoteVolume::Warning, "{text:?}");
        }
        // The fixtures cover the whole list, derived from the detector's output.
        assert_eq!(seen, SAFETY_SECTIONS.to_vec());
        // A deny-only file names no unapplied section, and still warns: it
        // changes what bash may run for the rest of the session.
        assert_eq!(note_volume(&sets(PERMS_TOML)), NoteVolume::Warning);
        // Same for a [directories]-deny-only file (Day 226): it changes what
        // the file tools may touch, so it warns though it names no section.
        assert_eq!(note_volume(&sets(DIR_DENY_TOML)), NoteVolume::Warning);
    }

    #[test]
    fn safety_note_renders_as_yellow_warning_not_dim() {
        let reading = sets(PERMS_TOML);
        let note = cd_config_note("/repo", &reading, false).unwrap();
        let out = render_cd_config_note(&note, note_volume(&reading), false);
        assert!(
            out.starts_with("\x1b[33m⚠ warning: /repo/.yoyo.toml: its 1 [permissions] deny"),
            "{out:?}"
        );
        assert!(out.ends_with("\x1b[0m"), "{out:?}");
        assert!(!out.contains("\x1b[2m"), "{out:?}");
    }

    #[test]
    fn safety_note_plain_is_glyph_and_escape_free() {
        let reading = sets(PERMS_TOML);
        let note = cd_config_note("/repo", &reading, true).unwrap();
        let out = render_cd_config_note(&note, note_volume(&reading), true);
        assert!(
            out.starts_with("warning: /repo/.yoyo.toml: its 1 [permissions] deny"),
            "{out:?}"
        );
        assert!(!out.as_bytes().contains(&0x1b), "{out:?}");
        assert!(!out.contains('⚠') && !out.contains('—'), "{out:?}");
    }

    #[test]
    fn unreadable_note_stays_dim_and_byte_identical() {
        // Near-miss: the one note that names no safety section. Full-string
        // assert_eq against the Day-213 rendering, in both plain modes.
        let reading = CdConfigReading::Unreadable;
        assert_eq!(note_volume(&reading), NoteVolume::Chrome);
        let note = cd_config_note("/repo", &reading, false).unwrap();
        assert_eq!(
            render_cd_config_note(&note, note_volume(&reading), false),
            "\x1b[2m  /repo/.yoyo.toml exists but could not be read, and NONE of it is applied \
             in this session — the launch directory's settings stay in force. Restart yoyo here \
             to use it (#869).\x1b[0m"
        );
        let plain = cd_config_note("/repo", &reading, true).unwrap();
        assert_eq!(
            render_cd_config_note(&plain, note_volume(&reading), true),
            "\x1b[2m  /repo/.yoyo.toml exists but could not be read, and NONE of it is applied \
             in this session: the launch directory's settings stay in force. Restart yoyo here \
             to use it (#869).\x1b[0m"
        );
    }

    #[test]
    fn empty_and_absent_readings_are_chrome_and_print_nothing() {
        let model_only = sets("model = \"x\"\n");
        for r in [model_only, CdConfigReading::Absent] {
            assert_eq!(note_volume(&r), NoteVolume::Chrome);
            assert_eq!(cd_config_note("/repo", &r, false), None);
        }
    }

    /// Deliberately WEAK source guard: proves the print site does not consult
    /// quiet mode, never that the line reaches a terminal. A warning is not
    /// chrome and must not vanish under the auto-quiet of piped runs.
    #[test]
    fn print_site_is_not_quiet_gated() {
        let src = include_str!("cd_config_note.rs");
        let start = src.find("pub(crate) fn print_cd_config_note").unwrap();
        let end = start + src[start..].find("#[cfg(test)]").unwrap();
        let body = &src[start..end];
        assert!(body.contains("render_cd_config_note(")); // anti-vacuous
        let needle = ["is_", "quiet"].concat();
        assert!(
            !body.contains(&needle),
            "print_cd_config_note must not gate on quiet"
        );
    }

    // --- Day 226: [directories] deny applies to the file tools after /cd ---

    use crate::cd_deny::DenyAdditions;
    use crate::config::DirectoryRestrictions;

    fn local() -> DenyAdditions {
        Default::default()
    }

    fn tmp() -> (tempfile::TempDir, std::path::PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().canonicalize().unwrap();
        (d, p)
    }

    fn p(base: &Path, rel: &str) -> String {
        base.join(rel).to_string_lossy().into_owned()
    }

    #[test]
    fn dir_deny_note_says_applied_to_file_tools_and_allow_still_not() {
        let reading = sets(DIR_DENY_TOML);
        // Deny is applied, so it is no longer an unapplied section.
        assert_eq!(unapplied_config_sections(DIR_DENY_TOML), Vec::<&str>::new());
        assert_eq!(
            cd_config_note("/repo", &reading, false).unwrap(),
            "  /repo/.yoyo.toml: its 2 [directories] deny entries are now enforced on the file \
             tools for the rest of this session (another /cd does not remove them; bash never \
             reads this list). Its [permissions] allow, [directories] allow, hooks and MCP \
             servers would still not be reloaded on /cd (#869)."
        );
        // Both deny lists at once: each named, each with its own executor.
        let both = sets("[permissions]\ndeny = [\"rm *\"]\n[directories]\ndeny = [\"s\"]\n");
        assert_eq!(
            cd_config_note("/r", &both, true).unwrap(),
            "  /r/.yoyo.toml: its 1 [permissions] deny pattern is now enforced on bash commands \
             for the rest of this session (another /cd does not remove it; file tools never read \
             this list); its 1 [directories] deny entry is now enforced on the file tools for the \
             rest of this session (another /cd does not remove it; bash never reads this list). \
             Its [permissions] allow, [directories] allow, hooks and MCP servers would still not \
             be reloaded on /cd (#869)."
        );
    }

    #[test]
    fn relative_entry_is_resolved_against_the_cd_target_and_stays_there() {
        let (_a_guard, a) = tmp();
        let reading = sets(DIR_DENY_TOML);
        let entries = dir_deny_entries(&a, &reading);
        // Relative -> <target>/secrets; absolute kept as written.
        assert_eq!(entries, vec![p(&a, "secrets"), "/abs/vault".to_string()]);
        let list = local();
        assert_eq!(crate::cd_deny::append_deny_with(&list, &entries), 2);
        // A path in A/secrets is refused even though the process cwd is not A.
        assert_ne!(std::env::current_dir().unwrap(), a); // anti-vacuous: different base
        assert!(cd_denied_dir_refusal_with(&list, &p(&a, "secrets/key.txt")).is_some());
        // Near-miss: the same relative spelling under the CURRENT cwd is not
        // denied, because the entry was bound to A at append time.
        assert_eq!(cd_denied_dir_refusal_with(&list, "secrets/key.txt"), None);
        // `~` entries are left for check_path to expand, never joined.
        let tilde = sets("[directories]\ndeny = [\"~/vault\"]\n");
        assert_eq!(dir_deny_entries(&a, &tilde), vec!["~/vault".to_string()]);
        // Nothing applied from a reading that names no entries or was unread.
        assert!(dir_deny_entries(&a, &CdConfigReading::Unreadable).is_empty());
        assert!(dir_deny_entries(&a, &CdConfigReading::Absent).is_empty());
    }

    #[test]
    fn guard_refuses_a_cd_denied_dir_and_passes_near_misses() {
        let (_g, t) = tmp();
        let secrets = p(&t, "secrets");
        let list = local();
        crate::cd_deny::append_deny_with(&list, std::slice::from_ref(&secrets));
        let none = DirectoryRestrictions::default();
        let hit = p(&t, "secrets/a.txt");
        assert_eq!(
            path_guard_refusal_with(&list, &none, &hit),
            Some(format!(
                "Access denied: '{hit}' is under restricted directory '{secrets}'"
            ))
        );
        for near in [p(&t, "secretsXYZ/a"), p(&t, "other.txt")] {
            assert_eq!(path_guard_refusal_with(&list, &none, &near), None, "{near}");
        }
        // A second /cd elsewhere appends; it never removes the first entry.
        let (_g2, b) = tmp();
        crate::cd_deny::append_deny_with(&list, &dir_deny_entries(&b, &sets(DIR_DENY_TOML)));
        assert!(path_guard_refusal_with(&list, &none, &hit).is_some());
        assert!(path_guard_refusal_with(&list, &none, &p(&b, "secrets/x")).is_some());
    }

    #[test]
    fn empty_cd_list_leaves_the_startup_guard_byte_identical() {
        let (_g, t) = tmp();
        let empty = local();
        let startups = [
            DirectoryRestrictions::default(),
            DirectoryRestrictions {
                allow: vec![],
                deny: vec![p(&t, "secrets")],
            },
            DirectoryRestrictions {
                allow: vec![p(&t, "src")],
                deny: vec![],
            },
        ];
        let paths = [p(&t, "secrets/a"), p(&t, "src/main.rs"), p(&t, "other")];
        let mut refusals = 0;
        for r in &startups {
            for path in &paths {
                let before = r.check_path(path).err();
                refusals += usize::from(before.is_some());
                assert_eq!(
                    path_guard_refusal_with(&empty, r, path),
                    before,
                    "{r:?} {path}"
                );
            }
        }
        assert!(refusals > 0); // anti-vacuous: the table exercises refusals too
                               // And the startup refusal keeps its precedence over a /cd entry.
        let list = local();
        crate::cd_deny::append_deny_with(&list, &[p(&t, "secrets")]);
        assert_eq!(
            path_guard_refusal_with(&list, &startups[2], &p(&t, "secrets/a")),
            startups[2].check_path(&p(&t, "secrets/a")).err()
        );
    }

    /// Deliberately WEAK source guard: proves both path guards in
    /// tool_wrappers.rs call the /cd-aware refusal and that no direct
    /// startup-only check remains there, never that the call fires.
    #[test]
    fn both_path_guards_consult_the_cd_list() {
        let src = include_str!("tool_wrappers.rs");
        let call = ["cd_config_note::", "path_guard_refusal("].concat();
        assert_eq!(src.matches(&call).count(), 2, "both guards must call it");
        let direct = ["restrictions.", "check_path("].concat();
        assert_eq!(
            src.matches(&direct).count(),
            0,
            "a guard bypasses the /cd list"
        );
    }
}
