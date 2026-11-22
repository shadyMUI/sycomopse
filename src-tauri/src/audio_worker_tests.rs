use super::*;
use serde_json::{json, Value};

fn node() -> NodeIdentity {
    NodeIdentity {
        id: 30,
        object_serial: u64::MAX.to_string(),
        node_name: "chosen.\"\\'; os.execute('bad') --.sink".into(),
        media_class: "Audio/Sink".into(),
    }
}

fn level(volume: f64, muted: bool) -> Request {
    Request::new(Operation::SetLevel {
        node: node(),
        core_cookie: u32::MAX.to_string(),
        volume: Some(volume),
        muted: Some(muted),
    })
    .unwrap()
}

fn default(node: Option<NodeIdentity>, baseline: Option<&str>) -> Request {
    Request::new(Operation::SetDefault {
        node,
        core_cookie: u32::MAX.to_string(),
        expected_configured: baseline.map(str::to_owned),
    })
    .unwrap()
}

#[test]
fn request_schema_is_closed_and_bounded() {
    let value = serde_json::to_value(level(0.25, true)).unwrap();
    Request::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
    for path in [
        vec!["shell"],
        vec!["request", "executable"],
        vec!["request", "node", "command"],
    ] {
        let mut invalid = value.clone();
        let mut object = &mut invalid;
        for part in &path[..path.len() - 1] {
            object = &mut object[*part];
        }
        object[path[path.len() - 1]] = json!("wpctl");
        assert_eq!(
            Request::parse(&serde_json::to_vec(&invalid).unwrap())
                .unwrap_err()
                .code,
            ErrorCode::Validation
        );
    }
    let mut invalid = value.clone();
    invalid["schemaVersion"] = json!(2);
    assert!(Request::parse(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = value;
    invalid["request"]["operation"] = json!("run");
    assert!(Request::parse(&serde_json::to_vec(&invalid).unwrap()).is_err());
    assert!(Request::parse(br#"{"schemaVersion":1,"schemaVersion":1,"request":{"operation":"validateSofa","path":[47,97]}}"#).is_err());
    let mut bytes = serde_json::to_vec(&level(0.25, false)).unwrap();
    bytes.extend_from_slice(b" {}");
    assert!(Request::parse(&bytes).is_err());
    assert!(Request::parse(&vec![b' '; MAX_REQUEST_BYTES + 1]).is_err());
    let value = serde_json::to_value(default(None, None)).unwrap();
    for field in ["node", "expectedConfigured"] {
        let mut invalid = value.clone();
        invalid["request"].as_object_mut().unwrap().remove(field);
        assert!(Request::parse(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
}

#[test]
fn exact_decimal_identity_and_safe_level_ranges() {
    assert_eq!(decimal(&u64::MAX.to_string(), u64::MAX).unwrap(), u64::MAX);
    for invalid in [
        "",
        "+1",
        "-1",
        "01",
        "1.0",
        "1e3",
        " 1",
        "1 ",
        "18446744073709551616",
    ] {
        assert!(decimal(invalid, u64::MAX).is_err(), "{invalid}");
    }
    assert!(decimal("4294967296", u32::MAX as u64).is_err());
    for volume in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.001, 1.001] {
        assert!(Request::new(Operation::SetLevel {
            node: node(),
            core_cookie: "1".into(),
            volume: Some(volume),
            muted: Some(false),
        })
        .is_err());
    }
    for id in [0, u32::MAX] {
        let mut node = node();
        node.id = id;
        assert!(node.validate().is_err());
    }
    for class in ["Audio/SinkFake", "Stream/Output/Audio", "Video/Source"] {
        let mut node = node();
        node.media_class = class.into();
        assert!(node.validate().is_err());
    }
    let mut source = node();
    source.media_class = "Audio/Source".into();
    source.validate().unwrap();
    assert!(Request::new(Operation::SetDefault {
        node: Some(source),
        core_cookie: "1".into(),
        expected_configured: None,
    })
    .is_err());
    assert!(Request::new(Operation::SetLevel {
        node: node(),
        core_cookie: "1".into(),
        volume: None,
        muted: None,
    })
    .is_err());
}

#[test]
fn sofa_paths_are_bounded_binary_data_not_commands() {
    Request::new(Operation::ValidateSofa {
        path: b"/tmp/non-utf8-\xff.sofa".to_vec(),
    })
    .unwrap();
    for path in [
        b"relative.sofa".to_vec(),
        b"/tmp/a\0.sofa".to_vec(),
        Vec::new(),
        vec![b'/'; MAX_PATH_BYTES + 1],
    ] {
        assert!(Request::new(Operation::ValidateSofa { path }).is_err());
    }
    let request = Request::new(Operation::ValidateSofa {
        path: vec![b'/'; MAX_PATH_BYTES],
    })
    .unwrap();
    let bytes = serde_json::to_vec(&request).unwrap();
    assert!(bytes.len() <= MAX_REQUEST_BYTES);
    Request::parse(&bytes).unwrap();
}

fn decode_lua_string(value: &str) -> Vec<u8> {
    assert!(value.starts_with('"') && value.ends_with('"'));
    value.as_bytes()[1..value.len() - 1]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| {
            assert_eq!(chunk[0], b'\\');
            std::str::from_utf8(&chunk[1..])
                .unwrap()
                .parse::<u8>()
                .unwrap()
        })
        .collect()
}

#[test]
fn lua_and_json_quotes_cannot_become_script_syntax() {
    for value in [
        "quote\"slash\\';error('injected')--",
        "\n\0\r\t",
        "1][=[2",
        "name-\u{00e9}-\u{1f5a5}",
    ] {
        let quoted = lua_string(value);
        assert_eq!(decode_lua_string(&quoted), value.as_bytes());
        assert_eq!(quoted.len(), value.len() * 4 + 2);
    }
    for volume in [0.0, 1.0, 0.25] {
        let script = render_lua(&level(volume, false)).unwrap();
        assert!(script.starts_with("local request="));
        assert!(script.contains(&format!("volume={volume:.17}")));
        assert!(!script.contains("os.execute('bad')"));
        assert!(script.contains(&lua_string(&u64::MAX.to_string())));
    }
    let script = render_lua(&default(Some(node()), None)).unwrap();
    let prefix = script.strip_suffix(CONTROL_SCRIPT).unwrap();
    let encoded = prefix
        .split("name_json=")
        .nth(1)
        .unwrap()
        .trim_end_matches("}\n");
    let json: Value = serde_json::from_slice(&decode_lua_string(encoded)).unwrap();
    assert_eq!(json["name"], node().node_name);
    assert!(!CONTROL_SCRIPT.contains("\"wpctl\""));
    assert!(render_lua(
        &Request::new(Operation::ValidateSofa {
            path: b"/tmp/a".to_vec()
        })
        .unwrap()
    )
    .is_err());
}

#[test]
fn replies_require_schema_exit_success_and_bounded_errors() {
    let confirmed = br#"{"schemaVersion":1,"outcome":{"status":"confirmed"}}"#;
    Response::parse(confirmed, true).unwrap();
    assert!(Response::parse(confirmed, false).is_err());
    for invalid in [
        br#"{"schemaVersion":2,"outcome":{"status":"confirmed"}}"#.as_slice(),
        br#"{"schemaVersion":1,"outcome":{"status":"confirmed","extra":1}}"#,
        br#"{"schemaVersion":1,"outcome":{"status":"error","code":"shell","message":"bad"}}"#,
        b"{}",
    ] {
        assert!(Response::parse(invalid, true).is_err());
    }
    let error = Response::from_result(Err(
        AppError::new(ErrorCode::NotConfirmed, "No mutation").detail("identity changed")
    ));
    assert_eq!(
        Response::parse(&serde_json::to_vec(&error).unwrap(), false)
            .unwrap_err()
            .code,
        ErrorCode::NotConfirmed
    );
    assert!(Response::parse(&vec![b' '; MAX_RESPONSE_BYTES + 1], true).is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn helper_path_uses_sibling_and_concrete_debug_deps_fallback() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    let debug = root.path().join("debug");
    let deps = debug.join("deps");
    std::fs::create_dir_all(&deps).unwrap();
    let test_binary = deps.join("test-hash");
    std::fs::write(&test_binary, b"test").unwrap();
    std::fs::set_permissions(&test_binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let helper = debug.join(WORKER_BINARY);
    assert_eq!(
        worker_path(&test_binary).unwrap_err().code,
        ErrorCode::ProviderMissing
    );
    std::fs::write(&helper, b"helper").unwrap();
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
    #[cfg(debug_assertions)]
    assert_eq!(worker_path(&test_binary).unwrap(), helper);
    let sibling = deps.join(WORKER_BINARY);
    symlink(&test_binary, &sibling).unwrap();
    assert_eq!(
        worker_path(&test_binary).unwrap_err().code,
        ErrorCode::Validation
    );
    assert_eq!(
        worker_path(&helper).unwrap_err().code,
        ErrorCode::Validation
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn output_limits_stop_at_the_budget() {
    assert_eq!(read_bounded(&b"1234"[..], 4).await.unwrap(), b"1234");
    assert_eq!(
        read_bounded(&b"12345"[..], 4).await.unwrap_err().code,
        ErrorCode::InvalidData
    );
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "fixture used only by the paired process tests"]
fn native_child_fixture() {
    use std::io::{Read, Write};
    let Ok(mode) = std::env::var("PH_WORKER_FIXTURE") else {
        return;
    };
    if mode == "budgets" {
        for (resource, maximum) in [
            (libc::RLIMIT_AS, ADDRESS_SPACE_BYTES),
            (libc::RLIMIT_CPU, CPU_SECONDS),
            (libc::RLIMIT_CORE, 0),
        ] {
            let mut limit = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            assert_eq!(unsafe { libc::getrlimit(resource, &mut limit) }, 0);
            assert_eq!(limit.rlim_cur, maximum);
            assert_eq!(limit.rlim_max, maximum);
        }
        let allocation = unsafe { libc::malloc(ADDRESS_SPACE_BYTES as usize) };
        assert!(allocation.is_null());
        assert_eq!(
            unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) },
            1
        );
        println!("BUDGETS_CONFIRMED");
    } else if mode == "input" {
        let mut bytes = Vec::new();
        std::io::stdin().read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"bounded-input");
        println!("EOF_CONFIRMED");
    } else if mode == "output" {
        std::io::stdout()
            .write_all(&vec![b'x'; MAX_RESPONSE_BYTES * 2])
            .unwrap();
        std::thread::sleep(Duration::from_secs(30));
    } else if mode == "hang" {
        unsafe {
            libc::signal(libc::SIGTERM, libc::SIG_IGN);
        }
        std::fs::write(
            std::env::var_os("PH_WORKER_PID").unwrap(),
            std::process::id().to_string(),
        )
        .unwrap();
        // Represents a native call that cannot be cancelled by aborting a Rust task.
        std::thread::sleep(Duration::from_secs(30));
    } else {
        panic!("unknown fixture");
    }
}

#[cfg(target_os = "linux")]
fn fixture_command(mode: &str) -> tokio::process::Command {
    let mut command = limited_command(&std::env::current_exe().unwrap(), None).unwrap();
    command
        .args([
            "--ignored",
            "--exact",
            "audio_worker::tests::native_child_fixture",
            "--nocapture",
        ])
        .env("PH_WORKER_FIXTURE", mode);
    command
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn child_budgets_and_protocol_eof_are_enforced_before_work() {
    let budgets = capture(fixture_command("budgets"), None, Duration::from_secs(3))
        .await
        .unwrap();
    assert!(budgets.success, "{}", budgets.stderr);
    assert!(String::from_utf8_lossy(&budgets.stdout).contains("BUDGETS_CONFIRMED"));
    let input = capture(
        fixture_command("input"),
        Some(b"bounded-input"),
        Duration::from_secs(3),
    )
    .await
    .unwrap();
    assert!(input.success);
    assert!(String::from_utf8_lossy(&input.stdout).contains("EOF_CONFIRMED"));
    assert_eq!(
        capture(fixture_command("output"), None, Duration::from_secs(3))
            .await
            .unwrap_err()
            .code,
        ErrorCode::InvalidData
    );
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "current_thread")]
async fn deadline_kills_and_reaps_an_uncancellable_native_process() {
    let directory = tempfile::tempdir().unwrap();
    let pidfile = directory.path().join("pid");
    let mut command = fixture_command("hang");
    command.env("PH_WORKER_PID", &pidfile);
    let started = std::time::Instant::now();
    let error = capture(command, None, Duration::from_millis(200))
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Timeout);
    assert!(started.elapsed() < Duration::from_secs(3));
    let pid: libc::pid_t = std::fs::read_to_string(pidfile).unwrap().parse().unwrap();
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn cancelling_parent_future_kills_and_reaps_the_child() {
    let directory = tempfile::tempdir().unwrap();
    let pidfile = directory.path().join("pid");
    let mut command = fixture_command("hang");
    command.env("PH_WORKER_PID", &pidfile);
    let task = tokio::spawn(async move { capture(command, None, Duration::from_secs(30)).await });
    let pid = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(pid) = std::fs::read_to_string(&pidfile) {
                break pid.parse::<libc::pid_t>().unwrap();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(3), async {
        while unsafe { libc::kill(pid, 0) } == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}

#[cfg(target_os = "linux")]
fn resolve_lua_path() -> PathBuf {
    for candidate in &[
        "/usr/bin/lua5.4",
        "/usr/bin/lua",
        "/usr/sbin/lua",
        "/bin/lua5.4",
        "/bin/lua",
    ] {
        let p = Path::new(candidate);
        if p.is_file() {
            return p.to_path_buf();
        }
    }
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            for name in &["lua5.4", "lua"] {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
    }
    PathBuf::from("/usr/bin/lua5.4")
}

#[cfg(target_os = "linux")]
async fn run_lua_scenario(request: &Request, scenario: &str, control: &str) -> Captured {
    let harness = include_str!("../tests/fixtures/audio-worker-proxy.lua");
    let script = render_lua(request).unwrap();
    let prefix = script.strip_suffix(CONTROL_SCRIPT).unwrap();
    let program = format!(
        "local scenario={}\n{prefix}{harness}\n{control}\nworker_test_finish()\n",
        lua_string(scenario)
    );
    let lua_path = resolve_lua_path();
    let mut command = limited_command(&lua_path, None).unwrap();
    // Explicit stdin script mode propagates errors to the exit code. Bare lua
    // otherwise treats stdin like an interactive session and can exit zero.
    command.arg("-");
    capture(command, Some(program.as_bytes()), Duration::from_secs(3))
        .await
        .unwrap()
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "ACP deferred-gain regression in isolated Lua; never opens audio devices"]
async fn lua_full_level_phase_safety() {
    for (scenario, volume, muted, success) in [
        ("acp-lower-before-unmute", 0.1, false, true),
        ("acp-mute-before-raise", 0.9, true, true),
        ("acp-volume-ack-fail", 0.1, false, false),
        ("acp-mute-ack-fail", 0.9, true, false),
        ("acp-node-swap", 0.1, false, false),
        ("acp-device-swap", 0.1, false, false),
        ("acp-cookie-swap", 0.1, false, false),
        ("acp-node-swap-muted", 0.9, true, false),
        ("acp-device-swap-muted", 0.9, true, false),
        ("acp-cookie-swap-muted", 0.9, true, false),
        ("acp-node-swap-sync", 0.1, false, false),
        ("acp-device-swap-sync", 0.1, false, false),
        ("acp-volume-drift", 0.1, false, false),
        ("acp-mute-drift", 0.9, true, false),
        ("acp-volume-drift-final", 0.1, false, false),
    ] {
        let output = run_lua_scenario(&level(volume, muted), scenario, CONTROL_SCRIPT).await;
        assert!(output.success, "{scenario}: {}", output.stderr);
        let result = Response::parse(&output.stdout, true);
        assert_eq!(result.is_ok(), success, "{scenario}: {result:?}");
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires lua5.4 in the isolated worker test builder; never uses real audio"]
async fn lua_proxy_identity_and_readback_contracts() {
    for scenario in [
        "normal",
        "scalar-volume",
        "serial-mismatch",
        "volume-only",
        "mute-only",
        "no-yield",
        "replace-before-write",
        "replace-after-write",
        "cache-side-effect",
        "device-route",
        "device-replace",
        "wrong-level",
        "wrong-mute",
        "channel-mismatch",
        "cookie-mismatch",
    ] {
        let request = match scenario {
            "volume-only" => Request::new(Operation::SetLevel {
                node: node(),
                core_cookie: u32::MAX.to_string(),
                volume: Some(0.25),
                muted: None,
            })
            .unwrap(),
            "mute-only" => Request::new(Operation::SetLevel {
                node: node(),
                core_cookie: u32::MAX.to_string(),
                volume: None,
                muted: Some(true),
            })
            .unwrap(),
            _ => level(0.25, true),
        };
        let output = run_lua_scenario(&request, scenario, CONTROL_SCRIPT).await;
        assert!(output.success, "{scenario}: {}", output.stderr);
        let result = Response::parse(&output.stdout, true);
        assert_eq!(
            result.is_ok(),
            matches!(
                scenario,
                "normal"
                    | "scalar-volume"
                    | "volume-only"
                    | "mute-only"
                    | "no-yield"
                    | "device-route"
            ),
            "{scenario}: {result:?}"
        );
    }
    for scenario in [
        "default",
        "baseline-change",
        "clear",
        "malformed-default",
        "structured-default",
        "duplicate-node",
        "metadata-replace",
    ] {
        let request = if scenario == "clear" {
            default(None, Some("baseline"))
        } else {
            default(Some(node()), Some("baseline"))
        };
        let output = run_lua_scenario(&request, scenario, CONTROL_SCRIPT).await;
        assert!(output.success, "{scenario}: {}", output.stderr);
        let result = Response::parse(&output.stdout, true);
        assert_eq!(
            result.is_ok(),
            matches!(scenario, "default" | "clear"),
            "{scenario}: {result:?}"
        );
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "Lua mutation proof in the isolated worker test builder, no real devices"]
async fn lua_guard_mutations_are_detected() {
    for (scenario, needle, replacement) in [
        (
            "cookie-mismatch",
            "tostring(info.cookie) == request.cookie",
            "true",
        ),
        (
            "serial-mismatch",
            "properties[\"object.serial\"] ~= request.node.serial or",
            "false or",
        ),
        ("baseline-change", "previous ~= request.expected", "false"),
        (
            "acp-lower-before-unmute",
            "volume = phase.volume, mute = phase.mute",
            "volume = request.volume, mute = request.muted",
        ),
    ] {
        assert_eq!(CONTROL_SCRIPT.matches(needle).count(), 1);
        let mutated = CONTROL_SCRIPT.replace(needle, replacement);
        let request = match scenario {
            "baseline-change" => default(Some(node()), Some("baseline")),
            "acp-lower-before-unmute" => level(0.1, false),
            _ => level(0.25, true),
        };
        let output = run_lua_scenario(&request, scenario, &mutated).await;
        assert!(
            !output.success,
            "{scenario}: mutation must fail the negative regression; stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            output.stderr
        );
        assert!(
            output.stderr.contains("stale proxy must not mutate")
                || output
                    .stderr
                    .contains("external/invalid default must be preserved")
                || output
                    .stderr
                    .contains("ACP unmute before confirmed gain reduction"),
            "{scenario}: failure must be the semantic regression, not a syntax error: {}",
            output.stderr
        );
    }
}
