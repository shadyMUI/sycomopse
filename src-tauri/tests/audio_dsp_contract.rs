#![cfg(target_os = "linux")]
// Native software contract only. No hardware/HID/native-install certificate.
// The ignored probe requires check_dsp.py's own DBus/PipeWire/WirePlumber server.
// Fixture device.api=alsa is admission metadata only, on a real software NULL factory.

use peripheral_hub_lib::audio::AudioController;
use peripheral_hub_lib::models::{AudioProfile, AudioSnapshot, SurroundState};
use peripheral_hub_lib::profiles::AppPaths;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::{Child, Command};
use uuid::Uuid;

const TARGET: &str = "dsp-contract.synthetic-stereo";
const OTHER: &str = "dsp-contract.synthetic-other";
const CHANNELS: [&str; 8] = ["FL", "FR", "FC", "LFE", "RL", "RR", "SL", "SR"];

fn sandbox_error(
    oci: bool,
    uid: u32,
    devices: [bool; 3],
    caps: u64,
    nnp: bool,
) -> Option<&'static str> {
    if !oci {
        Some("Real OCI marker required; environment flags cannot authorize native audio")
    } else if uid != 1000 || caps != 0 || !nnp {
        Some("UID1000/cap-drop ALL/no-new-privileges are mandatory")
    } else if devices.into_iter().any(|present| present) {
        Some("No input/uinput/sound device may be exposed")
    } else {
        None
    }
}

fn status_field(status: &str, key: &str) -> String {
    status
        .lines()
        .find_map(|line| line.strip_prefix(key))
        .unwrap()
        .trim()
        .to_owned()
}

fn require_sandbox() {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let error = sandbox_error(
        Path::new("/.dockerenv").is_file() || Path::new("/run/.containerenv").is_file(),
        unsafe { libc::geteuid() },
        ["/dev/input", "/dev/uinput", "/dev/snd"].map(|path| Path::new(path).exists()),
        u64::from_str_radix(&status_field(&status, "CapEff:"), 16).unwrap(),
        status_field(&status, "NoNewPrivs:") == "1",
    );
    assert!(error.is_none(), "Native DSP sandbox rejected: {error:?}");
    for path in [
        "/run/host",
        "/run/dbus/system_bus_socket",
        "/var/run/docker.sock",
        "/run/user/1000",
    ] {
        assert!(
            !Path::new(path).exists(),
            "Host/session mount forbidden: {path}"
        );
    }
}

fn private_runtime_path(path: &Path) -> bool {
    path.is_absolute()
        && path.starts_with("/work")
        && path != Path::new("/work")
        && !path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
}

#[test]
fn isolation_header_requires_real_oci_marker_and_empty_devices() {
    assert!(sandbox_error(true, 1000, [false; 3], 0, true).is_none());
    assert!(sandbox_error(false, 1000, [false; 3], 0, true).is_some());
    for index in 0..3 {
        let mut devices = [false; 3];
        devices[index] = true;
        assert!(sandbox_error(true, 1000, devices, 0, true).is_some());
    }
}

#[test]
fn isolation_header_rejects_privileges_and_unstaged_host_paths() {
    for (uid, caps, nnp) in [(0, 0, true), (1000, 1, true), (1000, 0, false)] {
        assert!(sandbox_error(true, uid, [false; 3], caps, nnp).is_some());
    }
    for path in ["/work", "/run/user/1000", "relative", "/work/a/../b"] {
        assert!(!private_runtime_path(Path::new(path)), "{path}");
    }
    assert!(private_runtime_path(Path::new("/work/dsp/private-runtime")));
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StagedServer {
    schema_version: u32,
    synthetic: bool,
    runtime: PathBuf,
    remote: String,
    core_cookie: u64,
    runner_pid: u32,
    dbus_pid: u32,
    pipewire_pid: u32,
    wireplumber_pid: u32,
}

fn proc_parent(pid: u32) -> u32 {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    assert_eq!(
        status_field(&status, "Uid:").split_whitespace().next(),
        Some("1000")
    );
    assert_eq!(status_field(&status, "CapEff:"), "0000000000000000");
    assert_eq!(status_field(&status, "NoNewPrivs:"), "1");
    status_field(&status, "PPid:").parse().unwrap()
}

impl StagedServer {
    fn read() -> Self {
        require_sandbox();
        let runtime = PathBuf::from(std::env::var_os("PH_DSP_CONTRACT_RUNTIME").expect(
            "Native probe is ignored normally: explicitly stage check_dsp.py's private OCI server",
        ));
        assert!(private_runtime_path(&runtime));
        let meta = std::fs::symlink_metadata(&runtime).unwrap();
        assert!(meta.is_dir() && !meta.file_type().is_symlink());
        assert_eq!(meta.uid(), 1000);
        assert_eq!(meta.permissions().mode() & 0o777, 0o700);
        let stage_path = runtime.join("dsp-contract.json");
        let meta = std::fs::symlink_metadata(&stage_path).unwrap();
        assert!(meta.is_file() && meta.len() <= 4096 && meta.nlink() == 1);
        assert_eq!(meta.uid(), 1000);
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        let stage: Self = serde_json::from_slice(&std::fs::read(stage_path).unwrap()).unwrap();
        assert_eq!(stage.schema_version, 1);
        assert!(
            stage.synthetic,
            "Fixture must not masquerade as a physical device"
        );
        assert_eq!(stage.runtime, runtime);
        assert!(stage.remote.starts_with("peripheral-hub-dsp-"));
        for key in ["XDG_RUNTIME_DIR", "PIPEWIRE_RUNTIME_DIR", "PW_RUNTIME_DIR"] {
            assert_eq!(PathBuf::from(std::env::var_os(key).unwrap()), runtime);
        }
        assert_eq!(std::env::var("PIPEWIRE_REMOTE").unwrap(), stage.remote);
        for key in ["DBUS_SESSION_BUS_ADDRESS", "DBUS_SYSTEM_BUS_ADDRESS"] {
            assert_eq!(
                std::env::var(key).unwrap(),
                format!("unix:path={}/bus", runtime.display())
            );
        }
        for socket in [runtime.join(&stage.remote), runtime.join("bus")] {
            let meta = std::fs::symlink_metadata(&socket).unwrap();
            assert!(meta.file_type().is_socket());
            assert_eq!(meta.uid(), 1000);
        }
        for (pid, executable) in [
            (stage.dbus_pid, "dbus-daemon"),
            (stage.pipewire_pid, "pipewire"),
            (stage.wireplumber_pid, "wireplumber"),
        ] {
            assert_eq!(
                proc_parent(pid),
                stage.runner_pid,
                "Server is not owned by the staging runner"
            );
            let exe = std::fs::read_link(format!("/proc/{pid}/exe")).unwrap();
            assert_eq!(exe.file_name().unwrap(), executable);
        }
        assert_eq!(
            proc_parent(stage.runner_pid),
            0,
            "Runner must be PID1 in its private OCI PID namespace"
        );
        stage
    }

    async fn dump(&self) -> Value {
        let mut command = Command::new("/usr/bin/pw-dump");
        command.args(["-N"]).stdin(Stdio::null()).kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(4), command.output())
            .await
            .unwrap()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let dump: Value = serde_json::from_slice(&output.stdout).unwrap();
        let core = dump
            .as_array()
            .unwrap()
            .iter()
            .find(|object| object["type"] == "PipeWire:Interface:Core")
            .unwrap();
        assert_eq!(
            number(&core["info"]["cookie"]),
            self.core_cookie,
            "Probe connected to another/restarted server"
        );
        dump
    }
}

fn number(value: &Value) -> u64 {
    value
        .as_u64()
        .unwrap_or_else(|| value.as_str().unwrap().parse().unwrap())
}

fn node<'a>(dump: &'a Value, name: &str) -> &'a Value {
    let nodes: Vec<_> = dump
        .as_array()
        .unwrap()
        .iter()
        .filter(|object| {
            object["type"] == "PipeWire:Interface:Node"
                && object["info"]["props"]["node.name"] == name
        })
        .collect();
    assert_eq!(
        nodes.len(),
        1,
        "Expected exactly one real node {name}: {dump}"
    );
    nodes[0]
}

fn metadata(dump: &Value, key: &str) -> Option<String> {
    let stores: Vec<_> = dump
        .as_array()
        .unwrap()
        .iter()
        .filter(|object| {
            object["type"] == "PipeWire:Interface:Metadata"
                && object["props"]["metadata.name"] == "default"
        })
        .collect();
    assert_eq!(stores.len(), 1);
    stores[0]["metadata"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["key"] == key)
        .map(|entry| {
            let value = &entry["value"];
            let parsed = value
                .as_str()
                .map(|value| serde_json::from_str::<Value>(value).unwrap());
            parsed.as_ref().unwrap_or(value)["name"]
                .as_str()
                .unwrap()
                .to_owned()
        })
}

fn assert_default(snapshot: &AudioSnapshot, dump: &Value, name: &str) {
    assert_eq!(
        snapshot.default_output.as_deref(),
        Some(name),
        "{snapshot:?}"
    );
    assert_eq!(
        snapshot.configured_default_output.as_deref(),
        Some(name),
        "{snapshot:?}"
    );
    assert_eq!(metadata(dump, "default.audio.sink").as_deref(), Some(name));
    assert_eq!(
        metadata(dump, "default.configured.audio.sink").as_deref(),
        Some(name)
    );
}

fn assert_level(snapshot: &AudioSnapshot, dump: &Value, name: &str, volume: f64, muted: bool) {
    let observed = snapshot
        .outputs
        .iter()
        .find(|node| node.node_name == name)
        .unwrap();
    assert!(
        (observed.volume.unwrap() - volume).abs() < 0.002,
        "{observed:?}"
    );
    assert_eq!(observed.muted, Some(muted));
    let params = node(dump, name)["info"]["params"]["Props"]
        .as_array()
        .unwrap();
    let props = params
        .iter()
        .find(|props| props.get("channelVolumes").is_some())
        .unwrap();
    for level in props["channelVolumes"].as_array().unwrap() {
        assert!(
            (level.as_f64().unwrap() - volume.powi(3)).abs() < 0.00001,
            "Cubic level readback: {props}"
        );
    }
    assert_eq!(props["mute"].as_bool(), Some(muted));
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Journal {
    schema_version: u32,
    session_id: String,
    previous_configured_default: Option<String>,
    previous_effective_default: Option<String>,
    selected_physical_target: String,
    target_serial: u64,
    virtual_node_name: String,
    playback_node_name: String,
    app_process_id: u32,
    filter_process_id: Option<u32>,
    virtual_object_serial: Option<u64>,
    playback_object_serial: Option<u64>,
    core_cookie: Option<u64>,
}

fn journal(paths: &AppPaths) -> Journal {
    let path = paths.state_dir.join("audio-restore.json");
    let meta = std::fs::symlink_metadata(&path).unwrap();
    assert!(meta.is_file() && meta.len() <= 16 * 1024 && meta.nlink() == 1);
    assert_eq!(meta.uid(), 1000);
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    let record: Journal = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(record.schema_version, 1);
    assert_eq!(
        Uuid::parse_str(&record.session_id).unwrap().to_string(),
        record.session_id
    );
    assert_eq!(record.app_process_id, std::process::id());
    assert_eq!(record.selected_physical_target, TARGET);
    assert_eq!(record.previous_configured_default.as_deref(), Some(TARGET));
    assert_eq!(record.previous_effective_default.as_deref(), Some(TARGET));
    record
}

fn ports<'a>(dump: &'a Value, node_id: u64, direction: &str) -> Vec<&'a Value> {
    let info_direction = match direction {
        "in" => "input",
        "out" => "output",
        _ => panic!("Unknown test port direction"),
    };
    dump.as_array()
        .unwrap()
        .iter()
        .filter(|object| {
            object["type"] == "PipeWire:Interface:Port"
                && number(&object["info"]["props"]["node.id"]) == node_id
                && object["info"]["direction"] == info_direction
                && object["info"]["props"]["port.direction"] == direction
                && object["info"]["props"]["port.monitor"] != true
        })
        .collect()
}

fn assert_channels(actual: Vec<&Value>, expected: &[&str]) {
    let channels: BTreeSet<_> = actual
        .iter()
        .map(|port| port["info"]["props"]["audio.channel"].as_str().unwrap())
        .collect();
    assert_eq!(actual.len(), expected.len());
    assert_eq!(channels, expected.iter().copied().collect());
}

fn assert_graph(dump: &Value, record: &Journal, paths: &AppPaths, server: &StagedServer) {
    let target = node(dump, TARGET);
    assert_eq!(
        target["info"]["props"]["factory.name"],
        "support.null-audio-sink"
    );
    assert_eq!(target["info"]["props"]["device.api"], "alsa");
    assert!(
        target["info"]["props"]["device.id"].is_null(),
        "No hardware device may back this synthetic target"
    );
    assert_eq!(
        target["info"]["props"]["peripheral-hub.synthetic-test"],
        true
    );
    assert_eq!(
        target["info"]["props"]["node.virtual"], false,
        "ONLY the synthetic fixture declares this; production classifier is unchanged"
    );
    assert_eq!(
        number(&target["info"]["props"]["object.serial"]),
        record.target_serial
    );
    let input = node(dump, &record.virtual_node_name);
    let playback = node(dump, &record.playback_node_name);
    assert_eq!(input["info"]["props"]["media.class"], "Audio/Sink");
    assert_eq!(
        playback["info"]["props"]["media.class"],
        "Stream/Output/Audio"
    );
    assert_eq!(record.core_cookie, Some(server.core_cookie));
    for (object, expected_serial) in [
        (input, record.virtual_object_serial),
        (playback, record.playback_object_serial),
    ] {
        let props = &object["info"]["props"];
        assert_eq!(props["node.virtual"], true);
        assert_eq!(props["peripheral-hub.session"], record.session_id);
        assert_eq!(Some(number(&props["object.serial"])), expected_serial);
        let client_id = number(&props["client.id"]);
        let client = dump
            .as_array()
            .unwrap()
            .iter()
            .find(|object| number(&object["id"]) == client_id)
            .unwrap();
        assert_eq!(
            number(&client["info"]["props"]["application.process.id"]),
            u64::from(record.filter_process_id.unwrap())
        );
    }
    let input_id = number(&input["id"]);
    let playback_id = number(&playback["id"]);
    let target_id = number(&target["id"]);
    assert_channels(ports(dump, input_id, "in"), &CHANNELS);
    assert_channels(ports(dump, playback_id, "out"), &["FL", "FR"]);
    assert_channels(ports(dump, target_id, "in"), &["FL", "FR"]);
    let links: Vec<_> = dump
        .as_array()
        .unwrap()
        .iter()
        .filter(|object| {
            object["type"] == "PipeWire:Interface:Link"
                && number(&object["info"]["output-node-id"]) == playback_id
        })
        .collect();
    assert_eq!(
        links.len(),
        2,
        "DSP must really link exactly two stereo outputs"
    );
    let mut channels = BTreeSet::new();
    for link in links {
        let info = &link["info"];
        assert_eq!(number(&info["input-node-id"]), target_id);
        assert!(
            matches!(info["state"].as_str(), Some("active" | "paused")),
            "{link}"
        );
        let port = |key: &str| {
            dump.as_array()
                .unwrap()
                .iter()
                .find(|object| object["id"] == info[key])
                .unwrap()
        };
        let output = port("output-port-id");
        let input = port("input-port-id");
        let channel = output["info"]["props"]["audio.channel"].as_str().unwrap();
        assert_eq!(input["info"]["props"]["audio.channel"], channel);
        channels.insert(channel);
    }
    assert_eq!(channels, BTreeSet::from(["FL", "FR"]));
    let pid = record.filter_process_id.unwrap();
    assert_eq!(
        proc_parent(pid),
        std::process::id(),
        "Only the controller's actually owned child counts"
    );
    let maps = std::fs::read_to_string(format!("/proc/{pid}/maps")).unwrap();
    assert!(
        maps.contains("libmysofa.so"),
        "Actual DSP did not load libmysofa: {maps}"
    );
    assert!(maps.contains("libpipewire-module-filter-chain"));
    let config_path = paths
        .runtime_dir
        .join(&record.session_id)
        .join("spatializer.conf");
    let config: Value = serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
    let module = config["context.modules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|module| module["name"] == "libpipewire-module-filter-chain")
        .unwrap();
    let graph = &module["args"]["filter.graph"];
    let spatializers: Vec<_> = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["type"] == "sofa")
        .collect();
    assert_eq!(spatializers.len(), 8);
    for spatializer in spatializers {
        assert_eq!(spatializer["label"], "spatializer");
        assert_eq!(
            spatializer["config"]["filename"],
            "/usr/share/libmysofa/default.sofa"
        );
    }
    assert_eq!(graph["inputs"].as_array().unwrap().len(), 8);
    assert_eq!(graph["outputs"].as_array().unwrap().len(), 2);
    assert_eq!(graph["links"].as_array().unwrap().len(), 16);
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap();
    assert!(cmdline
        .split(|byte| *byte == 0)
        .any(|arg| arg == config_path.as_os_str().as_encoded_bytes()));
    println!("DSP PROOF: session={} ownedPid={pid} libmysofa=loaded inputs=8 stereoOutputs=2 stereoLinks=2", record.session_id);
}

async fn wait_removed(server: &StagedServer, record: &Journal) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let dump = server.dump().await;
            let visible = dump.as_array().unwrap().iter().any(|object| {
                let name = &object["info"]["props"]["node.name"];
                name == &record.virtual_node_name || name == &record.playback_node_name
            });
            if !Path::new(&format!("/proc/{}", record.filter_process_id.unwrap())).exists()
                && !visible
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("Owned DSP PID/nodes did not disappear in time");
}

fn profile(output: &str, surround: bool, volume: f64, mute: bool) -> AudioProfile {
    AudioProfile {
        schema_version: 1,
        id: Uuid::new_v4().to_string(),
        name: "Synthetic software DSP contract".into(),
        output_node_name: output.into(),
        hrtf: Some("system".into()),
        volume,
        mute,
        surround_requested: surround,
    }
}

struct Capture(Child);

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.0.start_kill();
    }
}

async fn pcm_probe(server: &StagedServer, record: &Journal) {
    let raw = server.runtime.join("eight-channel.f32");
    let capture = server.runtime.join("stereo-monitor.f32");
    let mut samples = Vec::with_capacity(48000 * 3 * 8 * 4);
    for frame in 0..48000 * 3 {
        for channel in 0..8 {
            let phase =
                frame as f32 * (220.0 + channel as f32 * 37.0) * std::f32::consts::TAU / 48000.0;
            samples.extend_from_slice(&(0.1 * phase.sin()).to_le_bytes());
        }
    }
    std::fs::write(&raw, samples).unwrap();
    let mut command = Command::new("/usr/bin/pw-cat");
    command
        .args([
            "--record",
            "--raw",
            "--format",
            "f32",
            "--rate",
            "48000",
            "--channels",
            "2",
            "--channel-map",
            "FL,FR",
            "--target",
            TARGET,
            "--properties",
            "{ stream.capture.sink = true node.name = dsp-contract.monitor }",
        ])
        .arg(&capture)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    let mut monitor = Capture(
        command
            .spawn()
            .expect("SDK pw-cat is required for the software PCM probe"),
    );
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(
        monitor.0.try_wait().unwrap().is_none(),
        "Monitor exited before playback"
    );
    let mut playback = Command::new("/usr/bin/pw-cat");
    playback
        .args([
            "--playback",
            "--raw",
            "--format",
            "f32",
            "--rate",
            "48000",
            "--channels",
            "8",
            "--channel-map",
            "FL,FR,FC,LFE,RL,RR,SL,SR",
            "--target",
            &record.virtual_node_name,
            "--properties",
            "{ node.name = dsp-contract.stream stream.dont-remix = true }",
        ])
        .arg(&raw)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    let mut stream = Capture(playback.spawn().unwrap());
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let dump = server.dump().await;
            let object = dump
                .as_array()
                .unwrap()
                .iter()
                .find(|object| object["info"]["props"]["node.name"] == "dsp-contract.stream");
            if let Some(object) = object {
                let stream_id = number(&object["id"]);
                let stream_ports = ports(&dump, stream_id, "out");
                let links: Vec<_> = dump
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|object| {
                        object["type"] == "PipeWire:Interface:Link"
                            && number(&object["info"]["output-node-id"]) == stream_id
                    })
                    .collect();
                if stream_ports.len() == 8 && links.len() == 8 {
                    assert_channels(stream_ports, &CHANNELS);
                    let sink_id = number(&node(&dump, &record.virtual_node_name)["id"]);
                    for link in links {
                        assert_eq!(number(&link["info"]["input-node-id"]), sink_id);
                        assert!(matches!(
                            link["info"]["state"].as_str(),
                            Some("active" | "paused")
                        ));
                    }
                    break;
                }
            }
            assert!(
                stream.0.try_wait().unwrap().is_none(),
                "8-channel stream exited before actual graph proof"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("Eight real stream ports/links did not connect to the DSP");
    let status = tokio::time::timeout(Duration::from_secs(8), stream.0.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(
        status.success(),
        "8-channel software playback failed: {status}"
    );
    tokio::time::sleep(Duration::from_millis(150)).await;
    monitor.0.start_kill().unwrap();
    tokio::time::timeout(Duration::from_secs(3), monitor.0.wait())
        .await
        .unwrap()
        .unwrap();
    let bytes = std::fs::read(&capture).unwrap();
    assert!(
        bytes.len() >= 4800 * 2 * 4 && bytes.len() % 8 == 0,
        "Missing/invalid stereo monitor PCM: {} bytes",
        bytes.len()
    );
    let mut peaks = [0.0_f32; 2];
    for (index, sample) in bytes.chunks_exact(4).enumerate() {
        let sample = f32::from_le_bytes(sample.try_into().unwrap());
        assert!(sample.is_finite(), "Non-finite DSP output sample");
        peaks[index % 2] = peaks[index % 2].max(sample.abs());
    }
    assert!(
        peaks.into_iter().all(|peak| peak > 0.000001),
        "DSP monitor must contain nonzero audio in BOTH channels: {peaks:?}"
    );
    println!(
        "PCM PROOF: genuine 8-channel stream -> stereo null-sink monitor bytes={} peaks={peaks:?}",
        bytes.len()
    );
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit check_dsp.py OCI gate only; requires staged private server, never host audio"]
async fn real_controller_spatializer_71_stereo_contract() {
    let server = StagedServer::read();
    tokio::time::timeout(Duration::from_secs(120), async {
        let directory = tempfile::Builder::new().prefix("controller-").tempdir_in(&server.runtime).unwrap();
        let paths = AppPaths { config_dir: directory.path().join("config"), data_dir: directory.path().join("data"),
            state_dir: directory.path().join("state"), runtime_dir: directory.path().join("runtime") };
        // Use only exported caller APIs; cfg(test) fake transport is NOT in this library build.
        let mut controller = AudioController::new(paths.clone());
        controller.refresh().await.expect("Public controller initialization/refresh");
        let snapshot = controller.snapshot();
        assert!(snapshot.available);
        assert_eq!(snapshot.surround_state, SurroundState::Off);
        let hrtf = snapshot.hrtf.as_ref().expect("Real system SOFA must be validated through sibling helper");
        assert_eq!(hrtf.reference, "system");
        assert!(hrtf.readable);
        assert_default(&snapshot, &server.dump().await, OTHER);
        controller.set_default_output(TARGET).await.expect("Public set_default_output");
        assert_default(&controller.snapshot(), &server.dump().await, TARGET);
        assert!(controller.snapshot().can_enable_surround, "{:?}", controller.snapshot());
        controller.set_surround(true).await.expect("Actual production Spatializer7.1 startup");
        let first = journal(&paths);
        let dump = server.dump().await;
        assert_eq!(controller.snapshot().surround_state, SurroundState::On);
        assert_default(&controller.snapshot(), &dump, &first.virtual_node_name);
        assert_graph(&dump, &first, &paths, &server);
        controller.set_volume(&first.virtual_node_name, 0.35).await.expect("Public DSP volume");
        controller.set_mute(&first.virtual_node_name, true).await.expect("Public DSP mute");
        assert_level(&controller.snapshot(), &server.dump().await, &first.virtual_node_name, 0.35, true);
        controller.set_mute(&first.virtual_node_name, false).await.unwrap();
        pcm_probe(&server, &first).await;
        controller.apply_profile(&profile(TARGET, true, 0.25, true)).await.expect("Public surround profile");
        let reused = journal(&paths);
        assert_eq!(reused.session_id, first.session_id, "Verified active default should reuse the owned DSP");
        assert_eq!(reused.filter_process_id, first.filter_process_id);
        assert_level(&controller.snapshot(), &server.dump().await, &first.virtual_node_name, 0.25, true);
        controller.set_surround(false).await.expect("Public surround stop/restore");
        wait_removed(&server, &first).await;
        assert_eq!(controller.snapshot().surround_state, SurroundState::Off);
        assert_default(&controller.snapshot(), &server.dump().await, TARGET);
        assert!(!paths.state_dir.join("audio-restore.json").exists());
        assert!(!paths.runtime_dir.join(&first.session_id).exists());
        controller.apply_profile(&profile(TARGET, true, 0.3, false)).await.expect("Restart through surround profile");
        let second = journal(&paths);
        assert_ne!(second.session_id, first.session_id);
        assert_graph(&server.dump().await, &second, &paths, &server);
        controller.apply_profile(&profile(OTHER, false, 0.2, true)).await.expect("Stereo profile stops DSP before switching output");
        wait_removed(&server, &second).await;
        assert_eq!(controller.snapshot().surround_state, SurroundState::Off);
        assert_default(&controller.snapshot(), &server.dump().await, OTHER);
        assert_level(&controller.snapshot(), &server.dump().await, OTHER, 0.2, true);
        assert!(!paths.state_dir.join("audio-restore.json").exists());
        controller.set_default_output(TARGET).await.unwrap();
        controller.set_surround(true).await.unwrap();
        let dropped = journal(&paths);
        drop(controller);
        wait_removed(&server, &dropped).await;
        assert!(paths.state_dir.join("audio-restore.json").is_file(), "Abnormal drop must retain independent recovery journal");
        let mut recovered = AudioController::new(paths.clone());
        recovered.refresh().await.expect("Public refresh must recover retained journal without signalling persisted PIDs");
        assert_eq!(recovered.snapshot().surround_state, SurroundState::Off);
        assert!(!paths.state_dir.join("audio-restore.json").exists());
        assert_default(&recovered.snapshot(), &server.dump().await, TARGET);
        recovered.set_surround(true).await.expect("Recovery permits a new, actually owned DSP");
        let final_record = journal(&paths);
        assert_graph(&server.dump().await, &final_record, &paths, &server);
        recovered.shutdown().await.expect("Public shutdown must confirm owned exit and default restore");
        wait_removed(&server, &final_record).await;
        recovered.refresh().await.unwrap();
        assert_eq!(recovered.snapshot().surround_state, SurroundState::Off);
        assert_default(&recovered.snapshot(), &server.dump().await, TARGET);
        assert!(!paths.state_dir.join("audio-restore.json").exists());
        println!("CONTROLLER PROOF: init=refresh default=confirmed surround=On profile/reuse=confirmed volume/mute=cubic stop/profile/drop-recovery/shutdown=confirmed ownedPids=reaped; synthetic software ONLY");
    }).await.expect("Public controller DSP contract exceeded its finite 120s budget");
}
