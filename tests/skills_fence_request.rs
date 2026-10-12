//! #1002, the skills door: does a skill whose files sit under a `--deny-dir`
//! target reach the provider REQUEST?
//!
//! Day 221/222 recorded that `--print-system-prompt` is the wrong sink for this
//! question: skills are handed to yoagent via `with_skills` and rendered into
//! the system prompt at agent-build time, so an unfenced skill also read 0 hits
//! there. This file measures at the only sink that settles it — the HTTP body
//! the real binary POSTs to a stub provider — with an anti-vacuous control (the
//! same fixture with NO fence must carry the canary) so a zero can never be a
//! blind sink again.
//!
//! The stub is a copy of `tests/print_stdout_contract.rs`'s `start_stub_seq`,
//! cut down and extended to keep each request body. It is copied rather than
//! shared because that file's helper does not capture bodies and changing its
//! signature would touch ~60 unrelated tests.
//!
//! Every fixture lives under a tempdir with its own HOME/XDG, never the repo.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// The token planted in the skill's frontmatter `description`. Unique enough
/// that nothing else in a request body can contain it.
const CANARY: &str = "SKILL_CANARY_ZZ7";

fn sse_body() -> String {
    let events: Vec<(&str, String)> = vec![
        (
            "message_start",
            r#"{"type":"message_start","message":{"id":"msg_s","type":"message","role":"assistant","content":[],"model":"claude-sonnet-4-5","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":10,"output_tokens":1}}}"#.to_string(),
        ),
        (
            "content_block_start",
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#.to_string(),
        ),
        (
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ok"}}"#.to_string(),
        ),
        (
            "content_block_stop",
            r#"{"type":"content_block_stop","index":0}"#.to_string(),
        ),
        (
            "message_delta",
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":1}}"#.to_string(),
        ),
        ("message_stop", r#"{"type":"message_stop"}"#.to_string()),
    ];
    events
        .into_iter()
        .map(|(e, d)| format!("event: {e}\ndata: {d}\n\n"))
        .collect()
}

/// A stub on 127.0.0.1:0 answering every POST with `sse_body()` and recording
/// each POST body. Returns the port and the shared body store.
fn start_capturing_stub() -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub");
    let port = listener.local_addr().unwrap().port();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let store = Arc::clone(&bodies);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Ok(clone) = stream.try_clone() else {
                continue;
            };
            let mut reader = BufReader::new(clone);
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).is_err() {
                continue;
            }
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let trimmed = line.trim_end();
                if trimmed.is_empty() {
                    break;
                }
                if let Some((k, v)) = trimmed.split_once(':') {
                    if k.eq_ignore_ascii_case("content-length") {
                        content_length = v.trim().parse().unwrap_or(0);
                    }
                }
            }
            let mut body = vec![0u8; content_length];
            let _ = reader.read_exact(&mut body);
            if request_line.starts_with("POST") {
                store
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&body).into_owned());
            }
            let payload = sse_body();
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                payload.len(),
                payload
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
    });
    (port, bodies)
}

struct Probe {
    /// Every POST body the stub received, concatenated.
    request: String,
    posts: usize,
    stderr: String,
}

impl Probe {
    fn hits(&self) -> usize {
        self.request.matches(CANARY).count()
    }
}

fn skill_md() -> String {
    format!(
        "---\nname: canary\ndescription: Planted skill whose description carries {CANARY} for the fence probe.\n---\n\n# Canary\n\nBody text {CANARY}.\n"
    )
}

/// Run `yoyo -p hi` from `proj` against a fresh capturing stub.
fn run_in(proj: &Path, home: &Path, extra: &[&str]) -> Probe {
    let (port, bodies) = start_capturing_stub();
    let base_url = format!("http://127.0.0.1:{port}/v1");
    let mut child = Command::new(env!("CARGO_BIN_EXE_yoyo"))
        .args([
            "--provider",
            "anthropic",
            "--model",
            "claude-sonnet-4-5",
            "--base-url",
            &base_url,
            "--no-tools",
            "--max-turns",
            "1",
        ])
        .args(extra)
        .args(["-p", "hi"])
        .current_dir(proj)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("ANTHROPIC_API_KEY", "sk-ant-dummy-test-key")
        .env_remove("API_KEY")
        .env_remove("MODEL")
        .env_remove("YOYO_MODEL")
        .env_remove("YOYO_PROVIDER")
        .env_remove("YOYO_RESTRICTED")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn yoyo");
    let deadline = Instant::now() + Duration::from_secs(30);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("yoyo did not exit within 30s");
        }
        thread::sleep(Duration::from_millis(50));
    }
    let out = child.wait_with_output().unwrap();
    let all = bodies.lock().unwrap();
    Probe {
        request: all.join("\n"),
        posts: all.len(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// `root/proj` with `proj/.yoyo/skills -> ../secret/skills`, the canary living
/// in `proj/secret/skills/canary/SKILL.md`. Returns (canonical proj, canonical
/// secret dir). Canonical because macOS tempdirs sit behind a `/var` link.
#[cfg(unix)]
fn symlinked_fixture(root: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let proj = root.join("proj");
    let skill_dir = proj.join("secret/skills/canary");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), skill_md()).unwrap();
    std::fs::create_dir_all(proj.join(".yoyo")).unwrap();
    std::os::unix::fs::symlink("../secret/skills", proj.join(".yoyo/skills")).unwrap();
    let proj = std::fs::canonicalize(&proj).unwrap();
    let secret = proj.join("secret");
    (proj, secret)
}

/// `root/proj/.yoyo/skills/canary/SKILL.md`, a plain directory. Returns the
/// canonical proj path.
fn direct_fixture(root: &Path) -> std::path::PathBuf {
    let proj = root.join("proj");
    let skill_dir = proj.join(".yoyo/skills/canary");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), skill_md()).unwrap();
    std::fs::canonicalize(&proj).unwrap()
}

fn assert_posted(p: &Probe, label: &str) {
    assert!(
        p.posts > 0,
        "anti-vacuous ({label}): the stub never received a POST, so a zero means nothing; stderr={}",
        p.stderr
    );
}

#[cfg(unix)]
#[test]
fn symlinked_skills_dir_into_denied_dir_does_not_reach_the_request() {
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let (proj, secret) = symlinked_fixture(root.path());
    let secret_s = secret.to_str().unwrap();
    let proj_s = proj.to_str().unwrap();

    let open = run_in(&proj, home.path(), &[]);
    let denied = run_in(&proj, home.path(), &["--deny-dir", secret_s]);
    let allowed = run_in(&proj, home.path(), &["--allow-dir", proj_s]);
    eprintln!(
        "PROBE symlink: no-fence={} deny={} allow-proj={}",
        open.hits(),
        denied.hits(),
        allowed.hits()
    );
    for (p, l) in [(&open, "open"), (&denied, "deny"), (&allowed, "allow")] {
        assert_posted(p, l);
    }
    // Anti-vacuous control: with no fence the canary MUST be in the body, or
    // this sink is as blind as --print-system-prompt was.
    assert!(
        open.hits() > 0,
        "control: unfenced skill canary absent from the request body; the sink is wrong. stderr={}",
        open.stderr
    );
    assert_eq!(denied.hits(), 0, "deny stderr={}", denied.stderr);
    assert_refusal_warned(&denied);
    // Near-miss: allow-dir covering the target still loads the skill, and the
    // request is byte-identical to the unfenced one.
    assert_eq!(
        allowed.hits(),
        open.hits(),
        "allow stderr={}",
        allowed.stderr
    );
    assert_eq!(
        allowed.request, open.request,
        "allow-dir must not change the request"
    );
    assert!(
        !allowed.stderr.contains("skill directory"),
        "{}",
        allowed.stderr
    );
    // Near-miss: a deny entry that does NOT cover the skill changes nothing.
    let elsewhere = proj.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let other = run_in(
        &proj,
        home.path(),
        &["--deny-dir", elsewhere.to_str().unwrap()],
    );
    assert_posted(&other, "deny-elsewhere");
    assert_eq!(
        other.request, open.request,
        "unrelated deny must not change the request"
    );
}

/// The refusal is loud: one normal warning naming the skill directory.
fn assert_refusal_warned(p: &Probe) {
    assert!(
        p.stderr
            .contains("skill directory .yoyo/skills/ not loaded"),
        "a fenced skill dir must be named on stderr; got: {}",
        p.stderr
    );
}

#[test]
fn direct_skills_dir_under_denied_dir_does_not_reach_the_request() {
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let proj = direct_fixture(root.path());
    let skills = proj.join(".yoyo/skills");
    let skills_s = skills.to_str().unwrap();

    let open = run_in(&proj, home.path(), &[]);
    let denied = run_in(&proj, home.path(), &["--deny-dir", skills_s]);
    eprintln!(
        "PROBE direct: no-fence={} deny={}",
        open.hits(),
        denied.hits()
    );
    assert_posted(&open, "open");
    assert_posted(&denied, "deny");
    assert!(
        open.hits() > 0,
        "control: unfenced skill canary absent from the request body; the sink is wrong. stderr={}",
        open.stderr
    );
    assert_eq!(denied.hits(), 0, "deny stderr={}", denied.stderr);
    assert_refusal_warned(&denied);
}

/// The flag door: `--skills <dir>` is fenced by the same check, so passing a
/// denied directory explicitly does not smuggle its skills into the request.
#[test]
fn skills_flag_dir_under_denied_dir_does_not_reach_the_request() {
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(root.path()).unwrap();
    let proj = root.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let skill_dir = root.join("secret/skills/canary");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), skill_md()).unwrap();
    let skills = root.join("secret/skills");
    let skills_s = skills.to_str().unwrap();
    let secret = root.join("secret");

    let open = run_in(&proj, home.path(), &["--skills", skills_s]);
    let denied = run_in(
        &proj,
        home.path(),
        &["--skills", skills_s, "--deny-dir", secret.to_str().unwrap()],
    );
    eprintln!(
        "PROBE --skills: no-fence={} deny={}",
        open.hits(),
        denied.hits()
    );
    assert_posted(&open, "open");
    assert_posted(&denied, "deny");
    assert!(open.hits() > 0, "control: stderr={}", open.stderr);
    assert_eq!(denied.hits(), 0, "deny stderr={}", denied.stderr);
    assert!(
        denied.stderr.contains("not loaded"),
        "the flag door must warn too: {}",
        denied.stderr
    );
}
