# Sycomopse

**Sycomopse 0.1.0** brings mouse settings, button remapping and headset audio into one Linux desktop app built with **Tauri 2** and **React 19**. It is an independent project inspired by the all-in-one approach of Razer Synapse, not an official Razer client. You can use the Mouse or Audio panel on its own; both ship in the same suite.

The Linux package and command names still use `peripheral-hub`. The rename does not change service names or profile directories; existing profiles need no migration.

It provides native integration with key Linux subsystems:
- **OpenRazer and libratbag / ratbagd**: mouse DPI and polling settings where the device and its driver expose verified controls.
- **HeadsetControl (4.2.0)**: read-only battery reporting for supported Logitech headsets that the app can identify. Sycomopse does not currently set sidetone, chat/game mix or headset LEDs.
- **PipeWire / WirePlumber**: system audio routing, output volume, microphone gain/mute, a ten-band headphone equalizer, and optional virtual 7.1 playback using a SOFA HRTF.
- **Input Remapping Engine**: mouse buttons, key bindings, keyboard shortcuts and hold-to-repeat clicks via an isolated evdev/uinput service.
- **UPower**: unified power and battery level monitoring for standard wireless and Bluetooth peripherals.

The user interface (GUI) and background session agents are licensed under the **MIT License**. The isolated Input Remapper engine is licensed under **GPL-3.0-or-later**. Native packages include the engine's license, attribution and corresponding source; the complete bundle also includes an application source archive under `sources/`.

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
- Quick recovery via the non-root desktop entry **Restore mouse mappings** or its CLI command.

### 🎧 Spatial Audio & Headset Controls
- Choose a physical playback output and adjust output volume or microphone gain/mute. These controls belong to the Linux audio session, not the headset firmware.
- Select your physical headphones under **Audio → Output** before enabling EQ or surround. Click **Apply output** if it is enabled; the button is disabled when the target is already confirmed. If another app changes the default, Sycomopse requires you to confirm the target again rather than silently sending audio to a different device.
- Enable virtual 7.1 to render multichannel audio as binaural stereo using PipeWire and `libmysofa`. The virtual output receives the application's audio; the physical headphones play it. Apps pinned to another sink are not forcibly moved.

### Headphone equalizer
1. In **Audio → Headphone equalizer**, start with **Neutral**, **Voice** or **Gaming**, or adjust the ten bands from **31.5 Hz to 16 kHz** (±12 dB). Choose **Apply EQ** to activate it. Once active, gains, presets, preamp and **Advanced: per-band Q** update the existing filter while you listen, without creating another audio output. A lower Q shapes a broader range of frequencies.
2. **Bypass EQ** compares playback with the saved curve removed. It does not delete your settings or disable active surround. Stereo EQ needs no SOFA file; with 7.1 enabled the same curve processes the two ear signals *after* spatialization. Neither mode processes the microphone.
3. Settings are remembered per physical PipeWire output. **Audio profiles → New profile → Include equalizer in profile** captures the confirmed bands, preamp and bypass state in a named preset. Existing profiles without an EQ use the target output's saved curve. Startup or plugging in another headset does not silently reapply processing.

**Preamp mode:** **Auto** reserves headroom by subtracting the sum of positive band gains before filtering. **Manual** starts at **0 dB** and lets you set **−24 to 0 dB** without automatic compensation. With Manual at 0 dB, boosts can clip on loud material; lower the preamp if you hear distortion. Existing curves with no preamp setting retain **Auto**.

---

## Native Package Installation

### 1. Choose and download the bundle

Sycomopse is one Linux desktop app. The complete bundle installs the GUI and its companion services; there is no separate download for the Mouse or Audio panel. You need an `x86_64` system with systemd, Python 3.10 or newer, and a supported distribution version. To check your machine:

```sh
cat /etc/os-release
uname -m
ps -p 1 -o comm=
python3 --version
```

Choose the exact **target ID** from the [distribution matrix](#distribution-support-matrix--ci-validation). A package built for another distro or version is not interchangeable just because it uses the same format. **Pop!_OS 24.04 with `UBUNTU_CODENAME=noble` uses the `ubuntu-24.04` bundle.**

- **[GitHub Releases](https://github.com/shadyMUI/sycomopse/releases):** Under **Assets**, download the matching full `.tar.gz` and its `.tar.gz.sha256` into the same directory. Keep both filenames unchanged. Neither the automatic **Source code** archives nor the standalone packages in the original `v0.1.0` release are the complete installer bundle.
- **[GitHub Actions](https://github.com/shadyMUI/sycomopse/actions/workflows/linux-packaging.yml):** If Releases has no full bundle for your target, sign in to GitHub, open a **Linux Native Packaging** run where your **target job** passed, and download `peripheral-hub-<version>-<target>-x86_64` from **Summary → Artifacts**. GitHub downloads it as a ZIP. Your target's artifact can be available even when the overall run failed because another distro did not pass. Artifacts expire after 14 days.

If neither location has a verified bundle, there is no ready-to-install package for that target. The [local build instructions](#building-native-distribution-packages) are for developers; cloning the repository does not download native packages.

**Debian 13 only:** The bundle requires PipeWire **1.4.10 or newer**. Debian 13's original 1.4.2 can accept a mute change without publishing the new state, so Sycomopse cannot confirm the change. If your PipeWire is older, follow the [official Debian Backports instructions](https://backports.debian.org/Instructions/) to enable `trixie-backports` manually, then run:

```sh
sudo apt update &&
sudo apt install --no-remove -t trixie-backports pipewire pipewire-bin
```

If APT reports a version conflict, include the already-installed PipeWire companion packages it names (for example, `pipewire-pulse`, `pipewire-alsa` or `libspa-0.2-bluetooth`) in the same transaction and keep `--no-remove`. Log out and back in after upgrading to restart the audio session. Sycomopse's installer does not enable Debian Backports for you.

### 2. Verify and extract

Open a terminal in your download directory. Use just the block for the type of file you downloaded, and replace the first line with its actual filename.

**From Releases (`.tar.gz`):** verify the SHA-256 before extracting.

```sh
archive='REPLACE_WITH_DOWNLOADED_TAR_GZ_FILENAME'
sha256sum -c "${archive}.sha256" &&
unpack_dir=$(mktemp -d ./sycomopse.XXXXXX) &&
tar -xzf "$archive" -C "$unpack_dir" &&
cd "$unpack_dir"/peripheral-hub-*
```

**From Actions (`.zip`):** the installer will verify the internal manifest and file hashes in the next step.

```sh
archive='REPLACE_WITH_DOWNLOADED_ZIP_FILENAME'
unpack_dir=$(mktemp -d ./sycomopse.XXXXXX) &&
unzip -q "$archive" -d "$unpack_dir" &&
cd "$unpack_dir"
```

### 3. Install and launch

In the extracted directory, check that you have both `install.sh` and `manifest.json`, then preview the install:

```sh
test -f ./install.sh && test -f ./manifest.json &&
sh ./install.sh install --dry-run
```

The dry run verifies the bundle and target and shows the plan without changing the system. If any command fails or the target is wrong, stop. If the plan looks right, run these commands in the **same directory**:

```sh
sh ./install.sh install && sh ./install.sh status
```

Run the installer as your normal desktop user, **not** with `sudo`. It asks for administrative permissions only for the system phase and uses your distro's native package manager. Do not install the inner `.deb`, `.rpm` or `.pkg.tar.zst` on its own. After installation, open **Sycomopse** from the app menu or run `peripheral-hub` from the terminal. See [Repair, Verification & Removal](#repair-verification--removal) for maintenance commands.

The `--without-audio` and `--without-openrazer` options affect the wrapper's setup steps; they do **not** remove the native package's declared dependencies. In particular, the Ubuntu, Debian, Arch and CachyOS packages currently require OpenRazer/DKMS even if you only use Audio.

---

## Architecture & Security Model

The GUI, audio worker and remapping agent run as the desktop user. The GUI communicates with the audio worker through JSON over standard input and output, and starts the remapping agent through `systemctl --user`. Both the GUI and the agent call the separate `io.peripheralhub.InputRemapper` service on the system D-Bus.

- **Non-Invasive System Integration**: Native hooks never overwrite external configuration files, do not pipe unverified remote scripts to bash, and respect administrative system policies (`policy-rc.d`, systemd masks).
- **License Compliance**: The GPL Input Remapper engine runs as a separate system process and is not statically linked into the MIT-licensed app. The bundle includes corresponding source archives under `sources/`.

---

## Remapping Agent & Session Recovery

The package installs:
- **GUI**: `/usr/bin/peripheral-hub`
- **Session Agent**: `/usr/bin/peripheral-hub-remapping-agent` (compiled with Cargo `--no-default-features`)
- **Audio Worker**: `/usr/bin/peripheral-hub-audio-worker`
- **Systemd User Unit**: `peripheral-hub-remapping-agent.service` (auto-started via `/etc/xdg/autostart/io.peripheralhub.RemappingAgent.desktop`)
- **Private Remapper Engine**: `/usr/lib/peripheral-hub/remapper/` and wrapper `/usr/lib/peripheral-hub/remapper-service`
- **Systemd System Unit**: `peripheral-hub-input-remapper.service`
- **Desktop Recovery Shortcut**: **Restore mouse mappings** (`io.peripheralhub.Recovery.desktop`) to release active remaps from the app menu.

### Quick Recovery Commands
If button mappings become unresponsive, the recovery shortcut or command releases Sycomopse's remaps and turns off automatic reapplication on success. It is not just a temporary pause.

```bash
# Release active remaps and disable automatic reapplication
peripheral-hub-remapping-agent --disable-all

# Inspect the running session agent
systemctl --user status peripheral-hub-remapping-agent.service
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
- **`plugdev` group and udev permissions**: When you authorize OpenRazer setup, the installer can add your desktop user to `plugdev`. If it reports that the group is not yet effective in your session, log out and back in. Do not launch Sycomopse as root to work around device permissions.

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

The [installer catalog](packaging/targets.json) recognizes the following **mutable x86_64** systems with **systemd** and **Python 3.10 or newer**. A catalog entry is a build target, not a promise that a bundle is available. The statuses below refer to the [packaging run for `3411316`](https://github.com/shadyMUI/sycomopse/actions/runs/46033175195).

| Distribution | Bundle target ID | Package format | Native package CI |
| :--- | :--- | :--- | :--- |
| Ubuntu 22.04 LTS | `ubuntu-22.04` | `.deb` | Passed |
| Ubuntu 24.04 LTS / Pop!_OS 24.04 (Noble) | `ubuntu-24.04` (shared bundle) | `.deb` | Passed on Ubuntu 24.04 |
| Ubuntu 26.04 LTS | `ubuntu-26.04` | `.deb` | Passed |
| Debian 12 | `debian-12` | `.deb` | Passed |
| Debian 13 | `debian-13` | `.deb` | Passed with PipeWire 1.6.9; requires 1.4.10+ |
| Fedora 44 | `fedora-44` | `.rpm` | Passed |
| Arch Linux (rolling) | `arch` | `.pkg.tar.zst` | Passed |
| CachyOS (rolling) | `cachyos` (not the Arch bundle) | `.pkg.tar.zst` | Passed |
| openSUSE Leap 16.0 | `opensuse-leap-16.0` | `.rpm` | Failed; no verified bundle |
| openSUSE Tumbleweed | `opensuse-tumbleweed` | `.rpm` | Failed; no verified bundle |

RHEL, Manjaro and unlisted distro versions are not accepted just because they use `dnf`, `pacman` or `apt`. ARM64, immutable systems (such as Fedora Silverblue) and systems without systemd are outside the current packaging targets.

The [packaging workflow](.github/workflows/linux-packaging.yml) builds each native target in its own distribution image. A passing rootless Podman smoke test verifies package installation, same-version upgrade, repair, removal and service startup. An additional [QEMU harness](packaging/ci/vm.py) is specific to Ubuntu 24.04. Neither a target entry nor a passing package test proves graphical login, DKMS operation on your kernel, physical mouse input or audible headset output.

---

## Local Development & Building

To run the project in development mode or build from source:

### Prerequisites
- **Node.js**: use Node 24 LTS or a version allowed by [`package.json`](package.json).
- **Python**: Python 3.10 or newer for staging the native helpers.
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

# Rust backend tests from a clean checkout: prepare Tauri assets first
npm run build &&
npm run native:helpers &&
python3 -B packaging/stage_sidecars.py debug &&
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
- Launch **Restore mouse mappings** from your desktop application launcher, or run in a terminal:
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
