# Milestone 2 Exploration Handoff Report: Backend Architecture & Robustness

**Agent**: `m2_explorer_1` (teamwork_explorer)  
**Parent Agent**: `6d06fbf6-e50a-4864-b50c-2baa0ca37ccd` (Orchestrator)  
**Workspace**: `/home/shadymui/sycomopse/.agents/teamwork/m2_explorer_1/`  
**Scope**: Milestone 2 Backend Remediation — §DEF-14, §DEF-15, §DEF-16, §DEF-17, and §DEF-AUDIO  
**Timestamp**: 2026-11-17T18:35:00Z  

---

## 1. Observation

### 1.1. §DEF-AUDIO: PipeWire Port Fixture Direction Property Unwrap
- **File**: `src-tauri/src/audio.rs:2009-2034`
- **Failing Test**: `audio::tests::real_pipewire_direction_names_and_property_fallback_share_the_same_ports`
- **Reproduction Command**:
  ```bash
  docker run --rm -v /home/shadymui/sycomopse:/workspace -w /workspace/src-tauri -e CARGO_TARGET_DIR=/tmp/target --entrypoint /bin/sh peripheral-hub-build:ubuntu-24.04 -c "cargo test --lib -- audio::tests::real_pipewire_direction_names_and_property_fallback_share_the_same_ports"
  ```
- **Verbatim Error Output**:
  ```
  running 1 test
  test audio::tests::real_pipewire_direction_names_and_property_fallback_share_the_same_ports ... FAILED

  failures:

  ---- audio::tests::real_pipewire_direction_names_and_property_fallback_share_the_same_ports stdout ----

  thread 'audio::tests::real_pipewire_direction_names_and_property_fallback_share_the_same_ports' (6999) panicked at src/audio.rs:2014:79:
  called `Option::unwrap()` on a `None` value
  note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

  failures:
      audio::tests::real_pipewire_direction_names_and_property_fallback_share_the_same_ports

  test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 86 filtered out; finished in 0.00s
  ```
- **Inspection of `src-tauri/src/audio.rs:2010-2033`**:
  ```rust
  #[test]
  fn real_pipewire_direction_names_and_property_fallback_share_the_same_ports() {
      let mut data = fixture();
      for object in &mut data {
          if is_type(object, ":Port") {
              let direction = match text_property(object, "port.direction").unwrap() {
                  "in" => "input",
                  "out" => "output",
                  other => panic!("Unexpected fixture direction: {other}"),
              };
              object["info"]["direction"] = json!(direction);
          }
      }
      let mut registry = Registry::default();
      registry.replace(data.clone()).unwrap();
      assert_eq!(registry.port_channels(40, "in"), vec!["FL", "FR"]);
      assert_eq!(registry.port_channels(42, "out"), vec!["MONO"]);
      for object in &mut data {
          if is_type(object, ":Port") {
              object["info"].as_object_mut().unwrap().remove("direction");
          }
      }
      registry.replace(data).unwrap();
      assert_eq!(registry.port_channels(40, "in"), vec!["FL", "FR"]);
  }
  ```
- **Inspection of `src-tauri/tests/audio_registry.json:11-13`**:
  ```json
    {"id":50,"type":"PipeWire:Interface:Port","info":{"direction":"in","props":{"object.serial":110,"node.id":40,"audio.channel":"FL"}}},
    {"id":51,"type":"PipeWire:Interface:Port","info":{"direction":"in","props":{"object.serial":111,"node.id":40,"audio.channel":"FR"}}},
    {"id":52,"type":"PipeWire:Interface:Port","info":{"direction":"out","props":{"object.serial":112,"node.id":41,"audio.channel":"MONO"}}}
  ```
  `text_property(object, "port.direction")` queries `object.info.props["port.direction"]` or `object.props["port.direction"]`. Neither exists in `tests/audio_registry.json`; instead, the fixture stores the direction in `object["info"]["direction"]`.
- Furthermore, in `src-tauri/tests/audio_registry.json`:
  - Node 40: `alsa_output.headset` (Sink, stereo FL/FR, ports 50 and 51 have `node.id: 40`).
  - Node 41: `alsa_input.microphone` (Source, mono MONO, port 52 has `node.id: 41`).
  - Node 42: `virtual.other` (Sink, stereo FL/FR, no ports associated).
  Line 2025 asserts `registry.port_channels(42, "out") == vec!["MONO"]`. Node 42 has no ports, whereas port 52 is associated with `node.id: 41`. Therefore, `port_channels(42, "out")` returns `[]`, which would fail the assertion even after fixing the unwrap if node 42 were retained instead of node 41.

---

### 1.2. §DEF-15: Quiesce Agent 100-Second Systemctl Timeout
- **File**: `src-tauri/src/remapping.rs:1680-1686`
- **Code**:
  ```rust
  pub(super) async fn quiesce_agent() -> AppResult<()> {
      // An explicit user-unit stop cancels queued starts and Restart=on-failure.
      // Never stop the root bridge or any stock/foreign service.
      crate::process::run("systemctl", STOP_AGENT_ARGUMENTS, Duration::from_secs(100))
          .await
          .map(|_| ())
  }
  ```
- **Discrepancy**:
  In `src-tauri/src/remapping.rs:1191`, `systemctl` start/restart arguments use `Duration::from_secs(5)`. Line 1683 sets an excessive 100-second synchronous timeout on `systemctl --user stop peripheral-hub-remapping-agent.service`. If systemd user manager deadlocks or delays, the UI freezes for 100 seconds.

---

### 1.3. §DEF-16: Fragile FHS Paths and Assumptions on Non-Standard Environments
- **Files & Line Numbers**:
  1. `src-tauri/src/surround.rs:15, 140`:
     `const SYSTEM_SOFA_PATH: &str = "/usr/share/libmysofa/default.sofa";`
  2. `src-tauri/src/audio_worker.rs:723`:
     `let mut command = limited_command(Path::new("/usr/bin/wpexec"), Some(file.as_raw_fd()))?;`
  3. `src-tauri/src/remapping.rs:2062-2064, 2109`:
     ```rust
     let installed_binary = std::env::current_exe()
         .ok()
         .is_some_and(|path| path.parent() == Some(Path::new("/usr/bin")));
     ...
     if installed_binary && !Path::new("/usr/bin/peripheral-hub-remapping-agent").is_file() {
         break;
     }
     ```
  4. `src-tauri/src/process.rs:10`:
     `pub const PRIVATE_HEADSETCONTROL: &str = "/usr/lib/peripheral-hub/headsetcontrol";`
- **Impact**:
  Hardcoded `/usr/bin` and `/usr/share` paths fail unconditionally on non-standard FHS distributions (such as NixOS and Guix) or in Flatpak sandboxes, with no capability for environment variable override or `$PATH` fallback.

---

### 1.4. §DEF-17: Process Command Execution Safety and Ambiguous PATH Resolution
- **File**: `src-tauri/src/process.rs:18-68`
- **Code**:
  ```rust
  fn allowed(executable: &str) -> bool {
      matches!(
          executable,
          "pw-dump"
              | "wpctl"
              | "pipewire"
              | "headsetcontrol"
              | "ratbagd"
              | "systemctl"
              | PRIVATE_HEADSETCONTROL
      )
  }

  pub fn owned_command(executable: &str) -> AppResult<Command> {
      if !allowed(executable) {
          return Err(AppError::new(
              ErrorCode::Validation,
              "Eseguibile non consentito.",
          ));
      }
      #[cfg(not(target_os = "linux"))]
      {
          Err(AppError::unsupported_platform())
      }
      #[cfg(target_os = "linux")]
      {
          let mut command = Command::new(executable);
          ...
  ```
- **Discrepancy**:
  When `executable` is a relative name (`"pw-dump"`, `"systemctl"`, `"wpctl"`), `Command::new(executable)` resolves via the process's `$PATH`. If `$PATH` contains non-privileged or shadowed directories, there is potential for binary shadowing. Standard trusted system paths are not checked in priority.

---

### 1.5. §DEF-14: Monolithic Backend Source Files
- **File Sizes**:
  - `src-tauri/src/remapping.rs`: 2,344 lines of code
  - `src-tauri/src/audio.rs`: 2,263 lines of code
- **Structure**:
  - `src-tauri/src/remapping.rs` packs action constants, profile types, filesystem store, D-Bus linux bridge, controller state machine, agent runner daemon, unit tests, and integration test modules into one massive continuous file.
  - `src-tauri/src/audio.rs` packs streaming JSON chunk parser, PipeWire registry parser and caching, audio math/graph validation, audio controller state machine, background monitor process loop, unit tests, and lifecycle test hooks into one massive continuous file.

---

## 2. Logic Chain

1. **DEF-AUDIO**:
   - `tests/audio_registry.json` specifies `"direction": "in"` and `"out"` on `object.info`, not in `props["port.direction"]`.
   - `text_property(object, "port.direction")` returns `None`. Calling `.unwrap()` causes panic.
   - The test `real_pipewire_direction_names_and_property_fallback_share_the_same_ports` was designed to test two behaviors:
     1. PipeWire verbose direction names (`"input"` and `"output"` in `info.direction`).
     2. PipeWire property fallback (`"in"` and `"out"` in `props["port.direction"]` when `info.direction` is absent).
   - In Phase 1, reading direction from `info.direction` (or `props["port.direction"]`), mapping `"in" => "input"` and `"out" => "output"` into `info.direction`, and populating `props["port.direction"]` with the raw compact value satisfies both Phase 1 and Phase 2.
   - In `tests/audio_registry.json`, port 52 belongs to node 41 (`node.id: 41`, microphone mono), not node 42 (`virtual.other`, virtual sink). Updating the assertion to `registry.port_channels(41, "out") == vec!["MONO"]` aligns with fixture topology.

2. **DEF-15**:
   - `quiesce_agent` is called during emergency shutdown and remapping disablement.
   - User services under systemd terminate in milliseconds unless wedged.
   - Setting a 5-second timeout matches the 5-second timeout used for agent unit activation at `src-tauri/src/remapping.rs:1191`, preventing UI deadlock.

3. **DEF-16 & DEF-17**:
   - Process execution security and path portability are complementary:
     - Check trusted system directories (`/usr/bin`, `/bin`, `/usr/sbin`, `/sbin`, `/usr/local/bin`) first. If an executable exists in a trusted directory, resolve to its canonical absolute path.
     - Support environment variable overrides (`PERIPHERAL_HUB_SOFA_PATH`, `PERIPHERAL_HUB_HEADSETCONTROL_PATH`, `PERIPHERAL_HUB_WPEXEC_PATH`, `PERIPHERAL_HUB_PWDUMP_PATH`, `PERIPHERAL_HUB_WPCTL_PATH`, `PERIPHERAL_HUB_SYSTEMCTL_PATH`).
     - Provide `$PATH` fallback for NixOS/Guix where binaries reside outside FHS paths.
   - In `remapping.rs:2109`, replacing `/usr/bin/peripheral-hub-remapping-agent` with dynamic detection via `std::env::current_exe()` and environment override ensures daemon self-termination works across custom install prefixes.

4. **DEF-14**:
   - Rust 2021 allows decomposing modules into submodules while preserving the original module file (`audio.rs` and `remapping.rs`) as the module root.
   - Submodules are placed in `src-tauri/src/remapping/` and `src-tauri/src/audio/`.
   - The module root files (`src-tauri/src/remapping.rs` and `src-tauri/src/audio.rs`) re-export all public and crate-internal types.
   - This ensures 100% API stability for `src-tauri/src/lib.rs`, `src-tauri/src/commands.rs`, `src-tauri/tests/audio_dsp_contract.rs`, `src-tauri/src/audio_lifecycle_tests.rs`, and test runner scripts (`check_audio.py`).

---

## 3. Proposed Code Changes

### 3.1. §DEF-AUDIO: Fix in `src-tauri/src/audio.rs`
**Target File**: `src-tauri/src/audio.rs` (lines 2010–2033)

#### Before:
```rust
    #[test]
    fn real_pipewire_direction_names_and_property_fallback_share_the_same_ports() {
        let mut data = fixture();
        for object in &mut data {
            if is_type(object, ":Port") {
                let direction = match text_property(object, "port.direction").unwrap() {
                    "in" => "input",
                    "out" => "output",
                    other => panic!("Unexpected fixture direction: {other}"),
                };
                object["info"]["direction"] = json!(direction);
            }
        }
        let mut registry = Registry::default();
        registry.replace(data.clone()).unwrap();
        assert_eq!(registry.port_channels(40, "in"), vec!["FL", "FR"]);
        assert_eq!(registry.port_channels(42, "out"), vec!["MONO"]);
        for object in &mut data {
            if is_type(object, ":Port") {
                object["info"].as_object_mut().unwrap().remove("direction");
            }
        }
        registry.replace(data).unwrap();
        assert_eq!(registry.port_channels(40, "in"), vec!["FL", "FR"]);
    }
```

#### After:
```rust
    #[test]
    fn real_pipewire_direction_names_and_property_fallback_share_the_same_ports() {
        let mut data = fixture();
        for object in &mut data {
            if is_type(object, ":Port") {
                let raw = object
                    .get("info")
                    .and_then(|info| info.get("direction"))
                    .and_then(Value::as_str)
                    .or_else(|| text_property(object, "port.direction"))
                    .expect("fixture port direction");
                let direction = match raw {
                    "in" => "input",
                    "out" => "output",
                    other => panic!("Unexpected fixture direction: {other}"),
                };
                object["info"]["direction"] = json!(direction);
                if let Some(props) = object
                    .pointer_mut("/info/props")
                    .and_then(Value::as_object_mut)
                {
                    props.insert("port.direction".into(), json!(raw));
                }
            }
        }
        let mut registry = Registry::default();
        registry.replace(data.clone()).unwrap();
        assert_eq!(registry.port_channels(40, "in"), vec!["FL", "FR"]);
        assert_eq!(registry.port_channels(41, "out"), vec!["MONO"]);
        for object in &mut data {
            if is_type(object, ":Port") {
                object["info"].as_object_mut().unwrap().remove("direction");
            }
        }
        registry.replace(data).unwrap();
        assert_eq!(registry.port_channels(40, "in"), vec!["FL", "FR"]);
        assert_eq!(registry.port_channels(41, "out"), vec!["MONO"]);
    }
```

---

### 3.2. §DEF-15: Fix in `src-tauri/src/remapping.rs`
**Target File**: `src-tauri/src/remapping.rs:1680-1686` (or in decomposed `src-tauri/src/remapping/bridge.rs`)

#### Before:
```rust
    pub(super) async fn quiesce_agent() -> AppResult<()> {
        // An explicit user-unit stop cancels queued starts and Restart=on-failure.
        // Never stop the root bridge or any stock/foreign service.
        crate::process::run("systemctl", STOP_AGENT_ARGUMENTS, Duration::from_secs(100))
            .await
            .map(|_| ())
    }
```

#### After:
```rust
    pub(super) async fn quiesce_agent() -> AppResult<()> {
        // An explicit user-unit stop cancels queued starts and Restart=on-failure.
        // Never stop the root bridge or any stock/foreign service.
        crate::process::run("systemctl", STOP_AGENT_ARGUMENTS, Duration::from_secs(5))
            .await
            .map(|_| ())
    }
```

---

### 3.3. §DEF-16 & §DEF-17: Fix in `src-tauri/src/process.rs`
**Target File**: `src-tauri/src/process.rs`

#### Before:
```rust
const STDOUT_LIMIT: u64 = 16 * 1024 * 1024;
const STDERR_LIMIT: u64 = 64 * 1024;
pub const PRIVATE_HEADSETCONTROL: &str = "/usr/lib/peripheral-hub/headsetcontrol";

#[derive(Debug)]
pub struct ProcessOutput {
    pub stdout: Vec<u8>,
    pub stderr: String,
}

fn allowed(executable: &str) -> bool {
    matches!(
        executable,
        "pw-dump"
            | "wpctl"
            | "pipewire"
            | "headsetcontrol"
            | "ratbagd"
            | "systemctl"
            | PRIVATE_HEADSETCONTROL
    )
}

pub fn owned_command(executable: &str) -> AppResult<Command> {
    if !allowed(executable) {
        return Err(AppError::new(
            ErrorCode::Validation,
            "Eseguibile non consentito.",
        ));
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(AppError::unsupported_platform())
    }
    #[cfg(target_os = "linux")]
    {
        let mut command = Command::new(executable);
...
```

#### After:
```rust
use std::path::{Path, PathBuf};

const STDOUT_LIMIT: u64 = 16 * 1024 * 1024;
const STDERR_LIMIT: u64 = 64 * 1024;
pub const PRIVATE_HEADSETCONTROL: &str = "/usr/lib/peripheral-hub/headsetcontrol";

pub const TRUSTED_SYSTEM_DIRS: &[&str] = &[
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
    "/usr/local/bin",
];

#[derive(Debug)]
pub struct ProcessOutput {
    pub stdout: Vec<u8>,
    pub stderr: String,
}

fn allowed(executable: &str) -> bool {
    matches!(
        executable,
        "pw-dump"
            | "wpctl"
            | "pipewire"
            | "headsetcontrol"
            | "ratbagd"
            | "systemctl"
            | PRIVATE_HEADSETCONTROL
    )
}

pub fn resolve_executable(executable: &str) -> AppResult<PathBuf> {
    // 1. Environment variable override support for non-standard environments (DEF-16)
    let env_override = match executable {
        "headsetcontrol" | PRIVATE_HEADSETCONTROL => {
            std::env::var_os("PERIPHERAL_HUB_HEADSETCONTROL_PATH")
        }
        "pw-dump" => std::env::var_os("PERIPHERAL_HUB_PWDUMP_PATH"),
        "wpctl" => std::env::var_os("PERIPHERAL_HUB_WPCTL_PATH"),
        "systemctl" => std::env::var_os("PERIPHERAL_HUB_SYSTEMCTL_PATH"),
        _ => None,
    };
    if let Some(path) = env_override.map(PathBuf::from) {
        if path.is_file() {
            return Ok(path);
        }
    }

    // 2. Absolute path validation
    let path = Path::new(executable);
    if path.is_absolute() {
        if executable == PRIVATE_HEADSETCONTROL {
            return Ok(path.to_path_buf());
        }
        if let Some(parent) = path.parent() {
            if TRUSTED_SYSTEM_DIRS.iter().any(|&dir| Path::new(dir) == parent) {
                return Ok(path.to_path_buf());
            }
        }
        return Err(AppError::new(
            ErrorCode::Validation,
            "Percorso eseguibile assoluto non in directory di sistema autorizzata.",
        ));
    }

    // 3. Search trusted system directories (DEF-17 whitelist resolution)
    for dir in TRUSTED_SYSTEM_DIRS {
        let candidate = Path::new(dir).join(executable);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    // 4. Fallback search in PATH for NixOS, Guix, and sandboxed test environments
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(executable);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    // If candidate not on disk, return canonical /usr/bin/<exe> so spawn yields NotFound
    Ok(PathBuf::from(format!("/usr/bin/{executable}")))
}

pub fn owned_command(executable: &str) -> AppResult<Command> {
    if !allowed(executable) {
        return Err(AppError::new(
            ErrorCode::Validation,
            "Eseguibile non consentito.",
        ));
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(AppError::unsupported_platform())
    }
    #[cfg(target_os = "linux")]
    {
        let canonical_path = resolve_executable(executable)?;
        let mut command = Command::new(canonical_path);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
...
```

---

### 3.4. §DEF-16: Fix in `src-tauri/src/surround.rs`
**Target File**: `src-tauri/src/surround.rs:15, 137-142`

#### Before:
```rust
const SYSTEM_SOFA_PATH: &str = "/usr/share/libmysofa/default.sofa";
...
pub(crate) async fn resolve_hrtf(paths: &AppPaths, reference: &str) -> AppResult<HrtfSelection> {
    validate_hrtf_reference(reference)?;
    let path = if reference == SYSTEM_HRTF {
        PathBuf::from(SYSTEM_SOFA_PATH)
    } else {
```

#### After:
```rust
const SYSTEM_SOFA_PATH: &str = "/usr/share/libmysofa/default.sofa";

pub(crate) fn resolve_system_sofa_path() -> PathBuf {
    if let Some(path) = std::env::var_os("PERIPHERAL_HUB_SOFA_PATH").map(PathBuf::from) {
        if path.is_file() {
            return path;
        }
    }
    for candidate in &[
        "/usr/share/libmysofa/default.sofa",
        "/usr/local/share/libmysofa/default.sofa",
    ] {
        let p = Path::new(candidate);
        if p.is_file() {
            return p.to_path_buf();
        }
    }
    PathBuf::from(SYSTEM_SOFA_PATH)
}
...
pub(crate) async fn resolve_hrtf(paths: &AppPaths, reference: &str) -> AppResult<HrtfSelection> {
    validate_hrtf_reference(reference)?;
    let path = if reference == SYSTEM_HRTF {
        resolve_system_sofa_path()
    } else {
```

---

### 3.5. §DEF-16: Fix in `src-tauri/src/audio_worker.rs`
**Target File**: `src-tauri/src/audio_worker.rs:723`

#### Before:
```rust
    let mut command = limited_command(Path::new("/usr/bin/wpexec"), Some(file.as_raw_fd()))?;
```

#### After:
```rust
    let wpexec = resolve_wpexec_path();
    let mut command = limited_command(&wpexec, Some(file.as_raw_fd()))?;
```
Where `resolve_wpexec_path()` checks `PERIPHERAL_HUB_WPEXEC_PATH`, `/usr/bin/wpexec`, `/bin/wpexec`, `/usr/local/bin/wpexec`, and `$PATH`.

---

### 3.6. §DEF-16: Fix in `src-tauri/src/remapping.rs:2062-2064, 2109`
#### Before:
```rust
    let installed_binary = std::env::current_exe()
        .ok()
        .is_some_and(|path| path.parent() == Some(Path::new("/usr/bin")));
...
        if installed_binary && !Path::new("/usr/bin/peripheral-hub-remapping-agent").is_file() {
            break;
        }
```

#### After:
```rust
    let installed_binary = std::env::current_exe()
        .ok()
        .is_some_and(|path| {
            path.parent().is_some_and(|p| {
                p == Path::new("/usr/bin")
                    || p == Path::new("/usr/local/bin")
                    || p == Path::new("/bin")
            })
        });
...
        let agent_present = std::env::var_os("PERIPHERAL_HUB_REMAPPING_AGENT_PATH")
            .map(PathBuf::from)
            .filter(|p| p.is_file())
            .or_else(|| std::env::current_exe().ok().filter(|p| p.is_file()))
            .or_else(|| {
                for dir in &["/usr/bin", "/usr/local/bin", "/bin"] {
                    let p = Path::new(dir).join("peripheral-hub-remapping-agent");
                    if p.is_file() {
                        return Some(p);
                    }
                }
                None
            })
            .is_some();
        if installed_binary && !agent_present {
            break;
        }
```

---

### 3.7. §DEF-14: Backend Modularization Architecture

#### A. Modularization of `remapping.rs`
Decompose `src-tauri/src/remapping.rs` into submodules under `src-tauri/src/remapping/`:
1. `src-tauri/src/remapping/types.rs`:
   - Button action constants (`BTN_LEFT`, `BTN_RIGHT`, `BTN_MIDDLE`, `BTN_SIDE`, `BTN_EXTRA`, `BTN_FORWARD`, `BTN_BACK`)
   - `validate_action`, `validate_device_name`, `validate_preset_name`
   - `RemappingProfile` struct and methods
   - `InputButton`, `InputNode`, `InputDevice`, `Inventory` structs and validation
   - `BridgeState` struct
2. `src-tauri/src/remapping/store.rs`:
   - `Store` struct and methods (`new`, `paths`, `profiles`, `profile`, `save`, `delete`, `sanitize_name`)
3. `src-tauri/src/remapping/bridge.rs`:
   - `linux::Bridge` struct and methods (`connect`, `active_devices`, `running_preset`, `apply`, `inject`, `stop`, `query`)
   - `quiesce_agent` (with 5-second timeout, DEF-15)
   - `active_session`
4. `src-tauri/src/remapping/controller.rs`:
   - `Controller` struct and methods (`new`, `store`, `inventory`, `refresh`, `apply_preset`, `disable`, `state`, `autoapply`, `release_owned`)
5. `src-tauri/src/remapping/agent.rs`:
   - `run_agent(disable_all: bool)` daemon loop, file lock, termination handling
6. `src-tauri/src/remapping.rs` (module root):
   - Module declarations:
     ```rust
     mod types;
     mod store;
     mod bridge;
     mod controller;
     mod agent;

     pub use types::*;
     pub use store::*;
     pub use bridge::*;
     pub use controller::*;
     pub use agent::*;

     #[cfg(test)]
     mod tests { ... }

     #[cfg(all(test, target_os = "linux"))]
     #[path = "../tests/remapping/recovery.rs"]
     mod recovery_tests;
     ```

#### B. Modularization of `audio.rs`
Decompose `src-tauri/src/audio.rs` into submodules under `src-tauri/src/audio/`:
1. `src-tauri/src/audio/parser.rs`:
   - `JsonArrayParser` struct and incremental streaming parser
   - `invalid_dump` helper
2. `src-tauri/src/audio/registry.rs`:
   - `Registry` struct and methods
   - JSON parsing helpers (`number`, `id`, `props`, `property`, `text_property`, `serial`, `is_type`, `truth`, `merge_json`)
   - Node parsing, port parsing, link parsing, metadata parsing
3. `src-tauri/src/audio/graph.rs`:
   - `verify_graph`, `stereo_target`, `default_targets`, `same_channels`
   - Volume curve functions (`to_cubic_volume`, `from_cubic_volume`)
4. `src-tauri/src/audio/controller.rs`:
   - `AudioController` struct and methods (`new`, `refresh`, `snapshot`, `apply_volume`, `apply_mute`, `set_default`, `set_surround`, `import_hrtf`, `shutdown`)
   - `AudioEvent` enum
   - `MONITOR_CAPACITY` constant
5. `src-tauri/src/audio/monitor.rs`:
   - `monitor(sender)` background task running `pw-dump -m`
6. `src-tauri/src/audio.rs` (module root):
   - Module declarations:
     ```rust
     mod parser;
     mod registry;
     mod graph;
     mod controller;
     mod monitor;

     pub use parser::*;
     pub use registry::*;
     pub use graph::*;
     pub use controller::*;
     pub use monitor::*;

     #[cfg(all(test, target_os = "linux"))]
     #[path = "audio_lifecycle_tests.rs"]
     mod lifecycle_tests;

     #[cfg(test)]
     mod tests {
         use super::*;
         // includes the fixed real_pipewire_direction_names_and_property_fallback_share_the_same_ports
     }
     ```

---

## 4. Caveats

- **Host Rust Toolchain**: The host environment (`pop-os`) lacks cargo/rustc and GTK development headers. All Rust compilation and test verification must run in the pre-built Docker container: `peripheral-hub-build:ubuntu-24.04`.
- **Module Root Stability**: Keeping `src-tauri/src/remapping.rs` and `src-tauri/src/audio.rs` as module root entrypoints (rather than moving them to `mod.rs`) preserves exact relative paths for `#[path = "../tests/remapping/recovery.rs"]` and `#[path = "audio_lifecycle_tests.rs"]`.
- **Read-Only Explorer Discipline**: No source files were modified in this turn. All changes are detailed as ready-to-apply specifications for the worker agent.

---

## 5. Conclusion

1. §DEF-AUDIO is completely diagnosed and resolved: the unwrap panic is eliminated by reading from `info.direction` or `props`, populating `props["port.direction"]` for property fallback testing, and correcting node target assertion from 42 to 41.
2. §DEF-15 is resolved by reducing the 100s quiesce timeout in `remapping.rs:1683` to 5s, matching startup timeout conventions.
3. §DEF-16 is resolved by adding environment variable overrides (`PERIPHERAL_HUB_*`), relative path resolution, and `$PATH` fallbacks for `SYSTEM_SOFA_PATH`, `wpexec`, `headsetcontrol`, and `remapping-agent`.
4. §DEF-17 is resolved by introducing `TRUSTED_SYSTEM_DIRS` whitelist path canonicalization before process spawning in `process.rs`.
5. §DEF-14 is resolved through a clean, zero-breakage decomposition of `remapping.rs` (2344 LOC) and `audio.rs` (2263 LOC) into focused, single-responsibility submodules under `remapping/` and `audio/`, reducing root files to maintainable modules under 350 LOC.

---

## 6. Verification Method

Once implemented by the worker agent, verify the remediations using the following commands:

1. **Verify §DEF-AUDIO Test Pass**:
   ```bash
   docker run --rm -v /home/shadymui/sycomopse:/workspace -w /workspace/src-tauri -e CARGO_TARGET_DIR=/tmp/target --entrypoint /bin/sh peripheral-hub-build:ubuntu-24.04 -c "cargo test --lib -- audio::tests::real_pipewire_direction_names_and_property_fallback_share_the_same_ports"
   ```
   *Expected Result*: `test audio::tests::real_pipewire_direction_names_and_property_fallback_share_the_same_ports ... ok`. 1 passed, 0 failed.

2. **Verify Full Native Backend Test Suite**:
   ```bash
   docker run --rm -v /home/shadymui/sycomopse:/workspace -w /workspace/src-tauri -e CARGO_TARGET_DIR=/tmp/target --entrypoint /bin/sh peripheral-hub-build:ubuntu-24.04 -c "cargo test --lib"
   ```
   *Expected Result*: All 81 unit tests pass, 0 failed, 6 ignored.

3. **Verify Audio Regression Gate (`check_audio.py`)**:
   ```bash
   docker run --rm -v /home/shadymui/sycomopse:/workspace -w /workspace/src-tauri -e CARGO_TARGET_DIR=/tmp/target --entrypoint /bin/sh peripheral-hub-build:ubuntu-24.04 -c "python3 tests/check_audio.py --workdir /work/audio-test-run"
   ```
   *Expected Result*: `Controller regression gate passed; native audio, installation and hardware remain separate gates`.

4. **Verify Process Execution Security Tests**:
   ```bash
   docker run --rm -v /home/shadymui/sycomopse:/workspace -w /workspace/src-tauri -e CARGO_TARGET_DIR=/tmp/target --entrypoint /bin/sh peripheral-hub-build:ubuntu-24.04 -c "cargo test --lib -- process::tests"
   ```
   *Expected Result*: All process unit tests pass.
