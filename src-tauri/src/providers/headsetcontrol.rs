use crate::models::{AppResult, AvailabilityCode, Battery, Diagnostic, ProviderKind, Severity};
use crate::providers::{bounded_text, hex_id, invalid_data, MAX_DEVICES, MAX_PROVIDER_BYTES};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct Reading {
    pub vendor_id: u16,
    pub product_id: u16,
    pub name: String,
    pub readable_battery: bool,
    pub battery: Battery,
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub api_version: Option<String>,
    pub devices: Vec<Reading>,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn parse_json(bytes: &[u8], observed_at: u64) -> AppResult<Report> {
    if bytes.len() > MAX_PROVIDER_BYTES {
        return Err(invalid_data("Output JSON HeadsetControl troppo grande."));
    }
    let json: Value = serde_json::from_slice(bytes).map_err(|error| {
        invalid_data("Output HeadsetControl non JSON.").detail(error.to_string())
    })?;
    let root = json
        .as_object()
        .ok_or_else(|| invalid_data("Output HeadsetControl non e un oggetto JSON."))?;
    let api_version = root
        .get("api_version")
        .and_then(Value::as_str)
        .filter(|s| s.len() <= 32)
        .map(str::to_owned);
    let supported_api = matches!(api_version.as_deref(), Some("1.8" | "1.9"));
    let devices = root
        .get("devices")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_data("devices[] manca nell'output HeadsetControl."))?;
    if devices.len() > MAX_DEVICES {
        return Err(invalid_data("Troppi dispositivi HeadsetControl."));
    }
    let count_matches = root
        .get("device_count")
        .and_then(Value::as_u64)
        .is_none_or(|count| count == devices.len() as u64);
    let mut report = Report {
        api_version,
        ..Report::default()
    };
    if !supported_api || !count_matches {
        report.diagnostics.push(Diagnostic::new("headsetcontrol-api", ProviderKind::Headsetcontrol, AvailabilityCode::Unknown, Severity::Warning,
            "API HeadsetControl non verificata o conteggio incoerente: batteria lasciata non disponibile."));
    }
    for (index, device) in devices.iter().enumerate() {
        let Some(device) = device.as_object() else {
            report.diagnostics.push(Diagnostic::new(
                format!("headsetcontrol-record-{index}"),
                ProviderKind::Headsetcontrol,
                AvailabilityCode::Unknown,
                Severity::Warning,
                "Record cuffie HeadsetControl incompleto.",
            ));
            continue;
        };
        let vendor_id = device
            .get("id_vendor")
            .and_then(Value::as_str)
            .and_then(hex_id);
        let product_id = device
            .get("id_product")
            .and_then(Value::as_str)
            .and_then(hex_id);
        let (Some(0x046d), Some(product_id)) = (vendor_id, product_id) else {
            continue;
        };
        let readable = device
            .get("readable_capabilities")
            .and_then(Value::as_array);
        let advertised = device.get("capabilities").and_then(Value::as_array);
        let has_battery = |caps: Option<&Vec<Value>>| {
            caps.is_some_and(|caps| {
                caps.len() <= 128
                    && caps.iter().all(Value::is_string)
                    && caps
                        .iter()
                        .any(|cap| cap.as_str() == Some("CAP_BATTERY_STATUS"))
            })
        };
        let readable_battery =
            supported_api && count_matches && has_battery(readable) && has_battery(advertised);
        let success = device.get("status").and_then(Value::as_str) == Some("success");
        let no_errors = device.get("errors").is_none_or(|errors| match errors {
            Value::Object(errors) => errors.is_empty(),
            Value::Array(errors) => errors.is_empty(),
            Value::Null => true,
            _ => false,
        });
        let battery = device.get("battery").and_then(Value::as_object);
        let status = battery
            .and_then(|b| b.get("status"))
            .and_then(Value::as_str);
        let valid_status = matches!(status, Some("BATTERY_AVAILABLE" | "BATTERY_CHARGING"));
        let level = battery
            .and_then(|b| b.get("level"))
            .and_then(Value::as_i64)
            .filter(|level| (0..=100).contains(level));
        let available = readable_battery && success && no_errors && valid_status;
        let percentage = if available {
            level.map(|value| value as f64)
        } else {
            None
        };
        let charging = if available {
            match status {
                Some("BATTERY_CHARGING") => Some(true),
                Some("BATTERY_AVAILABLE") if level.is_some() => Some(false),
                _ => None,
            }
        } else {
            None
        };
        let name = bounded_text(
            device
                .get("device")
                .and_then(Value::as_str)
                .unwrap_or("Cuffie Logitech"),
        );
        if !available || percentage.is_none() {
            report.diagnostics.push(Diagnostic::new(
                format!("headsetcontrol-battery-{index}"),
                ProviderKind::Headsetcontrol,
                AvailabilityCode::Unknown,
                Severity::Info,
                format!("{name}: lettura batteria non disponibile o parziale."),
            ));
        }
        report.devices.push(Reading {
            vendor_id: 0x046d,
            product_id,
            name,
            readable_battery,
            battery: Battery {
                percentage,
                level: None,
                charging,
                source: ProviderKind::Headsetcontrol,
                observed_at,
                source_updated_at: None,
                stale: false,
            },
        });
    }
    Ok(report)
}

pub fn usb_fingerprint(usb: &[crate::devices::UsbMetadata]) -> String {
    let mut usb: Vec<_> = usb
        .iter()
        .filter(|device| device.identity.vendor_id == Some(0x046d) && device.is_audio)
        .collect();
    usb.sort_by(|a, b| a.instance.cmp(&b.instance));
    serde_json::to_string(
        &usb.iter()
            .map(|device| {
                (
                    &device.instance,
                    &device.identity.serial,
                    device.identity.vendor_id,
                    device.identity.product_id,
                )
            })
            .collect::<Vec<_>>(),
    )
    .unwrap_or_default()
}

#[cfg(target_os = "linux")]
pub(crate) async fn discover(usb: &[crate::devices::UsbMetadata]) -> Report {
    use crate::models::now_secs;
    use crate::providers::diagnostic;
    use std::{
        sync::OnceLock,
        time::{Duration, Instant},
    };
    use tokio::sync::Mutex;

    // Cache the USB instance generation too, never a model-only anonymous battery sample.
    // Holding the mutex during the probe prevents duplicate event/30s child processes.
    static CACHE: OnceLock<Mutex<Option<(String, Instant, Report)>>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(|| Mutex::new(None)).lock().await;
    let key = usb_fingerprint(usb);
    if key == "[]" {
        *cache = None;
        return Report::default();
    }
    if let Some((cached_key, observed, report)) = cache.as_ref() {
        if key == *cached_key && observed.elapsed() < Duration::from_secs(30) {
            return report.clone();
        }
    }
    let mut report = match crate::process::run(
        "headsetcontrol",
        &["-b", "-o", "json"],
        Duration::from_secs(5),
    )
    .await
    {
        Ok(output) => {
            match parse_json(&output.stdout, now_secs()) {
                Ok(mut report) => {
                    if !output.stderr.trim().is_empty() {
                        let mut warning = Diagnostic::new("headsetcontrol-stderr", ProviderKind::Headsetcontrol, AvailabilityCode::Unknown, Severity::Warning,
                        "HeadsetControl ha segnalato un errore: campioni batteria non confermati.");
                        warning.detail = Some(
                            crate::models::AppError::new(
                                crate::models::ErrorCode::CommandFailed,
                                "HeadsetControl",
                            )
                            .detail(&output.stderr)
                            .detail
                            .unwrap_or_default(),
                        );
                        report.diagnostics.push(warning);
                        for device in &mut report.devices {
                            device.battery.percentage = None;
                            device.battery.charging = None;
                        }
                    }
                    report
                }
                Err(error) => Report {
                    diagnostics: vec![diagnostic(
                        ProviderKind::Headsetcontrol,
                        "headsetcontrol-json",
                        &error,
                    )],
                    ..Report::default()
                },
            }
        }
        Err(error) => Report {
            diagnostics: vec![diagnostic(
                ProviderKind::Headsetcontrol,
                "headsetcontrol",
                &error,
            )],
            ..Report::default()
        },
    };
    let current_usb = tokio::task::spawn_blocking(crate::devices::read_usb).await;
    if !matches!(current_usb, Ok(Ok(ref current)) if usb_fingerprint(current) == key) {
        report.devices.clear();
        report.diagnostics.push(Diagnostic::new(
            "headsetcontrol-hotplug",
            ProviderKind::Headsetcontrol,
            AvailabilityCode::Unknown,
            Severity::Warning,
            "Istanze USB cambiate durante il probe: batteria anonima non associata.",
        ));
    }
    *cache = Some((key, Instant::now(), report.clone()));
    report
}
