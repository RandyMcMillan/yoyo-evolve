//! The directory fence for files the user names into a prompt (#1002):
//! `@path` mentions, `/add`, `/explain` and `--image`. Kept apart from
//! `commands_file.rs` so the one check and its refusal wording live together.

use crate::format::{RED, RESET, YELLOW};

/// Why a file the user named (`/add`, `@path`, `/explain`, `--image`) was not
/// read. The two arms are rendered differently: a fence refusal is a yellow
/// warning (the fence working as configured), a failed read a red `✗` (#1002).
#[derive(Debug, Clone, PartialEq)]
pub enum AddReadError {
    /// The session's `--deny-dir`/`--allow-dir` fence refused the path at its
    /// resolved target. Carries the full, glyph-correct warning line.
    Refused(String),
    /// The read itself failed (missing file, permissions, bad range, ...).
    Failed(String),
}

impl std::fmt::Display for AddReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AddReadError::Refused(m) | AddReadError::Failed(m) => f.write_str(m),
        }
    }
}

/// The warning printed when the directory fence refuses a user-named file.
/// `reason` is `DirectoryRestrictions::check_path`'s own text, which already
/// names the path and the fence entry; both strings are user/config-authored,
/// so they are sanitized before reaching the terminal.
pub(crate) fn add_fence_refusal_message(path: &str, reason: &str, plain: bool) -> String {
    let path = crate::cli::sanitize_for_display(path);
    let reason = crate::cli::sanitize_for_display(reason);
    if plain {
        format!("warning: {path} not sent to the model - {reason}")
    } else {
        format!("⚠ {path} not sent to the model — {reason}")
    }
}

/// The one fence check for every door that inlines a user-named file into a
/// prompt (#1002): `/add`, `@path` mentions, `/explain` and `--image` all
/// read through `commands_file::read_file_for_add` / `read_image_for_add`, which call
/// this before touching the bytes. It asks `DirectoryRestrictions::check_path`
/// — the resolver the file tools and prompt loaders use — so a symlink is
/// judged at its target. It also asks the `[directories] deny` entries `/cd`
/// has appended (#869), so these doors refuse what `read_file` refuses after a
/// `/cd`. With no restrictions and an empty `/cd` list it short-circuits to `Ok`.
pub(crate) fn admit_for_add(
    path: &str,
    restrictions: &crate::config::DirectoryRestrictions,
) -> Result<(), AddReadError> {
    admit_for_add_with(&cd_dir_deny_list(), path, restrictions)
}

/// [`admit_for_add`] with the `/cd`-appended `[directories] deny` list passed
/// in (#869). The startup fence is asked first and its verdict and wording are
/// byte-identical to before; only a path it admits is then asked of the `/cd`
/// list, through `cd_config_note::cd_denied_dir_refusal_with` — the same
/// predicate (and so the same `check_path` matching: symlinks, wildcards,
/// unresolvable paths) the file tools' `path_guard_refusal` uses. It is
/// called directly rather than through `path_guard_refusal_with` only so the
/// refusal can say which list refused. An empty `/cd` list adds no check.
pub(crate) fn admit_for_add_with(
    cd_list: &crate::cd_deny::DenyAdditions,
    path: &str,
    restrictions: &crate::config::DirectoryRestrictions,
) -> Result<(), AddReadError> {
    let plain = crate::format::is_plain_output();
    if let Err(reason) = restrictions.check_path(path) {
        return Err(AddReadError::Refused(add_fence_refusal_message(
            path, &reason, plain,
        )));
    }
    if let Some(reason) = crate::cd_config_note::cd_denied_dir_refusal_with(cd_list, path) {
        return Err(AddReadError::Refused(add_fence_refusal_message(
            path,
            &format!("{reason} {CD_DENY_SOURCE_NOTE}"),
            plain,
        )));
    }
    Ok(())
}

/// Appended to a refusal that came from the `/cd`-applied list, so the user
/// can tell it apart from a launch `--deny-dir` / startup-config refusal.
pub(crate) const CD_DENY_SOURCE_NOTE: &str = "(a [directories] deny entry applied by /cd)";

#[cfg(test)]
thread_local! {
    /// Test-only, per-thread override of the `/cd` list the doors read, so a
    /// door-level test (`expand_file_mentions`, `handle_add`, ...) can drive a
    /// local list without writing the process-wide one. `None` = production.
    pub(crate) static TEST_CD_DIR_DENY: std::cell::RefCell<Option<crate::cd_deny::DenyAdditions>> =
        const { std::cell::RefCell::new(None) };
}

/// The `/cd` list the doors consult: the process-wide handle in production.
fn cd_dir_deny_list() -> crate::cd_deny::DenyAdditions {
    #[cfg(test)]
    if let Some(list) = TEST_CD_DIR_DENY.with(|c| c.borrow().clone()) {
        return list;
    }
    crate::cd_config_note::cd_denied_dirs_handle()
}

/// The coloured line a door prints when a user-named file was not read: a
/// fence refusal is a yellow warning, a failed read keeps its red `✗`.
pub(crate) fn add_read_error_line(e: &AddReadError) -> String {
    match e {
        AddReadError::Refused(m) => format!("{YELLOW}  {m}{RESET}"),
        AddReadError::Failed(m) => format!("{RED}  ✗ {m}{RESET}"),
    }
}
