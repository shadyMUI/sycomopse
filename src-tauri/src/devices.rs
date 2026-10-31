use crate::models::*;
use crate::providers::{identities_match, unique_identity_match, valid_serial};

#[derive(Debug, Clone)]
pub struct UsbMetadata {
    pub identity: DeviceIdentity,
    pub name: String,
    pub is_audio: bool,
    pub bus_alias: Option<String>,
    pub instance: String,
}

pub fn usb_device(usb: &UsbMetadata) -> Option<Device> {
    let brand = match usb.identity.vendor_id? {
        0x1532 => Brand::Razer,
        0x046d => Brand::Logitech,
        _ => return None,
    };
    let product = usb.identity.product_id?;
    let mut limitations =
        vec!["Discovery USB di soli metadata; nessun pacchetto HID inviato.".to_owned()];
    let (kind, fallback_name) = match (brand, product) {
        (Brand::Razer, 0x00b8) => {
            limitations.push("DPI, polling e batteria richiedono OpenRazer gia attivo.".to_owned());
            (DeviceKind::Mouse, "Razer Viper V3 HyperSpeed")
        }
        (Brand::Razer, 0x057c | 0x057d) => {
            limitations.push("ID BlackShark V3 X HyperSpeed proposto nella PR OpenRazer #2899 non integrata; modello e trasporto da verificare.".to_owned());
            limitations.push("OpenRazer stock non supporta questa cuffia. Solo audio PipeWire; batteria e controlli onboard non disponibili.".to_owned());
            if product == 0x057d {
                limitations.push(
                    "ID del dongle audio, non prova dell'identita/firmware della cuffia associata."
                        .to_owned(),
                );
            }
            (
                DeviceKind::Headset,
                "Audio Razer, possibile BlackShark V3 X HyperSpeed",
            )
        }
        (Brand::Razer, 0x00b3) => {
            limitations.push("HyperPolling receiver separato: non identifica il mouse associato e non prova il supporto 8000 Hz.".to_owned());
            (DeviceKind::Receiver, "Razer HyperPolling receiver")
        }
        (Brand::Logitech, 0xc547) => {
            limitations.push("Receiver SUPERLIGHT: non e il mouse HID++ 4093, ne prova il supporto SUPERLIGHT 2.".to_owned());
            (DeviceKind::Receiver, "Logitech SUPERLIGHT receiver")
        }
        (Brand::Logitech, 0xc08b | 0xc094) => (DeviceKind::Mouse, "Mouse Logitech USB"),
        (_, _) if usb.is_audio => {
            limitations.push("Audio standard disponibile dove esposto da PipeWire; controlli proprietari non presunti.".to_owned());
            (DeviceKind::Headset, "Audio USB")
        }
        _ => (DeviceKind::Unknown, "Periferica USB"),
    };
    let name = if usb.name.trim().is_empty() {
        fallback_name.to_owned()
    } else {
        usb.name.clone()
    };
    Some(Device {
        id: format!("usb:{}", usb.identity.physical_path.as_deref()?),
        name,
        brand,
        kind,
        provider: ProviderKind::Usb,
        identity: usb.identity.clone(),
        availability: AvailabilityCode::Available,
        support: if kind == DeviceKind::Headset {
            SupportLevel::Partial
        } else {
            SupportLevel::Unsupported
        },
        capabilities: DeviceCapabilities::default(),
        values: DeviceValues::default(),
        output_node_names: Vec::new(),
        limitations,
    })
}

pub fn usb_physical_path(value: &str) -> Option<String> {
    let value = value.strip_prefix("/sys").unwrap_or(value);
    if !value.starts_with("/devices/")
        || value.len() > 4096
        || value.contains('\\')
        || value.chars().any(char::is_control)
    {
        return None;
    }
    let mut components = Vec::new();
    let mut usb_root = None;
    for component in value.split('/').skip(1) {
        if component.is_empty() || matches!(component, "." | "..") {
            return None;
        }
        components.push(component);
        if let Some((bus, ports)) = component.split_once('-') {
            if !bus.is_empty()
                && bus.bytes().all(|b| b.is_ascii_digit())
                && !ports.is_empty()
                && ports
                    .split('.')
                    .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
            {
                usb_root = Some(format!("/sys/{}", components.join("/")));
            }
        }
    }
    usb_root
}

pub fn usb_bus_alias(physical_path: &str) -> Option<String> {
    let root = usb_physical_path(physical_path)?;
    let sysname = root.rsplit('/').next()?;
    let (_, ports) = sysname.split_once('-')?;
    let pci = root.split('/').rev().find(|part| {
        part.len() == 12
            && part.as_bytes()[4] == b':'
            && part.as_bytes()[7] == b':'
            && part.as_bytes()[10] == b'.'
            && part
                .bytes()
                .enumerate()
                .all(|(i, b)| matches!(i, 4 | 7 | 10) || b.is_ascii_hexdigit())
    })?;
    Some(format!("pci-{pci}-usb-0:{ports}"))
}

pub fn verified_usb_path(value: &str, devices: &[UsbMetadata]) -> Option<usize> {
    let physical = usb_physical_path(value);
    let mut matches = devices.iter().enumerate().filter(|(_, usb)| {
        if physical
            .as_deref()
            .is_some_and(|path| Some(path) == usb.identity.physical_path.as_deref())
        {
            return true;
        }
        usb.bus_alias.as_deref().is_some_and(|alias| {
            value == alias
                || value
                    .strip_prefix(alias)
                    .and_then(|suffix| suffix.strip_prefix(':'))
                    .is_some_and(|suffix| {
                        let Some((configuration, interface)) = suffix.split_once('.') else {
                            return false;
                        };
                        !configuration.is_empty()
                            && configuration.bytes().all(|b| b.is_ascii_digit())
                            && !interface.is_empty()
                            && interface.bytes().all(|b| b.is_ascii_digit())
                    })
        })
    });
    let (index, _) = matches.next()?;
    matches.next().is_none().then_some(index)
}

pub fn merge_usb(discovery: &mut Discovery, usb: &[UsbMetadata]) {
    for metadata in usb {
        let Some(mut device) = usb_device(metadata) else {
            continue;
        };
        let candidates: Vec<_> = discovery
            .devices
            .iter()
            .filter(|candidate| {
                candidate.brand == device.brand
                    && (candidate.kind == device.kind
                        || candidate.kind == DeviceKind::Unknown
                        || device.kind == DeviceKind::Unknown)
                    && !(candidate.kind == DeviceKind::Receiver && device.kind == DeviceKind::Mouse)
            })
            .map(usb_match_identity)
            .collect();
        if unique_identity_match(&device.identity, &candidates).is_some() {
            let index = discovery.devices.iter().position(|candidate| {
                candidate.brand == device.brand
                    && identities_match(&usb_match_identity(candidate), &device.identity)
                    && (candidate.kind == device.kind
                        || candidate.kind == DeviceKind::Unknown
                        || device.kind == DeviceKind::Unknown)
            });
            if let Some(index) = index {
                let usb_identities: Vec<_> = usb.iter().map(|usb| usb.identity.clone()).collect();
                if unique_identity_match(
                    &usb_match_identity(&discovery.devices[index]),
                    &usb_identities,
                )
                .is_none()
                {
                    discovery.devices.push(device);
                    continue;
                }
                let candidate = &mut discovery.devices[index];
                if candidate.identity.physical_path.is_none() {
                    candidate.identity.physical_path = device.identity.physical_path.take();
                }
                if candidate.identity.serial.is_none() {
                    candidate.identity.serial = device.identity.serial.take();
                }
                if candidate.identity.transport.is_none() {
                    candidate.identity.transport = device.identity.transport.take();
                }
                for limitation in device.limitations {
                    if !candidate.limitations.contains(&limitation) {
                        candidate.limitations.push(limitation);
                    }
                }
                continue;
            }
        }
        discovery.devices.push(device);
    }
}

fn usb_match_identity(device: &Device) -> DeviceIdentity {
    let mut identity = device.identity.clone();
    if device.provider == ProviderKind::Ratbag && device.kind == DeviceKind::Mouse {
        // Collapse a verified HID child only for USB metadata association. Matching
        // VID/PID still prevents a logical HID++ mouse being merged with its receiver.
        if let Some(path) = identity
            .physical_path
            .as_deref()
            .and_then(usb_physical_path)
        {
            identity.physical_path = Some(path);
        }
    }
    identity
}

pub fn associate_audio(discovery: &mut Discovery, audio_nodes: &[AudioNode], usb: &[UsbMetadata]) {
    for node in audio_nodes
        .iter()
        .filter(|node| !node.is_virtual && node.media_class == "Audio/Sink")
    {
        let mut identity = DeviceIdentity {
            serial: node.device_serial.as_deref().and_then(valid_serial),
            vendor_id: node.vendor_id,
            product_id: node.product_id,
            ..DeviceIdentity::default()
        };
        if let Some(index) = node
            .device_bus_path
            .as_deref()
            .and_then(|path| verified_usb_path(path, usb))
        {
            let physical = &usb[index].identity;
            if !identity
                .vendor_id
                .zip(physical.vendor_id)
                .is_some_and(|(a, b)| a != b)
                && !identity
                    .product_id
                    .zip(physical.product_id)
                    .is_some_and(|(a, b)| a != b)
                && !identity
                    .serial
                    .as_deref()
                    .zip(physical.serial.as_deref())
                    .is_some_and(|(a, b)| a != b)
            {
                identity.physical_path = physical.physical_path.clone();
                if identity.serial.is_none() {
                    identity.serial = physical.serial.clone();
                }
            }
        }
        let indices: Vec<_> = discovery
            .devices
            .iter()
            .enumerate()
            .filter(|(_, device)| {
                device.kind == DeviceKind::Headset
                    && device.brand != Brand::Generic
                    && identities_match(&identity, &device.identity)
            })
            .map(|(index, _)| index)
            .collect();
        if indices.len() == 1 {
            discovery.devices[indices[0]]
                .output_node_names
                .push(node.node_name.clone());
        } else {
            discovery.devices.push(Device {
                id: format!("pipewire:{}", node.node_name), name: node.description.clone(), brand: Brand::Generic,
                kind: DeviceKind::Headset, provider: ProviderKind::Pipewire,
                identity: DeviceIdentity { transport: node.transport.clone(), ..identity },
                availability: AvailabilityCode::Available, support: SupportLevel::Partial,
                capabilities: DeviceCapabilities::default(), values: DeviceValues::default(),
                output_node_names: vec![node.node_name.clone()],
                limitations: vec!["Uscita audio PipeWire; identita vendor non associata con certezza. Nessuna batteria o funzione onboard presunta.".to_owned()],
            });
        }
    }
}

pub fn associate_headset_batteries(
    discovery: &mut Discovery,
    usb: &[UsbMetadata],
    report: &crate::providers::headsetcontrol::Report,
) {
    for reading in &report.devices {
        let json_count = report
            .devices
            .iter()
            .filter(|r| r.vendor_id == reading.vendor_id && r.product_id == reading.product_id)
            .count();
        let matching_usb: Vec<_> = usb
            .iter()
            .filter(|usb| {
                usb.identity.vendor_id == Some(reading.vendor_id)
                    && usb.identity.product_id == Some(reading.product_id)
            })
            .collect();
        if json_count != 1 || matching_usb.len() != 1 {
            discovery.diagnostics.push(Diagnostic::new(
                "headsetcontrol-ambiguous",
                ProviderKind::Headsetcontrol,
                AvailabilityCode::Unknown,
                Severity::Warning,
                "Batteria cuffie non associata: JSON senza seriale o piu istanze USB identiche.",
            ));
            continue;
        }
        let metadata = matching_usb[0];
        let candidates: Vec<_> = discovery
            .devices
            .iter()
            .enumerate()
            .filter(|(_, device)| {
                device.brand == Brand::Logitech
                    && device.kind == DeviceKind::Headset
                    && identities_match(&metadata.identity, &device.identity)
            })
            .map(|(index, _)| index)
            .collect();
        if candidates.len() == 1 {
            let device = &mut discovery.devices[candidates[0]];
            device.provider = ProviderKind::Headsetcontrol;
            device.values.battery = Some(reading.battery.clone());
            device.support = SupportLevel::Partial;
            device
                .limitations
                .push("HeadsetControl usato solo per -b -o json; nessun setter vendor.".to_owned());
        }
    }
}

pub async fn discover(audio_nodes: &[AudioNode], include_batteries: bool) -> Discovery {
    #[cfg(target_os = "linux")]
    {
        linux::discover(audio_nodes, include_batteries).await
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (audio_nodes, include_batteries);
        Discovery {
            devices: Vec::new(),
            diagnostics: vec![Diagnostic::new(
                "hardware-platform",
                ProviderKind::App,
                AvailabilityCode::UnsupportedPlatform,
                Severity::Info,
                AppError::unsupported_platform().message,
            )],
        }
    }
}

pub async fn set_dpi(device: &Device, x: u32, y: u32) -> AppResult<()> {
    #[cfg(target_os = "linux")]
    {
        validate_owner(device)?;
        match device.provider {
            ProviderKind::Openrazer => crate::providers::openrazer::set_dpi(device, x, y).await,
            ProviderKind::Ratbag => crate::providers::ratbag::set_dpi(device, x, y).await,
            _ => Err(AppError::new(
                ErrorCode::Unsupported,
                "Nessun proprietario dei controlli DPI.",
            )),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (device, x, y);
        Err(AppError::unsupported_platform())
    }
}

pub async fn set_polling_rate(device: &Device, rate: u32) -> AppResult<()> {
    #[cfg(target_os = "linux")]
    {
        validate_owner(device)?;
        match device.provider {
            ProviderKind::Openrazer => {
                crate::providers::openrazer::set_polling_rate(device, rate).await
            }
            ProviderKind::Ratbag => crate::providers::ratbag::set_polling_rate(device, rate).await,
            _ => Err(AppError::new(
                ErrorCode::Unsupported,
                "Nessun proprietario del polling hardware.",
            )),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (device, rate);
        Err(AppError::unsupported_platform())
    }
}

#[cfg(target_os = "linux")]
fn validate_owner(device: &Device) -> AppResult<()> {
    if device.kind != DeviceKind::Mouse
        || device.availability != AvailabilityCode::Available
        || !matches!(
            (device.brand, device.provider),
            (Brand::Razer, ProviderKind::Openrazer) | (Brand::Logitech, ProviderKind::Ratbag)
        )
    {
        return Err(AppError::new(
            ErrorCode::Unsupported,
            "Dispositivo senza proprietario hardware scrivibile.",
        ));
    }
    Ok(())
}

pub async fn watch_changes(sender: tokio::sync::mpsc::UnboundedSender<()>) {
    #[cfg(target_os = "linux")]
    {
        tokio::join!(
            linux::watch_bus(sender.clone(), true, &[("org.razer", "/org/razer")]),
            linux::watch_bus(
                sender,
                false,
                &[
                    ("org.freedesktop.ratbag1", "/org/freedesktop/ratbag1"),
                    ("org.freedesktop.UPower", "/org/freedesktop/UPower")
                ]
            )
        );
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = sender;
    }
}

#[cfg(target_os = "linux")]
pub(crate) use linux::{ratbag_identity, read_usb};

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::providers::{
        self, bounded_text, dbus, diagnostic, hex_id, invalid_data, MAX_DEVICES,
    };
    use std::{
        collections::HashSet,
        fs,
        io::Read,
        path::{Path, PathBuf},
        time::Duration,
    };
    use tokio::sync::mpsc::UnboundedSender;
    use zbus::{MatchRule, MessageStream};

    fn text(path: &Path) -> AppResult<String> {
        let mut bytes = Vec::new();
        fs::File::open(path)?.take(4097).read_to_end(&mut bytes)?;
        if bytes.len() > 4096 {
            return Err(invalid_data("Metadata sysfs troppo grandi."));
        }
        String::from_utf8(bytes).map_err(|_| invalid_data("Metadata sysfs non UTF-8."))
    }

    pub(crate) fn read_usb() -> AppResult<Vec<UsbMetadata>> {
        let root = Path::new("/sys/bus/usb/devices");
        let mut devices = Vec::new();
        let mut seen = HashSet::new();
        for (index, entry) in fs::read_dir(root)?.enumerate() {
            if index >= 2048 {
                return Err(invalid_data("Troppi oggetti USB sysfs."));
            }
            let entry = entry?;
            let path = entry.path();
            let Ok(vendor_text) = text(&path.join("idVendor")) else {
                continue;
            };
            let Some(vendor_id) = hex_id(&vendor_text) else {
                continue;
            };
            if !matches!(vendor_id, 0x1532 | 0x046d) {
                continue;
            }
            let Some(product_id) = text(&path.join("idProduct"))
                .ok()
                .as_deref()
                .and_then(hex_id)
            else {
                continue;
            };
            let canonical = fs::canonicalize(&path)?;
            let Some(physical_path) = canonical
                .to_str()
                .filter(|p| p.starts_with("/sys/devices/"))
                .map(str::to_owned)
            else {
                continue;
            };
            if !seen.insert(physical_path.clone()) {
                continue;
            }
            let sysname = entry.file_name();
            let sysname = sysname.to_string_lossy();
            let mut is_audio = false;
            for interface in fs::read_dir(&canonical)?.take(128).flatten() {
                if interface
                    .file_name()
                    .to_string_lossy()
                    .starts_with(&format!("{sysname}:"))
                    && text(&interface.path().join("bInterfaceClass"))
                        .ok()
                        .as_deref()
                        .and_then(hex_id)
                        == Some(1)
                {
                    is_audio = true;
                }
            }
            let serial = text(&path.join("serial"))
                .ok()
                .as_deref()
                .and_then(valid_serial);
            let name = text(&path.join("product"))
                .ok()
                .map(|s| bounded_text(s.trim()))
                .unwrap_or_default();
            let instance = format!(
                "{}:{}:{}",
                physical_path,
                text(&path.join("busnum")).unwrap_or_default().trim(),
                text(&path.join("devnum")).unwrap_or_default().trim()
            );
            let bus_alias = usb_bus_alias(&physical_path);
            devices.push(UsbMetadata {
                identity: DeviceIdentity {
                    serial,
                    physical_path: Some(physical_path),
                    vendor_id: Some(vendor_id),
                    product_id: Some(product_id),
                    transport: Some("usb".to_owned()),
                    ..DeviceIdentity::default()
                },
                name,
                is_audio,
                bus_alias,
                instance,
            });
            if devices.len() > MAX_DEVICES {
                return Err(invalid_data("Troppi dispositivi USB vendor."));
            }
        }
        devices.sort_by(|a, b| a.instance.cmp(&b.instance));
        Ok(devices)
    }

    pub(crate) fn ratbag_identity(
        backend_path: &str,
        vendor_id: u16,
        product_id: u16,
    ) -> DeviceIdentity {
        let mut identity = DeviceIdentity {
            vendor_id: Some(vendor_id),
            product_id: Some(product_id),
            backend_path: Some(backend_path.to_owned()),
            ..DeviceIdentity::default()
        };
        let Some(sysname) = backend_path.strip_prefix("/org/freedesktop/ratbag1/device/") else {
            return identity;
        };
        let Some(number) = sysname.strip_prefix("hidraw") else {
            return identity;
        };
        if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
            return identity;
        }
        // The sysname comes from Manager.Devices, never from an assumed hidraw index.
        let Ok(path) =
            fs::canonicalize(Path::new("/sys/class/hidraw").join(sysname).join("device"))
        else {
            return identity;
        };
        let Some(physical_path) = path.to_str().filter(|p| p.starts_with("/sys/devices/")) else {
            return identity;
        };
        let Ok(uevent) = text(&path.join("uevent")) else {
            return identity;
        };
        let Some(hid_id) = uevent.lines().find_map(|line| line.strip_prefix("HID_ID=")) else {
            return identity;
        };
        let parts: Vec<_> = hid_id.split(':').collect();
        if parts.len() != 3
            || u32::from_str_radix(parts[1], 16).ok() != Some(u32::from(vendor_id))
            || u32::from_str_radix(parts[2], 16).ok() != Some(u32::from(product_id))
        {
            return identity;
        }
        identity.serial = uevent
            .lines()
            .find_map(|line| line.strip_prefix("HID_UNIQ="))
            .and_then(valid_serial);
        identity.physical_path = Some(physical_path.to_owned());
        identity
    }

    fn native_identity_path(value: &str, usb: &[UsbMetadata], device_type: u32) -> Option<String> {
        if matches!(device_type, 17 | 19) {
            if let Some(index) = verified_usb_path(value, usb).filter(|index| usb[*index].is_audio)
            {
                return usb[index].identity.physical_path.clone();
            }
        }
        if value.len() > 4096
            || value.chars().any(char::is_control)
            || value.split('/').any(|p| matches!(p, "." | ".."))
        {
            return None;
        }
        let path = if value.starts_with("/sys/") {
            PathBuf::from(value)
        } else if !value.is_empty()
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b':'))
        {
            Path::new("/sys/class/power_supply").join(value)
        } else {
            return None;
        };
        let canonical = fs::canonicalize(path.join("device"))
            .or_else(|_| fs::canonicalize(&path))
            .ok()?;
        let path = canonical.to_str()?.strip_prefix("/sys/devices/")?;
        let path = format!("/sys/devices/{path}");
        Some(path.split("/power_supply/").next()?.to_owned())
    }

    fn associate_upower(
        discovery: &mut Discovery,
        usb: &[UsbMetadata],
        readings: &[providers::upower::Reading],
    ) {
        let mut assignments: Vec<(usize, &providers::upower::Reading)> = Vec::new();
        for reading in readings {
            let identity = DeviceIdentity {
                serial: reading.serial.clone(),
                physical_path: reading
                    .native_path
                    .as_deref()
                    .and_then(|p| native_identity_path(p, usb, reading.device_type)),
                ..DeviceIdentity::default()
            };
            let matches: Vec<_> = discovery
                .devices
                .iter()
                .enumerate()
                .filter(|(_, device)| {
                    device.brand != Brand::Generic
                        && providers::upower::compatible_type(reading.device_type, device.kind)
                        && identities_match(&identity, &device.identity)
                })
                .map(|(index, _)| index)
                .collect();
            if matches.len() == 1 {
                assignments.push((matches[0], reading));
            } else if !matches.is_empty() {
                discovery.diagnostics.push(Diagnostic::new(
                    "upower-ambiguous",
                    ProviderKind::Upower,
                    AvailabilityCode::Unknown,
                    Severity::Warning,
                    "Batteria UPower non associata: identita periferica ambigua.",
                ));
            }
        }
        for (index, reading) in &assignments {
            if assignments
                .iter()
                .filter(|(candidate, _)| candidate == index)
                .count()
                != 1
            {
                continue;
            }
            let device = &mut discovery.devices[*index];
            let vendor_sample = device
                .values
                .battery
                .as_ref()
                .is_some_and(|b| b.percentage.is_some() || b.level.is_some());
            if !vendor_sample {
                device.values.battery = Some(reading.battery.clone());
            }
            if device.brand == Brand::Razer && device.identity.product_id == Some(0x00b8) {
                if let Some(battery) = &mut device.values.battery {
                    battery.charging = Some(false);
                }
            }
            if reading.battery.stale {
                device.limitations.push("Campione UPower vecchio o timestamp sorgente sconosciuto; non e una nuova misura hardware.".to_owned());
            }
        }
    }

    pub(super) async fn discover(audio_nodes: &[AudioNode], include_batteries: bool) -> Discovery {
        let (usb, usb_error) = match tokio::task::spawn_blocking(read_usb).await {
            Ok(Ok(usb)) => (usb, None),
            Ok(Err(error)) => (Vec::new(), Some(error)),
            Err(error) => (
                Vec::new(),
                Some(invalid_data("Worker USB interrotto.").detail(error.to_string())),
            ),
        };
        let (razer, ratbag, (power, power_diagnostics), headset) = tokio::join!(
            providers::openrazer::discover(&usb, include_batteries),
            providers::ratbag::discover(),
            async {
                if include_batteries {
                    providers::upower::discover().await
                } else {
                    (Vec::new(), Vec::new())
                }
            },
            async {
                if include_batteries {
                    providers::headsetcontrol::discover(&usb).await
                } else {
                    providers::headsetcontrol::Report::default()
                }
            }
        );
        let mut discovery = razer;
        discovery.devices.extend(ratbag.devices);
        discovery.diagnostics.extend(ratbag.diagnostics);
        discovery.diagnostics.extend(power_diagnostics);
        discovery.diagnostics.extend(headset.diagnostics.clone());
        if let Some(error) = usb_error {
            discovery
                .diagnostics
                .push(diagnostic(ProviderKind::Usb, "usb", &error));
        }
        merge_usb(&mut discovery, &usb);
        associate_headset_batteries(&mut discovery, &usb, &headset);
        associate_upower(&mut discovery, &usb, &power);
        associate_audio(&mut discovery, audio_nodes, &usb);
        discovery
    }

    pub(super) async fn watch_bus(
        sender: UnboundedSender<()>,
        session: bool,
        services: &[(&str, &str)],
    ) {
        let mut retry = Duration::from_secs(1);
        loop {
            if sender.is_closed() {
                return;
            }
            if let Ok(connection) = dbus::connection(session).await {
                let mut streams = Vec::new();
                for (service, root) in services {
                    let signal_rule = MatchRule::builder()
                        .msg_type(zbus::message::Type::Signal)
                        .sender(*service)
                        .and_then(|rule| rule.path_namespace(*root))
                        .map(|rule| rule.build().to_owned());
                    let owner_rule = MatchRule::builder()
                        .msg_type(zbus::message::Type::Signal)
                        .sender("org.freedesktop.DBus")
                        .and_then(|rule| rule.interface("org.freedesktop.DBus"))
                        .and_then(|rule| rule.member("NameOwnerChanged"))
                        .and_then(|rule| rule.arg(0, *service))
                        .map(|rule| rule.build().to_owned());
                    for rule in [signal_rule, owner_rule].into_iter().flatten() {
                        if let Ok(Ok(stream)) = tokio::time::timeout(
                            dbus::CALL_TIMEOUT,
                            MessageStream::for_match_rule(rule, &connection, Some(64)),
                        )
                        .await
                        {
                            streams.push(stream);
                        }
                    }
                }
                if streams.len() == services.len() * 2 {
                    retry = Duration::from_secs(1);
                    if sender.send(()).is_err() {
                        return;
                    }
                    let mut dirty = false;
                    let mut deadline = tokio::time::Instant::now();
                    loop {
                        let next = std::future::poll_fn(|cx| {
                            for stream in &mut streams {
                                let result = zbus::export::futures_core::Stream::poll_next(
                                    std::pin::Pin::new(stream),
                                    cx,
                                );
                                if result.is_ready() {
                                    return result;
                                }
                            }
                            std::task::Poll::Pending
                        });
                        tokio::select! {
                            _ = sender.closed() => return,
                            message = next => match message {
                                Some(Ok(_)) => { if !dirty { deadline = tokio::time::Instant::now() + Duration::from_millis(150); } dirty = true; }
                                _ => { let _ = sender.send(()); break; },
                            },
                            _ = tokio::time::sleep_until(deadline), if dirty => {
                                if sender.send(()).is_err() { return; }
                                dirty = false;
                            }
                        }
                    }
                }
            }
            tokio::select! { _ = sender.closed() => return, _ = tokio::time::sleep(retry) => {} }
            retry = (retry * 2).min(Duration::from_secs(30));
        }
    }
}
