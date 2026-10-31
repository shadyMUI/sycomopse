use crate::models::{Battery, BatteryLevel, DeviceKind, ProviderKind};
use serde::Deserialize;

pub const STALE_AFTER_SECS: u64 = 300;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct BatteryProperties {
    pub percentage: Option<f64>,
    pub battery_level: Option<u32>,
    pub state: Option<u32>,
    pub update_time: Option<u64>,
}

pub fn battery(properties: &BatteryProperties, observed_at: u64) -> Battery {
    // UPower's BatteryLevel uses the warning-level enum, not a contiguous 0..6 scale.
    let level = match properties.battery_level {
        Some(0) => Some(BatteryLevel::Unknown),
        Some(3) => Some(BatteryLevel::Low),
        Some(4) => Some(BatteryLevel::Critical),
        Some(6) => Some(BatteryLevel::Normal),
        Some(7) => Some(BatteryLevel::High),
        Some(8) => Some(BatteryLevel::Full),
        _ => None,
    };
    let percentage = if properties.battery_level == Some(1) {
        properties
            .percentage
            .filter(|p| p.is_finite() && (0.0..=100.0).contains(p))
    } else {
        None
    };
    let charging = match properties.state {
        Some(1) => Some(true),
        Some(2..=4) => Some(false),
        _ => None,
    };
    let source_updated_at = properties.update_time.filter(|time| *time > 0);
    let stale = source_updated_at.is_none_or(|time| {
        time > observed_at.saturating_add(30) || observed_at.saturating_sub(time) > STALE_AFTER_SECS
    });
    Battery {
        percentage,
        level,
        charging,
        source: ProviderKind::Upower,
        observed_at,
        source_updated_at,
        stale,
    }
}

pub fn compatible_type(device_type: u32, kind: DeviceKind) -> bool {
    matches!(
        (device_type, kind),
        (5, DeviceKind::Mouse) | (17 | 19, DeviceKind::Headset)
    )
}

#[derive(Debug, Clone)]
pub struct Reading {
    pub native_path: Option<String>,
    pub serial: Option<String>,
    pub device_type: u32,
    pub battery: Battery,
}

#[cfg(target_os = "linux")]
pub(crate) use linux::discover;

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::models::{now_secs, AppError, AppResult, Diagnostic, ErrorCode};
    use crate::providers::{
        dbus::{property, Bus},
        diagnostic, invalid_data, valid_serial, MAX_DEVICES,
    };
    use std::collections::HashSet;
    use zbus::zvariant::OwnedObjectPath;

    const SERVICE: &str = "org.freedesktop.UPower";
    const ROOT: &str = "/org/freedesktop/UPower";
    const DEVICE: &str = "org.freedesktop.UPower.Device";

    pub(crate) async fn discover() -> (Vec<Reading>, Vec<Diagnostic>) {
        match read().await {
            Ok(result) => result,
            Err(error) => (
                Vec::new(),
                vec![diagnostic(ProviderKind::Upower, "upower", &error)],
            ),
        }
    }

    async fn read() -> AppResult<(Vec<Reading>, Vec<Diagnostic>)> {
        let bus = Bus::active(SERVICE, false).await.map_err(|error| {
            if error.code == ErrorCode::ProviderMissing {
                AppError::new(ErrorCode::ProviderMissing, "UPower non attivo: batteria OS opzionale non disponibile. Il servizio non viene avviato dall'app.")
            } else { error }
        })?;
        let introspection = bus.introspect(ROOT).await?;
        if !introspection.method(SERVICE, "EnumerateDevices", "", "ao") {
            return Err(invalid_data(
                "UPower non espone EnumerateDevices con la firma prevista.",
            ));
        }
        let paths: Vec<OwnedObjectPath> = bus.call(ROOT, SERVICE, "EnumerateDevices", &()).await?;
        if paths.len() > MAX_DEVICES {
            return Err(invalid_data("Troppi dispositivi UPower."));
        }
        let mut seen = HashSet::new();
        let mut readings = Vec::new();
        let mut diagnostics = Vec::new();
        for path in paths {
            let path = path.as_str();
            if !path.starts_with("/org/freedesktop/UPower/devices/")
                || !seen.insert(path.to_owned())
            {
                diagnostics.push(diagnostic(
                    ProviderKind::Upower,
                    "upower-path",
                    &invalid_data("Percorso UPower duplicato o inatteso."),
                ));
                continue;
            }
            match bus.get_all(path, DEVICE).await {
                Ok(properties) => {
                    let device_type: u32 = match property(&properties, "Type") {
                        Ok(value) => value,
                        Err(error) => {
                            diagnostics.push(diagnostic(ProviderKind::Upower, path, &error));
                            continue;
                        }
                    };
                    if !matches!(device_type, 5 | 17 | 19) {
                        continue;
                    }
                    let native_path = property::<String>(&properties, "NativePath")
                        .ok()
                        .filter(|p| !p.is_empty() && p.len() <= 4096);
                    let serial = property::<String>(&properties, "Serial")
                        .ok()
                        .as_deref()
                        .and_then(valid_serial);
                    let observed_at = now_secs();
                    let values = BatteryProperties {
                        percentage: property(&properties, "Percentage").ok(),
                        battery_level: property(&properties, "BatteryLevel").ok(),
                        state: property(&properties, "State").ok(),
                        update_time: property(&properties, "UpdateTime").ok(),
                    };
                    for name in ["Percentage", "BatteryLevel", "State", "UpdateTime"] {
                        if !properties.contains_key(name) {
                            diagnostics.push(diagnostic(
                                ProviderKind::Upower,
                                format!("{path}:{name}"),
                                &invalid_data(
                                    "Proprieta batteria UPower assente; dato lasciato sconosciuto.",
                                )
                                .detail(name),
                            ));
                        }
                    }
                    readings.push(Reading {
                        native_path,
                        serial,
                        device_type,
                        battery: battery(&values, observed_at),
                    });
                }
                Err(error) => diagnostics.push(diagnostic(ProviderKind::Upower, path, &error)),
            }
        }
        Ok((readings, diagnostics))
    }
}
