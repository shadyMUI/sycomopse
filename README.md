# Peripheral Hub (sycomopse)

**Peripheral Hub 0.1.0** is a modern, open-source, modular Linux peripheral management suite built with **Tauri 2** and **React 19**. It is designed for centralized configuration of gaming peripherals (mice, keyboards, headsets), advanced spatial audio virtualization, and low-level software/hardware input remapping.

It provides native integration with key Linux subsystems:
- **OpenRazer**: advanced support for Razer mice (including 2.4 GHz wireless devices such as the Razer Viper V3 HyperSpeed), multi-stage DPI management, polling rate control (125 / 500 / 1000 Hz), battery percentage, and charging state reporting.
- **libratbag / ratbagd**: complete support for Logitech, SteelSeries, Roccat, and other compatible mice via D-Bus.
- **HeadsetControl (4.2.0)**: status monitoring and feature configuration for supported gaming headsets (battery level, sidetone, chatmix, LED controls).
- **PipeWire / HRTF (libmysofa)**: 7.1 virtual surround sound, binaural HRTF profiles, and real-time audio convolution through a dedicated headless worker.
- **Input Remapping Engine**: flexible evdev/uinput-level remapping for physical buttons, thumb/side buttons, key bindings, and macros via an isolated service.
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
- **Real-Time DPI Control**: select and switch DPI stages bidirectionally synchronized with the physical buttons on the mouse.
- **Wireless HyperSpeed Reliability**: robust reading via `getDPIStages` (opcode `0x86`) that prevents USB timeout failures on 2.4 GHz wireless dongles, avoiding unintended resets back to fallback default values (e.g. 1800 DPI).
- **Polling Rate Switching**: configure polling rates to 125 Hz, 500 Hz, or 1000 Hz with persistent configuration deduplication in `~/.config/openrazer/persistence.conf`.
- **Battery Status & Monitoring**: live remaining percentage and charging status displayed directly in the dashboard and desktop system tray.

### ⌨️ Button Remapping & Macros
- Remap physical mouse buttons (e.g., thumb buttons 4 & 5) to keyboard keys, function keys, or custom sequences.
- Low-level event injection via a virtual `uinput` device.
- Instant fallback and fail-safe recovery via the non-root desktop entry **"Restore Mouse"** or CLI command.

### 🎧 Spatial Audio & Headset Controls
- **HRTF Virtual Surround**: PipeWire filter-chain integration using SOFA convolution (via `libmysofa`) for precise 3D positional audio.
- **Headless Audio Worker**: lightweight background helper process (`peripheral-hub-audio-worker`) decoupled from the GUI lifecycle.
- **HeadsetControl Integration**: battery telemetry and sidetone adjustments for supported headsets without proprietary vendor software.

---

## Native Package Installation

Pre-built and verified native packages for all supported Linux distributions are available under:
```
packaging/out/
```

> [!IMPORTANT]
> Always install the **specific native package** built for your distribution and version. Do not convert packages across distributions using alien or third-party wrappers. Each package includes the GUI, user session agent, private remapper engine, audio worker, verified HeadsetControl CLI, systemd units, and desktop integration.

### Distribution Installation Commands (Run from Repository Root)

#### Ubuntu 24.04 LTS (Noble Numbat) / Pop!_OS 24.04
```bash
sudo apt install ./packaging/out/peripheral-hub-0.1.0-ubuntu-24.04-x86_64/peripheral-hub_0.1.0-1_amd64.deb
```

#### Ubuntu 22.04 LTS (Jammy Jellyfish)
```bash
sudo apt install ./packaging/out/peripheral-hub-0.1.0-ubuntu-22.04-x86_64/peripheral-hub_0.1.0-1_amd64.deb
```

#### Fedora 44 / Red Hat Enterprise Linux
```bash
sudo dnf install ./packaging/out/peripheral-hub-0.1.0-fedora-44-x86_64/peripheral-hub-0.1.0-1.x86_64.rpm
```

#### Arch Linux / Manjaro / CachyOS
```bash
sudo pacman -U ./packaging/out/peripheral-hub-0.1.0-arch-x86_64/peripheral-hub-0.1.0-1-x86_64.pkg.tar.zst
```

#### openSUSE (Leap 16.0 / Tumbleweed)
```bash
sudo zypper install ./peripheral-hub-0.1.0-1.x86_64.rpm
```

### Automatic Dependency Resolution
Your native package manager (`apt`, `pacman`, `dnf`, `zypper`) will automatically resolve all required dependencies:
- **GUI & Runtime**: WebKitGTK 4.1 (`libwebkit2gtk-4.1`), GTK3, AppIndicator3, Python >= 3.10, Cairo, PyGObject, Pydantic.
- **Hardware & Audio**: PipeWire, WirePlumber, libmysofa, UPower, libratbag/ratbagd, OpenRazer daemon/DKMS.

---

## Architecture & Security Model

The application enforces strict privilege separation across dedicated processes:

```
┌─────────────────────────────────────────────────────────────┐
│                    Peripheral Hub GUI                       │
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

Peripheral Hub never silently changes audio backends or kernel drivers:
- **No Unsolicited PulseAudio Replacement**: switching audio subsystems to PipeWire/WirePlumber requires explicit user consent.
- **ratbagd**: device probing and daemon activation require explicit user opt-in.
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

Every distribution bundle folder under `packaging/out/peripheral-hub-0.1.0-<target>-x86_64/` contains an actionable maintenance script:

```bash
cd packaging/out/peripheral-hub-0.1.0-ubuntu-24.04-x86_64/

# Verify bundle integrity and check service status
sh install.sh status

# Repair missing or modified files, restore permissions, and reload units
sh install.sh repair

# Clean uninstallation (preserves user configuration and profiles)
sh install.sh uninstall
```

- `repair`: audits all files against `manifest.json` SHA-256 checksums, reinstalls corrupted or deleted components, and re-validates backend connectivity.
- `uninstall`: stops private background services and removes application binaries while leaving OpenRazer, PipeWire, and personal profiles untouched.

---

## Distribution Support Matrix & CI Validation

The packaging pipeline and continuous integration suites certify the following **x86_64** targets running **systemd**:

| Distribution | Supported Versions | Package Format |
| :--- | :--- | :--- |
| **Ubuntu LTS** | 22.04 (Jammy), 24.04 (Noble), 26.04 (Resolute) | `.deb` |
| **Pop!_OS** | 24.04 (Noble host compatibility) | `.deb` |
| **Debian** | 12 (Bookworm), 13 (Trixie) | `.deb` |
| **Fedora** | 44 (current target) | `.rpm` |
| **openSUSE** | Leap 16.0, Tumbleweed (rolling) | `.rpm` |
| **Arch Linux** | Arch rolling, CachyOS (x86-64 generic) | `.pkg.tar.zst` |

> Non-systemd init systems (SysVinit, OpenRC, runit), immutable distributions, NixOS, Alpine, or Void Linux are not officially supported.

### Testing & Validation Environments
- **Rootless Podman**: full containerized sandboxing using `keep-id:uid=1000,gid=1000`, user scope with `Delegate=yes`, cgroup v2, and isolated namespaces.
- **Disposable QEMU VM**: On Ubuntu 24.04, install, upgrade, repair, removal, and autostart hooks are thoroughly validated inside an ephemeral cloud-image VM verified with official GPG keys.

---

## Local Development & Building

To run the project in development mode or build from source:

### Prerequisites
- **Node.js**: version `>= 22.12.0` (or Node 24 LTS)
- **Rust & Cargo**: version `>= 1.80` (with `x86_64-unknown-linux-gnu` target)
- **System development libraries (Ubuntu/Debian)**:
  ```bash
  sudo apt install build-essential pkg-config libwebkit2gtk-4.1-dev libgtk-3-dev \
                   libayatana-appindicator3-dev libratbag-dev libmysofa-dev libpipewire-0.3-dev
  ```

### Install Node Dependencies
```bash
npm install
```

### Run in Development Mode (Tauri Dev)
This command builds the native Rust helper binaries (`audio-worker`, `remapping-agent`) and launches the desktop app with hot reload:
```bash
npm run tauri dev
```

To run the Vite frontend preview server independently:
```bash
npm run dev
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
To build distribution packages using the container packaging infrastructure:
```bash
# Build for Ubuntu 24.04
python3 -B packaging/build.py --target ubuntu-24.04 --output packaging/out/

# Run the complete CI validation matrix
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

### 3. Audio spatialization or HRTF profiles are unavailable
- Check that PipeWire and WirePlumber are active in your user session:
  ```bash
  systemctl --user status pipewire wireplumber
  ```
- Inspect audio worker logs:
  ```bash
  journalctl --user -u peripheral-hub-audio-worker -e
  ```
