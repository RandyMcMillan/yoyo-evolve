//! The directory fence for skill directories (#1002, the skills door).
//!
//! Skills reach the provider request through yoagent's `with_skills`, which
//! renders every loaded skill's `name`, `description` and `location` into the
//! system prompt at agent-build time. Measured Day 226
//! (`tests/skills_fence_request.rs`, request-body sink): a skill whose
//! `SKILL.md` sat under a `--deny-dir` target reached the request in both the
//! symlinked-dir and the plain-dir shape. This module puts every skill
//! directory to the same resolver the file tools and the instruction loader
//! use — `DirectoryRestrictions::check_path`, which judges a symlink at its
//! target — BEFORE yoagent reads it.
//!
//! **Granularity is the whole directory, and that is forced, not chosen.**
//! yoagent's `SkillSet` has no public constructor from a `Vec<Skill>` and no
//! removal API, so a set cannot be loaded and then thinned. A directory with
//! any refused skill is therefore not loaded at all, and the warning says so
//! and names every refused file — the same whole-source refusal
//! `config_paths::partition_escaped_skills` already makes for symlink escapes.
//! With no restrictions (`is_empty`), `check_path` short-circuits and nothing
//! is refused, so the common case is byte-identical to before.

use crate::config::DirectoryRestrictions;
use std::path::{Path, PathBuf};

/// The `SKILL.md` files yoagent's loader would pick up in `dir`: one per
/// immediate subdirectory holding a `SKILL.md` (the same enumeration as
/// yoagent's `skill_paths_resilient`). Sorted for a stable warning. An
/// unreadable `dir` yields nothing — yoagent reports that itself.
fn skill_md_candidates(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.join("SKILL.md").exists())
        .map(|p| p.join("SKILL.md"))
        .collect();
    out.sort();
    out
}

/// Every skill file in `dir` the fence refuses, as `(path, reason)`. Empty
/// means the directory may be loaded unchanged. Only files that exist are
/// asked, so an absent or empty directory is never a refusal.
pub(crate) fn refused_skill_files(
    dir: &Path,
    restrictions: &DirectoryRestrictions,
) -> Vec<(String, String)> {
    if restrictions.is_empty() {
        return Vec::new();
    }
    skill_md_candidates(dir)
        .into_iter()
        .filter_map(|p| {
            let s = p.to_string_lossy().into_owned();
            restrictions.check_path(&s).err().map(|reason| (s, reason))
        })
        .collect()
}

/// The warning printed when the fence refuses a skill directory. A security
/// fact, so a normal warning — never DIM, never quiet-gated — worded like
/// `context::instruction_file_refused_warning`. Every interpolated string is
/// repository- or user-authored, so each goes through `sanitize_for_display`;
/// plain output is glyph-free.
pub(crate) fn skill_dir_refused_warning(
    dir: &str,
    refused: &[(String, String)],
    plain: bool,
) -> String {
    let dir = crate::cli::sanitize_for_display(dir);
    let reasons: Vec<String> = refused
        .iter()
        .map(|(_, reason)| crate::cli::sanitize_for_display(reason))
        .collect();
    let n = refused.len();
    let noun = if n == 1 { "skill" } else { "skills" };
    if plain {
        format!(
            "warning: skill directory {dir} not loaded - {n} {noun} under the directory fence, so none of its skills were loaded: {}",
            reasons.join("; ")
        )
    } else {
        format!(
            "⚠ skill directory {dir} not loaded — {n} {noun} under the directory fence, so none of its skills were loaded: {}",
            reasons.join("; ")
        )
    }
}

/// Ask the fence about `dir`; on refusal print the warning and return
/// `false` (do not load). The one call site shape for every skill source.
pub(crate) fn admit_skill_dir(
    dir: &Path,
    display: &str,
    restrictions: &DirectoryRestrictions,
) -> bool {
    let refused = refused_skill_files(dir, restrictions);
    if refused.is_empty() {
        return true;
    }
    let msg = skill_dir_refused_warning(display, &refused, crate::format::is_plain_output());
    eprintln!("{}{msg}{}", crate::format::YELLOW, crate::format::RESET);
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plant(root: &Path, rel: &str) {
        let d = root.join(rel);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("SKILL.md"), "---\nname: x\ndescription: y\n---\n").unwrap();
    }

    fn deny(dir: &Path) -> DirectoryRestrictions {
        DirectoryRestrictions {
            allow: vec![],
            deny: vec![dir.to_string_lossy().into_owned()],
        }
    }

    #[test]
    fn no_restrictions_refuses_nothing() {
        let t = tempfile::tempdir().unwrap();
        plant(t.path(), "skills/a");
        assert!(
            refused_skill_files(&t.path().join("skills"), &DirectoryRestrictions::default())
                .is_empty()
        );
    }

    #[test]
    fn plain_dir_under_deny_is_refused_and_named() {
        let t = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(t.path()).unwrap();
        plant(&root, "skills/a");
        plant(&root, "skills/b");
        let r = refused_skill_files(&root.join("skills"), &deny(&root.join("skills")));
        assert_eq!(r.len(), 2, "{r:?}");
        assert!(r[0].0.ends_with("a/SKILL.md"), "{r:?}");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_dir_is_judged_at_its_target() {
        let t = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(t.path()).unwrap();
        plant(&root, "secret/skills/canary");
        std::os::unix::fs::symlink(root.join("secret/skills"), root.join("linked")).unwrap();
        let r = refused_skill_files(&root.join("linked"), &deny(&root.join("secret")));
        assert_eq!(r.len(), 1, "{r:?}");
    }

    #[test]
    fn near_miss_dir_outside_deny_is_admitted() {
        let t = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(t.path()).unwrap();
        plant(&root, "skills/a");
        std::fs::create_dir_all(root.join("secret")).unwrap();
        assert!(refused_skill_files(&root.join("skills"), &deny(&root.join("secret"))).is_empty());
        // Sibling-prefix near miss: `secret` must not match `secretive`.
        plant(&root, "secretive/a");
        assert!(
            refused_skill_files(&root.join("secretive"), &deny(&root.join("secret"))).is_empty()
        );
    }

    #[test]
    fn absent_dir_is_never_a_refusal() {
        let t = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(t.path()).unwrap();
        assert!(refused_skill_files(&root.join("nope"), &deny(&root)).is_empty());
    }

    #[test]
    fn warning_names_dir_and_reason_sanitized_and_plain_is_glyph_free() {
        let refused = vec![(
            "/p/SKILL.md".to_string(),
            "Access denied: '/p/SKILL.md' is under restricted directory '/p\x1b[31m'".to_string(),
        )];
        let plain = skill_dir_refused_warning(".yoyo/skills/", &refused, true);
        assert_eq!(
            plain,
            "warning: skill directory .yoyo/skills/ not loaded - 1 skill under the directory fence, so none of its skills were loaded: Access denied: '/p/SKILL.md' is under restricted directory '/p\\x1b[31m'"
        );
        assert!(!plain.as_bytes().contains(&0x1b));
        assert!(plain.is_ascii());
        let fancy = skill_dir_refused_warning("d\x1b", &refused, false);
        assert!(!fancy.as_bytes().contains(&0x1b));
        assert!(fancy.starts_with("⚠ skill directory d"));
    }
}
