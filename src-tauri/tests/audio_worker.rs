#![cfg(target_os = "linux")]

use peripheral_hub_lib::audio_worker;
use peripheral_hub_lib::models::{AudioNode, ErrorCode};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::{Child, Command};

const REMOTE: &str = "peripheral-hub-worker-test";
const SELECTED: &str = "worker.\"\\selected';bad()--";

struct Server {
    child: Child,
    directory: tempfile::TempDir,
}

impl Server {
    async fn start() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let config = directory.path().join("pipewire.conf");
        std::fs::write(
            &config,
            include_bytes!("fixtures/audio-worker-pipewire.conf"),
        )
        .unwrap();
        let mut command = Command::new("/usr/bin/pipewire");
        command
            .args(["-c", "pipewire.conf"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        isolated_environment(&mut command, directory.path());
        command
            .env("PIPEWIRE_CONFIG_DIR", directory.path())
            .env("PIPEWIRE_CONFIG_PREFIX", "")
            .env("PIPEWIRE_CONFIG_NAME", "pipewire.conf");
        let child = command
            .spawn()
            .expect("pipewire must be installed in the isolated test builder");
        let mut server = Self { child, directory };
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(status) = server.child.try_wait().unwrap() {
                    panic!("isolated PipeWire exited before readiness: {status}");
                }
                if server.directory.path().join(REMOTE).exists() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("isolated socket was not created");
        let mut sink_cmd = Command::new("/usr/bin/pw-metadata");
        sink_cmd.args(["-n", "default", "0", "default.configured.audio.sink", r#"{"name":"worker.baseline"}"#, "Spa:String:JSON"]);
        isolated_environment(&mut sink_cmd, server.directory.path());
        let _ = sink_cmd.status().await;

        let mut source_cmd = Command::new("/usr/bin/pw-metadata");
        source_cmd.args(["-n", "default", "0", "default.configured.audio.source", r#"{"name":"source-kept"}"#, "Spa:String:JSON"]);
        isolated_environment(&mut source_cmd, server.directory.path());
        let _ = source_cmd.status().await;
        server
    }

    async fn dump(&self) -> Value {
        let mut command = Command::new("/usr/bin/pw-dump");
        isolated_environment(&mut command, self.directory.path());
        command.stdin(Stdio::null()).kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(4), command.output())
            .await
            .unwrap()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    async fn recycle(&self) -> Value {
        let script = self.directory.path().join("recycle.lua");
        std::fs::write(&script, include_bytes!("fixtures/audio-worker-recycle.lua")).unwrap();
        let mut command = Command::new("/usr/bin/wpexec");
        command.arg(&script).stdin(Stdio::null()).kill_on_drop(true);
        isolated_environment(&mut command, self.directory.path());
        let output = tokio::time::timeout(Duration::from_secs(6), command.output())
            .await
            .unwrap()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(value.get("error").is_none(), "{value}");
        value
    }

    fn select_for_worker(&self) {
        // Only this ignored integration-test process changes its environment.
        for key in ["XDG_RUNTIME_DIR", "PIPEWIRE_RUNTIME_DIR", "PW_RUNTIME_DIR"] {
            std::env::set_var(key, self.directory.path());
        }
        std::env::set_var("PIPEWIRE_REMOTE", REMOTE);
        std::env::set_var("XDG_CONFIG_HOME", self.directory.path());
        std::env::set_var("XDG_DATA_HOME", self.directory.path());
    }

    async fn stop(&mut self) {
        let _ = self.child.start_kill();
        tokio::time::timeout(Duration::from_secs(3), self.child.wait())
            .await
            .unwrap()
            .unwrap();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

fn isolated_environment(command: &mut Command, directory: &Path) {
    for key in [
        "XDG_RUNTIME_DIR",
        "PIPEWIRE_RUNTIME_DIR",
        "PW_RUNTIME_DIR",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
    ] {
        command.env(key, directory);
    }
    command.env("PIPEWIRE_REMOTE", REMOTE).env("LC_ALL", "C");
    for key in [
        "PIPEWIRE_CONFIG_DIR",
        "PIPEWIRE_CONFIG_NAME",
        "PIPEWIRE_CONFIG_PREFIX",
        "PIPEWIRE_PROPS",
        "WIREPLUMBER_CONFIG_DIR",
        "WIREPLUMBER_DATA_DIR",
        "WIREPLUMBER_MODULE_DIR",
    ] {
        command.env_remove(key);
    }
}

fn text(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Number(value) => value.to_string(),
        _ => panic!("not scalar: {value}"),
    }
}

fn cookie(dump: &Value) -> String {
    let core = dump
        .as_array()
        .unwrap()
        .iter()
        .find(|object| object["type"] == "PipeWire:Interface:Core")
        .unwrap();
    text(&core["info"]["cookie"])
}

fn node(dump: &Value, name: &str) -> AudioNode {
    let object = dump
        .as_array()
        .unwrap()
        .iter()
        .find(|object| object["info"]["props"]["node.name"] == name)
        .unwrap();
    let properties = &object["info"]["props"];
    AudioNode {
        id: object["id"].as_u64().unwrap() as u32,
        object_serial: Some(text(&properties["object.serial"]).parse().unwrap()),
        node_name: name.into(),
        description: name.into(),
        media_class: text(&properties["media.class"]),
        channels: vec!["FL".into(), "FR".into()],
        volume: None,
        muted: None,
        is_virtual: true,
        device_serial: None,
        device_bus_path: None,
        vendor_id: None,
        product_id: None,
        transport: None,
    }
}

fn metadata(dump: &Value, key: &str) -> Option<String> {
    let object = dump
        .as_array()
        .unwrap()
        .iter()
        .find(|object| {
            object["type"] == "PipeWire:Interface:Metadata"
                && object["props"]["metadata.name"] == "default"
        })
        .unwrap();
    object["metadata"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["key"] == key)
        .map(|entry| {
            let value = &entry["value"];
            let value = if let Some(value) = value.as_str() {
                serde_json::from_str(value).unwrap()
            } else {
                value.clone()
            };
            value["name"].as_str().unwrap().to_owned()
        })
}

fn raw_level(dump: &Value, name: &str) -> (f64, bool) {
    let object = dump
        .as_array()
        .unwrap()
        .iter()
        .find(|object| object["info"]["props"]["node.name"] == name)
        .unwrap();
    let props = object["info"]["params"]["Props"]
        .as_array()
        .unwrap()
        .iter()
        .find(|props| props.get("channelVolumes").is_some())
        .unwrap();
    (
        props["channelVolumes"][0].as_f64().unwrap(),
        props["mute"].as_bool().unwrap(),
    )
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit isolated OCI gate only; starts its own null-sink server, never host audio"]
async fn isolated_pipewire_proxy_recycling_cubic_default_and_sofa() {
    assert_eq!(
        std::env::var("PERIPHERAL_HUB_TEST_ISOLATED_PIPEWIRE").as_deref(),
        Ok("1")
    );
    assert!(
        Path::new("/.dockerenv").exists() || Path::new("/run/.containerenv").exists(),
        "OCI isolation is mandatory"
    );
    for device in ["/dev/snd", "/dev/input", "/dev/uinput"] {
        assert!(
            !Path::new(device).exists(),
            "host devices must not be exposed to this gate: {device}"
        );
    }
    let mut server = Server::start().await;
    server.select_for_worker();
    let initial = server.dump().await;
    let cookie = cookie(&initial);
    let selected = node(&initial, SELECTED);
    assert_eq!(
        metadata(&initial, "default.configured.audio.sink").as_deref(),
        Some("worker.baseline")
    );

    audio_worker::set_level(&selected, &cookie, 0.25, true)
        .await
        .unwrap();
    let dump = server.dump().await;
    let (linear, muted) = raw_level(&dump, SELECTED);
    assert!(
        (linear - 0.25_f64.powi(3)).abs() < 0.00001,
        "wpctl cubic semantics must be preserved: {linear}"
    );
    assert!(muted);
    assert_eq!(raw_level(&dump, "worker.other"), (1.0, false));

    let mut stale_snapshot = selected.clone();
    stale_snapshot.muted = Some(false);
    audio_worker::set_volume(&stale_snapshot, &cookie, 0.3)
        .await
        .unwrap();
    let (actual_volume, actual_mute) = raw_level(&server.dump().await, SELECTED);
    assert!((actual_volume - 0.3_f64.powi(3)).abs() < 0.00001);
    assert!(
        actual_mute,
        "volume-only must not replay old mute from the caller snapshot"
    );
    stale_snapshot.volume = Some(0.9);
    audio_worker::set_mute(&stale_snapshot, &cookie, false)
        .await
        .unwrap();
    assert_eq!(
        raw_level(&server.dump().await, SELECTED),
        (actual_volume, false)
    );
    audio_worker::set_level(&selected, &cookie, 0.25, true)
        .await
        .unwrap();

    audio_worker::set_level(&selected, &cookie, 1.0, true)
        .await
        .unwrap();
    assert_eq!(raw_level(&server.dump().await, SELECTED), (1.0, true));
    audio_worker::set_level(&selected, &cookie, 0.1, false)
        .await
        .unwrap();
    let (reduced, unmuted) = raw_level(&server.dump().await, SELECTED);
    assert!((reduced - 0.1_f64.powi(3)).abs() < 0.00001);
    assert!(!unmuted);
    audio_worker::set_level(&selected, &cookie, 0.9, true)
        .await
        .unwrap();
    let (raised, remuted) = raw_level(&server.dump().await, SELECTED);
    assert!((raised - 0.9_f64.powi(3)).abs() < 0.00001);
    assert!(remuted);
    audio_worker::set_level(&selected, &cookie, 0.25, true)
        .await
        .unwrap();

    audio_worker::set_default(Some(&selected), &cookie, Some("worker.baseline"))
        .await
        .unwrap();
    let dump = server.dump().await;
    assert_eq!(
        metadata(&dump, "default.configured.audio.sink").as_deref(),
        Some(SELECTED)
    );
    let other = node(&dump, "worker.other");
    assert_eq!(
        audio_worker::set_default(Some(&other), &cookie, Some("worker.baseline"))
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotConfirmed
    );
    assert_eq!(
        metadata(&server.dump().await, "default.configured.audio.sink").as_deref(),
        Some(SELECTED)
    );
    audio_worker::set_default(None, &cookie, Some(SELECTED))
        .await
        .unwrap();
    let dump = server.dump().await;
    assert_eq!(metadata(&dump, "default.configured.audio.sink"), None);
    assert_eq!(
        metadata(&dump, "default.configured.audio.source").as_deref(),
        Some("source-kept")
    );

    let recycled = server.recycle().await;
    assert_eq!(recycled["before"]["name"], recycled["after"]["name"]);
    assert_eq!(recycled["before"]["class"], recycled["after"]["class"]);
    if recycled["before"]["id"] == recycled["after"]["id"] {
        assert_ne!(recycled["before"]["serial"], recycled["after"]["serial"]);
    }
    let mut stale = selected.clone();
    stale.id = recycled["before"]["id"].as_u64().unwrap() as u32;
    stale.object_serial = Some(text(&recycled["before"]["serial"]).parse().unwrap());
    stale.node_name = text(&recycled["before"]["name"]);
    assert!(audio_worker::set_level(&stale, &cookie, 0.1, true)
        .await
        .is_err());
    assert!(audio_worker::set_default(Some(&stale), &cookie, None)
        .await
        .is_err());
    let dump = server.dump().await;
    assert_eq!(raw_level(&dump, "worker.recycled.old"), (1.0, false));
    assert_eq!(metadata(&dump, "default.configured.audio.sink"), None);

    let mut wrong_serial = selected.clone();
    wrong_serial.object_serial = Some(selected.object_serial.unwrap() + 1);
    assert!(audio_worker::set_level(&wrong_serial, &cookie, 0.9, false)
        .await
        .is_err());
    let wrong_cookie = if cookie == "1" { "2" } else { "1" };
    assert!(audio_worker::set_level(&selected, wrong_cookie, 0.9, false)
        .await
        .is_err());
    assert_eq!(raw_level(&server.dump().await, SELECTED), (linear, muted));

    for candidate in &[
        "/usr/share/libmysofa/default.sofa",
        "/usr/share/libmysofa/MIT_KEMAR_normal_pinna.sofa",
    ] {
        let path = Path::new(candidate);
        if path.is_file() {
            audio_worker::validate_sofa(path)
                .await
                .unwrap();
            break;
        }
    }
    let invalid: PathBuf = server.directory.path().join("invalid.sofa");
    std::fs::write(&invalid, vec![0; 128]).unwrap();
    assert!(audio_worker::validate_sofa(&invalid).await.is_err());
    server.stop().await;
}
