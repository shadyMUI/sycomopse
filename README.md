# Sycomopse

**Sycomopse 0.1.0** brings mouse settings, button remapping and headset audio into one Linux desktop app built with **Tauri 2** and **React 19**. It is an independent project inspired by the all-in-one approach of Razer Synapse, not an official Razer client. You can use the Mouse or Audio panel on its own; both ship in the same suite.

The Linux package and command names still use `peripheral-hub`. The rename does not change service names or profile directories; existing profiles need no migration.

It provides native integration with key Linux subsystems:
- **OpenRazer and libratbag / ratbagd**: mouse DPI and polling settings where the device and its driver expose verified controls.
- **HeadsetControl (4.2.0)**: read-only battery reporting for supported Logitech headsets that the app can identify. Sycomopse does not currently set sidetone, chat/game mix or headset LEDs.
- **PipeWire / WirePlumber**: system audio routing, output volume, microphone gain/mute, a ten-band headphone equalizer, and optional virtual 7.1 playback using a SOFA HRTF.
- **Input Remapping Engine**: mouse buttons, key bindings, keyboard shortcuts and hold-to-repeat clicks via an isolated evdev/uinput service.
- **UPower**: unified power and battery level monitoring for standard wireless and Bluetooth peripherals.

The user interface (GUI) and background session agents are licensed under the **MIT License**. The private input remapping engine is an isolated standalone process licensed under the **GPL-3.0** (licenses, attributions, and corresponding source archives are included in every distribution package and bundle).

---

## Table of Contents

1. [Key Features](#key-features)
2. [Native Package Installation](#native-package-installation)
3. [Architecture & Security Model](#architecture--security-model)
4. [Remapping Agent & Session Recovery](#remapping-agent--session-recovery)
5. [User Consents, Drivers & Hardware State](#user-consents-drivers--hardware-state)
6. [Repair, Verification & Removal](#repair-verification--removal)
7. [Distribution Support Matrix & CI Validation](#distribution-support-matrix--ci-validation)
8. [Local Development & Building](#local-development--building)
9. [Troubleshooting](#troubleshooting)

---

## Key Features

### 🖱️ Mouse Management & Performance
- Apply supported DPI and polling settings and read back the device state. For the Viper V3 HyperSpeed the polling options are 125, 500 and 1000 Hz; other models depend on their driver.
- Check battery level and charging status in the dashboard when a provider reports them. Unknown values are not estimates.

### Button Remapping
- Remap physical mouse buttons (e.g., thumb buttons 4 & 5) to another click, a key, or a keyboard shortcut. At least one direct left-click must remain available.
- **Hold to repeat:** choose a target of 1–100 clicks per second for the autoclicker. A quick tap sends one click. The setting is a software target, not a guarantee that every application will register every click.
- Low-level event injection via a virtual `uinput` device.
- Instant fallback and fail-safe recovery via the non-root desktop entry **"Restore Mouse"** or CLI command.

### 🎧 Spatial Audio & Headset Controls
- Choose a physical playback output and adjust output volume or microphone gain/mute. These controls belong to the Linux audio session, not the headset firmware.
- Apply the physical headphones under **Audio → Output** before enabling EQ or surround. If another app changes the default, Sycomopse requires you to confirm the target again rather than silently sending audio to a different device.
- Enable virtual 7.1 to render multichannel audio as binaural stereo using PipeWire and `libmysofa`. The virtual output receives the application's audio; the physical headphones play it. Apps pinned to another sink are not forcibly moved.

### Headphone equalizer
1. In **Audio → Headphone equalizer**, start with **Neutral**, **Voice** or **Gaming**, or adjust the ten bands from **31.5 Hz to 16 kHz** (±12 dB). Choose **Apply EQ** to activate it. Once active, gains, presets, preamp and **Advanced: per-band Q** update the existing filter while you listen, without creating another audio output. A lower Q shapes a broader range of frequencies.
2. **Bypass EQ** compares playback with the saved curve removed. It does not delete your settings or disable active surround. Stereo EQ needs no SOFA file; with 7.1 enabled the same curve processes the two ear signals *after* spatialization. Neither mode processes the microphone.
3. Settings are remembered per physical PipeWire output. **Audio profiles → New profile → Include equalizer in profile** captures the confirmed bands, preamp and bypass state in a named preset. Existing profiles without an EQ use the target output's saved curve. Startup or plugging in another headset does not silently reapply processing.

**Preamp mode:** **Auto** reserves headroom by subtracting the sum of positive band gains before filtering. **Manual** starts at **0 dB** and lets you set **−24 to 0 dB** without automatic compensation. With Manual at 0 dB, boosts can clip on loud material; lower the preamp if you hear distortion. Existing curves with no preamp setting retain **Auto**.

---

## Native Package Installation

### 1. Choose and download the bundle

Check your distro version, architecture and Python version:

```sh
cat /etc/os-release
uname -m
python3 --version
```

Use the exact **target ID** from the [distribution matrix](#distribution-support-matrix--ci-validation). A package built for another distro or version is not interchangeable just because it uses the same format. **Pop!_OS 24.04 with `UBUNTU_CODENAME=noble` uses the `ubuntu-24.04` bundle.**

On [GitHub Releases](https://github.com/shadyMUI/sycomopse/releases), download the matching `.tar.gz` and its `.tar.gz.sha256` file from **Assets**, if available. Do not download GitHub's automatic **Source code** archives to install the app, and do not extract or install just the `.deb`, `.rpm`, or `.pkg.tar.zst`. The installer needs the complete bundle, including `install.sh`, the manifest and its checksums.

If there is no matching release asset, look for the matching bundle in a successful [Linux Native Packaging run](https://github.com/shadyMUI/sycomopse/actions/workflows/linux-packaging.yml) or [build that target locally](#building-native-distribution-packages). CI artifacts are named `peripheral-hub-0.1.0-<target>-x86_64` and retained for 14 days. `packaging/out/` is generated locally; cloning the repository does not download the packages.

### 2. Verify, extract and preview

Open a terminal where you downloaded both release files. This example is for Ubuntu 24.04 / Pop!_OS 24.04. For another build, set `archive` to the exact downloaded filename and `target` to its target ID.

```sh
archive='Sycomopse-0.1.0-ubuntu-24.04-x86_64-47a3cfe.tar.gz'
target='ubuntu-24.04'

sha256sum -c "${archive}.sha256" &&
unpack_dir=$(mktemp -d ./sycomopse.XXXXXX) &&
tar -xzf "$archive" -C "$unpack_dir" &&
cd "$unpack_dir/peripheral-hub-0.1.0-${target}-x86_64" &&
sh install.sh install --dry-run
```

For an Actions artifact, extract its ZIP instead, open the directory containing `install.sh` and `manifest.json`, and run `sh install.sh install --dry-run`. The installer verifies the internal manifest and file hashes. The dry run checks the target and shows the plan without installing anything. If verification or the dry run fails, stop there.

### 3. Install and launch

In the same bundle directory, as your normal desktop user, run:

```sh
sh install.sh install && sh install.sh status
```

Do not prefix the installer with `sudo`. It asks for administrative permissions when needed. After installation, open **Sycomopse** from the app menu or run `peripheral-hub` from the terminal. See [Repair, Verification & Removal](#repair-verification--removal) for maintenance commands.

The wrapper checks the manifest and target and uses the native package manager for installation. The `--without-audio` and `--without-openrazer` options affect the wrapper's setup steps; they do **not** produce a smaller package or remove its declared dependencies. In particular, the Ubuntu, Debian, Arch and CachyOS packages currently require OpenRazer/DKMS even if you only use Audio.

---

## Architecture & Security Model

The application enforces strict privilege separation across dedicated processes:

```
┌─────────────────────────────────────────────────────────────┐
│                      Sycomopse GUI                          │
│             (Tauri 2 / Rust + React 19 Frontend)            │
│                 Executed as Standard User                   │
└──────────────┬───────────────────────────────┬──────────────┘
               │ IPC / Custom Protocol         │ Session D-Bus
               ▼                               ▼
┌───────────────────────────────┐ ┌───────────────────────────┐
│  peripheral-hub-audio-worker  │ │ peripheral-hub-remapping- │
│  (PipeWire Headless Worker)   │ │ agent (Session Agent)     │
└───────────────────────────────┘ └─────────────┬─────────────┘
                                                │ Private System D-Bus
                                                ▼ (io.peripheralhub.InputRemapper)
                                  ┌───────────────────────────┐
                                  │ peripheral-hub-remapper-  │
                                  │ service (GPL Daemon)      │
                                  └───────────────────────────┘
```

- **Process Privilege Isolation**: Neither the GUI nor the background agents require root privileges.
- **Non-Invasive System Integration**: Native hooks never overwrite external configuration files, do not pipe unverified remote scripts to bash, and respect administrative system policies (`policy-rc.d`, systemd masks).
- **License Compliance**: The GPL Input Remapper engine is isolated across a private system D-Bus bus (`io.peripheralhub.InputRemapper`) without static linking to the MIT-licensed application code. Complete corresponding source archives are included in the bundle under `sources/`.

---

## Remapping Agent & Session Recovery

The package installs:
- **GUI**: `/usr/bin/peripheral-hub`
- **Session Agent**: `/usr/bin/peripheral-hub-remapping-agent` (compiled with Cargo `--no-default-features`)
- **Audio Worker**: `/usr/bin/peripheral-hub-audio-worker`
- **Systemd User Unit**: `peripheral-hub-remapping-agent.service` (auto-started via `/etc/xdg/autostart/io.peripheralhub.RemappingAgent.desktop`)
- **Private Remapper Engine**: `/usr/lib/peripheral-hub/remapper/` and wrapper `/usr/lib/peripheral-hub/remapper-service`
- **Systemd System Unit**: `peripheral-hub-input-remapper.service`
- **Desktop Recovery Shortcut**: **"Restore Mouse"** (`io.peripheralhub.Recovery.desktop`) to immediately reset mouse mappings without opening a terminal.

### Quick Recovery Commands
If button mappings become unresponsive or you want to temporarily disable all active profiles:

```bash
# Immediately release and disable all active remapping presets
peripheral-hub-remapping-agent --disable-all

# Inspect current agent status
peripheral-hub-remapping-agent --status
```

Recovery state is maintained in a private ledger separate from user profiles: it can safely detach active presets even if a profile configuration file is damaged or corrupted.

---

## User Consents, Drivers & Hardware State

Review the installer plan and the package manager's proposed dependencies before proceeding:
- **Audio session**: the installer asks before switching a PulseAudio setup to PipeWire or replacing the session manager. Installing audio libraries is not the same as switching the running session.
- **ratbagd**: the installer's explicit activation step is opt-in. Installing the native package can still bring in libratbag or ratbagd as a dependency.
- **Kernel Headers & DKMS (OpenRazer)**:
  - Kernel headers must match the running kernel version (on Fedora, exact `kernel-devel` is required, not `kernel-debug-devel`; on CachyOS, matching `pkgbase`).
  - If **Secure Boot** is enabled, signing the DKMS module requires MOK enrollment through the distribution's official tooling. Insecure automated keys are never generated.
- **`plugdev` Group & Udev Permissions**: To access Razer hardware without root privileges, add your user to the `plugdev` group:
  ```bash
  sudo gpasswd -a $USER plugdev
  ```
  *(Requires logging out and back in for changes to take effect).*

---

## Repair, Verification & Removal

From your extracted bundle directory, as the same desktop user:

```bash
# Verify bundle integrity and check service status
sh install.sh status

# Repair missing or modified files, restore permissions, and reload units
sh install.sh repair

# Clean uninstallation (preserves user configuration and profiles)
sh install.sh uninstall
```

- `repair`: checks the bundle's manifest, reinstalls the native payload, and reports any remaining service or session setup steps.
- `uninstall`: stops private background services and removes application binaries while leaving OpenRazer, PipeWire, and personal profiles untouched.

---

## Distribution Support Matrix & CI Validation

The [installer catalog](packaging/targets.json) recognizes the following **mutable x86_64** systems with **systemd** and **Python 3.10 or newer**. These are build targets, not a hardware compatibility guarantee.

| Distribution | Bundle target ID | Package format |
| :--- | :--- | :--- |
| Ubuntu LTS 22.04 / 24.04 / 26.04 | `ubuntu-22.04`, `ubuntu-24.04`, `ubuntu-26.04` respectively | `.deb` |
| Pop!_OS 24.04 with `UBUNTU_CODENAME=noble` | `ubuntu-24.04` (explicit compatibility rule) | `.deb` |
| Debian 12 / 13 | `debian-12`, `debian-13` respectively | `.deb` |
| Fedora 44 | `fedora-44` | `.rpm` |
| Arch Linux (rolling) | `arch` | `.pkg.tar.zst` |
| CachyOS (rolling) | `cachyos` (not the Arch bundle) | `.pkg.tar.zst` |
| openSUSE Leap 16.0 / Tumbleweed | `opensuse-leap-16.0`, `opensuse-tumbleweed` respectively | `.rpm` |

RHEL, Manjaro and unlisted distro versions are not accepted just because they use `dnf`, `pacman` or `apt`. ARM64, immutable systems (such as Fedora Silverblue) and systems without systemd are outside the current packaging targets.

The [packaging workflow](.github/workflows/linux-packaging.yml) builds each native target in its own distribution image. A passing rootless Podman smoke test verifies package installation, same-version upgrade, repair, removal and service startup. An additional [QEMU harness](packaging/ci/vm.py) is specific to Ubuntu 24.04. Neither a target entry nor a passing package test proves graphical login, DKMS operation on your kernel, physical mouse input or audible headset output.

---

## Local Development & Building

To run the project in development mode or build from source:

### Prerequisites
- **Node.js**: use Node 24 LTS or a version allowed by [`package.json`](package.json).
- **Rust & Cargo**: Rust 1.90 or newer for `x86_64-unknown-linux-gnu` (the packaging SDK uses Rust 1.91.0).
- **System development libraries (Ubuntu/Debian)**:
  ```bash
  sudo apt install build-essential pkg-config libwebkit2gtk-4.1-dev libgtk-3-dev \
                    libayatana-appindicator3-dev librsvg2-dev libssl-dev libmysofa-dev \
                    libpipewire-0.3-dev libspa-0.2-dev
  ```

### Install Node Dependencies
```bash
npm ci
```

### Run in Development Mode (Tauri Dev)
This command builds the native Rust helper binaries (`audio-worker`, `remapping-agent`) and launches the desktop app with hot reload:
```bash
npm run tauri -- dev
```

To run just the frontend with simulated devices and no hardware access:
```bash
npm run demo
```

### Running Test Suites
```bash
# Frontend unit and regression tests (Vitest)
npm test

# Packaging and installer unit tests
python3 -m unittest discover -s packaging/tests -v

# Rust backend tests
cargo test --manifest-path src-tauri/Cargo.toml --locked
```

### Building Native Distribution Packages
From the repository root, use a normal user account with rootless Podman, systemd cgroup v2 and a delegated user scope. This command builds **one** target and runs its native package checks:
```bash
# Substitute another target ID from the matrix to build that distribution instead.
python3 -B packaging/ci/matrix.py --engine podman --target ubuntu-24.04 --output packaging/out/
```

---

## Troubleshooting

### 1. Razer mouse is not detected or DPI changes revert
- Verify that the OpenRazer daemon is running:
  ```bash
  systemctl --user status openrazer-daemon
  ```
- Ensure your user belongs to the `plugdev` group:
  ```bash
  groups | grep plugdev
  ```
- Unplug and reconnect the 2.4 GHz wireless USB dongle or cable.

### 2. Remapped buttons stop responding
- Launch the **"Restore Mouse"** shortcut from your desktop application launcher, or run in a terminal:
  ```bash
  peripheral-hub-remapping-agent --disable-all
  ```
- Verify the remapper service status:
  ```bash
  systemctl status peripheral-hub-input-remapper.service
  ```

### 3. The virtual surround output is missing or silent
- Check that PipeWire and WirePlumber are active in your user session:
  ```bash
  systemctl --user status pipewire wireplumber
  ```
- With surround **off**, select your headphones in **Audio → Output**, press **Apply output** if available, and play a stereo track. Then enable 7.1 and check **Physical target**: it must name the headphones, not HDMI or the speakers. Merely selecting a device in the menu does not apply it.
- Check that the playing application uses the virtual output in the system mixer. To inspect the default nodes and active links without changing the routing:
   ```bash
   wpctl status
   pw-link -l
   ```
