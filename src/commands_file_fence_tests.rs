//! #1002, the user's-own-prompt channel: `@path` mentions, `/add`, `/explain`
//! and the image reader honour `--deny-dir`/`--allow-dir` at the file's
//! RESOLVED target, through the one check in `read_file_for_add` /
//! `read_image_for_add`. Every fixture lives in a temp dir; the paths are
//! absolute, so nothing here reads or writes the repo cwd.
#![cfg(unix)]

use crate::commands_file::{
    build_explain_prompt, expand_file_mentions, handle_add, read_file_for_add, read_image_for_add,
    AddReadError, AddResult,
};
use crate::commands_file_fence::add_fence_refusal_message;
use crate::config::DirectoryRestrictions;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

const TOKEN: &str = "SECRET_MENTION_TOKEN_Q7";

/// `secret/notes.md` (holds TOKEN), `secret/pic.png`, `proj/` with
/// `proj/notes.md -> ../secret/notes.md`, `proj/pic.png -> ../secret/pic.png`,
/// an ordinary `proj/plain.md`, and `proj/inner.md -> plain.md` (a link whose
/// target is inside proj). All paths canonical.
struct Fixture {
    _tmp: tempfile::TempDir,
    secret: PathBuf,
    proj: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let secret = base.join("secret");
    let proj = base.join("proj");
    std::fs::create_dir_all(&secret).unwrap();
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(secret.join("notes.md"), format!("{TOKEN}\n")).unwrap();
    std::fs::write(secret.join("pic.png"), b"PNG_BYTES_SECRET").unwrap();
    std::fs::write(proj.join("plain.md"), "hello plain\n").unwrap();
    symlink("../secret/notes.md", proj.join("notes.md")).unwrap();
    symlink("../secret/pic.png", proj.join("pic.png")).unwrap();
    symlink("plain.md", proj.join("inner.md")).unwrap();
    Fixture {
        _tmp: tmp,
        secret,
        proj,
    }
}

fn deny(dir: &Path) -> DirectoryRestrictions {
    DirectoryRestrictions {
        allow: vec![],
        deny: vec![dir.display().to_string()],
    }
}

fn allow(dir: &Path) -> DirectoryRestrictions {
    DirectoryRestrictions {
        allow: vec![dir.display().to_string()],
        deny: vec![],
    }
}

fn none() -> DirectoryRestrictions {
    DirectoryRestrictions::default()
}

fn s(p: &Path) -> String {
    p.display().to_string()
}

/// The exact warning line a door receives for a deny refusal, spelled out
/// from literals (not from `check_path`) so the reason text is pinned too.
fn expected_deny_line(path: &str, denied: &str) -> String {
    let reason = format!("Access denied: '{path}' is under restricted directory '{denied}'");
    if crate::format::is_plain_output() {
        format!("warning: {path} not sent to the model - {reason}")
    } else {
        format!("⚠ {path} not sent to the model — {reason}")
    }
}

fn texts(results: &[AddResult]) -> Vec<String> {
    results
        .iter()
        .map(|r| match r {
            AddResult::Text { content, .. } => content.clone(),
            AddResult::Image { data, .. } => format!("IMAGE:{data}"),
        })
        .collect()
}

// ---------------------------------------------------------------------
// The composer, both plain values, whole string.
// ---------------------------------------------------------------------

#[test]
fn refusal_message_is_whole_line_in_both_modes() {
    assert_eq!(
        add_fence_refusal_message("a/b.md", "Access denied: 'a/b.md' is under restricted directory 'a'", false),
        "⚠ a/b.md not sent to the model — Access denied: 'a/b.md' is under restricted directory 'a'"
    );
    assert_eq!(
        add_fence_refusal_message("a/b.md", "Access denied: 'a/b.md' is under restricted directory 'a'", true),
        "warning: a/b.md not sent to the model - Access denied: 'a/b.md' is under restricted directory 'a'"
    );
}

#[test]
fn refusal_message_escapes_control_bytes() {
    let hostile = "x\u{1b}[31m.md";
    assert!(hostile.as_bytes().contains(&0x1b), "anti-vacuous");
    let out = add_fence_refusal_message(hostile, hostile, false);
    assert!(!out.as_bytes().contains(&0x1b), "{out:?}");
}

// ---------------------------------------------------------------------
// Refusals: a denied symlinked target, a direct denied path, allow-list.
// ---------------------------------------------------------------------

#[test]
fn read_file_for_add_refuses_symlink_into_denied_dir_with_exact_line() {
    let f = fixture();
    let link = s(&f.proj.join("notes.md"));
    let err = read_file_for_add(&link, None, &deny(&f.secret)).unwrap_err();
    assert_eq!(
        err,
        AddReadError::Refused(expected_deny_line(&link, &s(&f.secret)))
    );
    assert!(!err.to_string().contains(TOKEN));
}

#[test]
fn mention_of_symlink_into_denied_dir_is_not_inlined() {
    let f = fixture();
    let link = s(&f.proj.join("notes.md"));
    // Anti-vacuous: unfenced, this exact mention DOES inline the token.
    let (_, open) = expand_file_mentions(&format!("look @{link}"), &none());
    assert!(
        texts(&open).concat().contains(TOKEN),
        "fixture must leak unfenced"
    );

    let input = format!("look @{link} please");
    let (text, results) = expand_file_mentions(&input, &deny(&f.secret));
    assert!(results.is_empty(), "{:?}", texts(&results));
    assert_eq!(
        text, input,
        "the mention stays as typed; the rest goes through"
    );
    assert!(!text.contains(TOKEN));
}

#[test]
fn mention_direct_path_into_denied_dir_is_not_inlined() {
    let f = fixture();
    let direct = s(&f.secret.join("notes.md"));
    let input = format!("see @{direct}");
    let (text, results) = expand_file_mentions(&input, &deny(&f.secret));
    assert!(results.is_empty());
    assert_eq!(text, input);
}

#[test]
fn denied_mention_does_not_block_the_other_mentions() {
    let f = fixture();
    let link = s(&f.proj.join("notes.md"));
    let plain = s(&f.proj.join("plain.md"));
    let (text, results) = expand_file_mentions(&format!("@{link} and @{plain}"), &deny(&f.secret));
    assert_eq!(text, format!("@{link} and plain.md"));
    assert_eq!(
        texts(&results),
        vec![format!("**{plain}**\n```markdown\nhello plain\n\n```")]
    );
}

#[test]
fn add_refuses_symlink_and_direct_path_into_denied_dir() {
    let f = fixture();
    for p in [f.proj.join("notes.md"), f.secret.join("notes.md")] {
        let (results, added) = handle_add(&format!("/add {}", s(&p)), &deny(&f.secret));
        assert!(results.is_empty(), "{}: {:?}", s(&p), texts(&results));
        assert!(added.is_empty());
    }
}

#[test]
fn image_symlink_into_denied_dir_is_refused_on_both_doors() {
    let f = fixture();
    let link = s(&f.proj.join("pic.png"));
    let err = read_image_for_add(&link, &deny(&f.secret)).unwrap_err();
    assert_eq!(
        err,
        AddReadError::Refused(expected_deny_line(&link, &s(&f.secret)))
    );
    let (results, _) = handle_add(&format!("/add {link}"), &deny(&f.secret));
    assert!(results.is_empty());
    let (text, results) = expand_file_mentions(&format!("@{link}"), &deny(&f.secret));
    assert!(results.is_empty());
    assert_eq!(text, format!("@{link}"));
}

#[test]
fn allow_list_refuses_link_whose_target_is_outside_it() {
    let f = fixture();
    let link = s(&f.proj.join("notes.md"));
    let err = read_file_for_add(&link, None, &allow(&f.proj)).unwrap_err();
    assert!(matches!(err, AddReadError::Refused(_)), "{err:?}");
    let (_, results) = expand_file_mentions(&format!("@{link}"), &allow(&f.proj));
    assert!(results.is_empty(), "{:?}", texts(&results));
}

#[test]
fn explain_refuses_denied_target() {
    let f = fixture();
    let link = s(&f.proj.join("notes.md"));
    assert_eq!(
        build_explain_prompt(&format!("/explain {link}"), &deny(&f.secret)),
        None
    );
    // Anti-vacuous: unfenced, /explain inlines the token.
    let open = build_explain_prompt(&format!("/explain {link}"), &none()).unwrap();
    assert!(open.contains(TOKEN));
}

// ---------------------------------------------------------------------
// Near-misses: allowed files, allowed links, and no fence at all are
// byte-identical to the unrestricted read.
// ---------------------------------------------------------------------

#[test]
fn allowed_file_and_allowed_symlink_are_unchanged() {
    let f = fixture();
    let plain = s(&f.proj.join("plain.md"));
    let inner = s(&f.proj.join("inner.md"));
    for r in [deny(&f.secret), allow(&f.proj)] {
        for p in [&plain, &inner] {
            let input = format!("look @{p}");
            assert_eq!(
                expand_file_mentions(&input, &r).0,
                expand_file_mentions(&input, &none()).0
            );
            assert_eq!(
                texts(&expand_file_mentions(&input, &r).1),
                vec![format!("**{p}**\n```markdown\nhello plain\n\n```")]
            );
            let (res, added) = handle_add(&format!("/add {p}"), &r);
            assert_eq!(
                texts(&res),
                texts(&handle_add(&format!("/add {p}"), &none()).0)
            );
            assert_eq!(added, vec![p.clone()]);
            assert_eq!(
                read_file_for_add(p, None, &r),
                Ok(("hello plain\n".to_string(), 1))
            );
        }
    }
}

#[test]
fn no_restrictions_inline_exactly_as_before() {
    let f = fixture();
    let link = s(&f.proj.join("notes.md"));
    let (text, results) = expand_file_mentions(&format!("look @{link}"), &none());
    assert_eq!(text, "look notes.md");
    assert_eq!(
        texts(&results),
        vec![format!("**{link}**\n```markdown\n{TOKEN}\n\n```")]
    );
    let (res, added) = handle_add(&format!("/add {link}"), &none());
    assert_eq!(
        texts(&res),
        vec![format!("**{link}**\n```markdown\n{TOKEN}\n\n```")]
    );
    assert_eq!(added, vec![link]);
}

// ---------------------------------------------------------------------
// #869: the `/cd`-appended `[directories] deny` list reaches these doors.
// Driven through a per-thread override, never the process-wide list.
// ---------------------------------------------------------------------

use crate::cd_deny::DenyAdditions;
use crate::commands_file_fence::{admit_for_add_with, CD_DENY_SOURCE_NOTE, TEST_CD_DIR_DENY};

fn cd_list(entries: &[&Path]) -> DenyAdditions {
    std::sync::Arc::new(std::sync::Mutex::new(
        entries.iter().map(|p| s(p)).collect(),
    ))
}

/// Run `f` with the doors reading `list` as the `/cd` list on this thread.
fn with_cd_list<T>(list: DenyAdditions, f: impl FnOnce() -> T) -> T {
    TEST_CD_DIR_DENY.with(|c| *c.borrow_mut() = Some(list));
    let out = f();
    TEST_CD_DIR_DENY.with(|c| *c.borrow_mut() = None);
    out
}

/// The exact line for a `/cd`-list refusal, spelled out from literals.
fn expected_cd_deny_line(path: &str, denied: &str) -> String {
    let reason = format!(
        "Access denied: '{path}' is under restricted directory '{denied}' (a [directories] deny entry applied by /cd)"
    );
    if crate::format::is_plain_output() {
        format!("warning: {path} not sent to the model - {reason}")
    } else {
        format!("⚠ {path} not sent to the model — {reason}")
    }
}

#[test]
fn cd_deny_note_names_the_cd_source() {
    assert_eq!(
        CD_DENY_SOURCE_NOTE,
        "(a [directories] deny entry applied by /cd)"
    );
}

#[test]
fn cd_denied_dir_is_refused_with_exact_line_direct_and_via_symlink() {
    let f = fixture();
    let direct = s(&f.secret.join("notes.md"));
    let link = s(&f.proj.join("notes.md"));
    let list = cd_list(&[&f.secret]);
    for p in [&direct, &link] {
        // The startup fence is empty: only the `/cd` list can refuse.
        assert_eq!(
            admit_for_add_with(&list, p, &none()),
            Err(AddReadError::Refused(expected_cd_deny_line(
                p,
                &s(&f.secret)
            )))
        );
        let err = with_cd_list(list.clone(), || read_file_for_add(p, None, &none())).unwrap_err();
        assert_eq!(
            err,
            AddReadError::Refused(expected_cd_deny_line(p, &s(&f.secret)))
        );
        assert!(!err.to_string().contains(TOKEN));
    }
}

#[test]
fn cd_denied_mention_is_not_inlined() {
    let f = fixture();
    let link = s(&f.proj.join("notes.md"));
    let input = format!("look @{link} please");
    // Anti-vacuous: with an empty `/cd` list this mention DOES inline the token.
    let (_, open) = with_cd_list(cd_list(&[]), || expand_file_mentions(&input, &none()));
    assert!(texts(&open).concat().contains(TOKEN), "fixture must leak");

    let (text, results) = with_cd_list(cd_list(&[&f.secret]), || {
        expand_file_mentions(&input, &none())
    });
    assert!(results.is_empty(), "{:?}", texts(&results));
    assert_eq!(text, input);
    assert!(!text.contains(TOKEN));
}

#[test]
fn cd_denied_add_explain_and_image_are_refused() {
    let f = fixture();
    let link = s(&f.proj.join("notes.md"));
    let pic = s(&f.proj.join("pic.png"));
    let list = cd_list(&[&f.secret]);
    let (res, added) = with_cd_list(list.clone(), || {
        handle_add(&format!("/add {link}"), &none())
    });
    assert!(res.is_empty(), "{:?}", texts(&res));
    assert!(added.is_empty());
    assert_eq!(
        with_cd_list(list.clone(), || build_explain_prompt(
            &format!("/explain {link}"),
            &none()
        )),
        None
    );
    assert_eq!(
        with_cd_list(list, || read_image_for_add(&pic, &none())).unwrap_err(),
        AddReadError::Refused(expected_cd_deny_line(&pic, &s(&f.secret)))
    );
}

#[test]
fn startup_fence_wins_and_keeps_its_wording_when_both_deny() {
    let f = fixture();
    let link = s(&f.proj.join("notes.md"));
    assert_eq!(
        admit_for_add_with(&cd_list(&[&f.secret]), &link, &deny(&f.secret)),
        Err(AddReadError::Refused(expected_deny_line(
            &link,
            &s(&f.secret)
        )))
    );
}

#[test]
fn empty_cd_list_is_byte_identical_and_cd_list_elsewhere_is_inert() {
    let f = fixture();
    let link = s(&f.proj.join("notes.md"));
    let plain = s(&f.proj.join("plain.md"));
    let elsewhere = tempfile::TempDir::new().unwrap();
    for r in [none(), deny(&f.secret), allow(&f.proj)] {
        for p in [&link, &plain] {
            let input = format!("look @{p}");
            let base_mention = expand_file_mentions(&input, &r);
            let base_add = handle_add(&format!("/add {p}"), &r);
            let base_read = read_file_for_add(p, None, &r);
            for list in [cd_list(&[]), cd_list(&[elsewhere.path()])] {
                let (text, res) = with_cd_list(list.clone(), || expand_file_mentions(&input, &r));
                assert_eq!(text, base_mention.0);
                assert_eq!(texts(&res), texts(&base_mention.1));
                let (ares, aadded) =
                    with_cd_list(list.clone(), || handle_add(&format!("/add {p}"), &r));
                assert_eq!(texts(&ares), texts(&base_add.0));
                assert_eq!(aadded, base_add.1);
                assert_eq!(
                    with_cd_list(list.clone(), || read_file_for_add(p, None, &r)),
                    base_read
                );
            }
        }
    }
}
