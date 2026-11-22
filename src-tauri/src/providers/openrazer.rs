use crate::models::*;
use crate::providers::{invalid_data, ordered_values, Introspection};

pub const DPI_INTERFACE: &str = "razer.device.dpi";
pub const MISC_INTERFACE: &str = "razer.device.misc";
pub const POWER_INTERFACE: &str = "razer.device.power";
pub const VIPER_PRODUCT: u16 = 0x00b8;

pub fn parse_vid_pid(values: &[i32]) -> AppResult<(u16, u16)> {
    if values.len() != 2 {
        return Err(invalid_data("getVidPid deve restituire due Int32."));
    }
    let vendor =
        u16::try_from(values[0]).map_err(|_| invalid_data("Vendor USB OpenRazer non valido."))?;
    let product =
        u16::try_from(values[1]).map_err(|_| invalid_data("Product USB OpenRazer non valido."))?;
    if vendor != 0x1532 {
        return Err(invalid_data("Vendor OpenRazer non Razer."));
    }
    Ok((vendor, product))
}

pub fn parse_dpi(values: &[i32], scalar: bool) -> AppResult<DpiValue> {
    if values.len() != 2 {
        return Err(invalid_data("getDPI deve restituire due Int32."));
    }
    let x = u32::try_from(values[0])
        .ok()
        .filter(|x| *x > 0 && *x <= u32::from(u16::MAX))
        .ok_or_else(|| invalid_data("DPI X OpenRazer non valido."))?;
    let y = if scalar && values[1] == 0 {
        x
    } else {
        u32::try_from(values[1])
            .ok()
            .filter(|y| *y > 0 && *y <= u32::from(u16::MAX))
            .ok_or_else(|| invalid_data("DPI Y OpenRazer non valido."))?
    };
    if scalar && y != x {
        return Err(invalid_data("Assi DPI incoerenti con availableDPI."));
    }
    Ok(DpiValue { x, y })
}

pub fn dpi_capability(
    api: &Introspection,
    product: u16,
    maximum: Option<u32>,
    discrete: Option<Vec<u32>>,
    current: Option<DpiValue>,
) -> Option<DpiCapability> {
    let getter = api.method(DPI_INTERFACE, "getDPI", "", "ai")
        || api.method(DPI_INTERFACE, "getDPIStages", "", "(ya(qq))");
    let setter = api.method(DPI_INTERFACE, "setDPI", "qq", "");
    if !getter && !setter {
        return None;
    }
    let scalar = product != VIPER_PRODUCT && api.method(DPI_INTERFACE, "availableDPI", "", "ai");
    let values = if product == VIPER_PRODUCT && maximum == Some(30000) {
        DpiValues::Range {
            min: 100,
            max: 30000,
            step: 1,
        }
    } else if scalar {
        discrete
            .filter(|values| values.iter().all(|v| *v <= u32::from(u16::MAX)))
            .and_then(|values| ordered_values(values, 300).ok())
            .map(|values| DpiValues::Discrete { values })
            .unwrap_or(DpiValues::Unknown)
    } else {
        DpiValues::Unknown
    };
    let values_known = !matches!(values, DpiValues::Unknown);
    let current_valid =
        current.is_some_and(|current| values.contains(current.x) && values.contains(current.y));
    let writable = getter && setter && values_known;
    let reason = if !values_known {
        Some("Minimo/lista DPI del modello non verificati, oppure maxDPI runtime incoerente: sola lettura.".to_owned())
    } else if !getter || !setter {
        Some("Firme getDPI/setDPI non disponibili nell'introspection installata.".to_owned())
    } else if !current_valid {
        Some("DPI corrente non disponibile o fuori elenco: scegliere un valore verificato prima di Applica.".to_owned())
    } else {
        None
    };
    Some(DpiCapability {
        values,
        writable,
        separate_axes: !scalar,
        reason,
    })
}

pub fn polling_capability(
    api: &Introspection,
    product: u16,
    supported: Option<Vec<u32>>,
    current: Option<u32>,
) -> Option<PollingCapability> {
    let getter = api.method(MISC_INTERFACE, "getPollRate", "", "i");
    let setter = api.method(MISC_INTERFACE, "setPollRate", "q", "");
    if !getter && !setter {
        return None;
    }
    let runtime_list = api.method(MISC_INTERFACE, "getSupportedPollRates", "", "aq");
    let values = if runtime_list {
        supported
            .and_then(|rates| ordered_values(rates, 32).ok())
            .unwrap_or_default()
    } else if product == VIPER_PRODUCT {
        vec![125, 500, 1000]
    } else {
        Vec::new()
    };
    let documented =
        product != VIPER_PRODUCT || values.iter().all(|value| [125, 500, 1000].contains(value));
    let writable = getter
        && setter
        && !values.is_empty()
        && documented
        && current.is_some_and(|rate| values.contains(&rate));
    let reason = if values.is_empty() {
        Some("Rate supportati non verificati per il modello installato.".to_owned())
    } else if !documented {
        Some(
            "Rate runtime inattesi per Viper V3 HyperSpeed; non si presume HyperPolling."
                .to_owned(),
        )
    } else if !writable {
        Some("Getter/setter polling o stato corrente non validi.".to_owned())
    } else {
        None
    };
    Some(PollingCapability {
        values,
        writable,
        reason,
    })
}

pub fn battery(
    percentage: Option<f64>,
    charging: Option<bool>,
    product: u16,
    observed_at: u64,
) -> Battery {
    Battery {
        percentage: percentage.filter(|p| p.is_finite() && (0.0..=100.0).contains(p)),
        level: None,
        charging: if product == VIPER_PRODUCT {
            Some(false)
        } else {
            charging
        },
        source: ProviderKind::Openrazer,
        observed_at,
        source_updated_at: None,
        stale: false,
    }
}

#[cfg(target_os = "linux")]
#[allow(unused_imports)]
pub(crate) use linux::check_hardware_change;
#[cfg(target_os = "linux")]
pub(crate) use linux::{discover, set_dpi, set_polling_rate};
#[cfg(target_os = "linux")]
pub use linux::{format_persistence_config, parse_existing_config};

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::devices::UsbMetadata;
    use crate::providers::{
        bounded_text, dbus::Bus, diagnostic, identities_match, unique_identity_match, valid_serial,
        MAX_DEVICES,
    };
    use std::{collections::HashSet, time::Duration};

    const SERVICE: &str = "org.razer";
    const ROOT: &str = "/org/razer";
    const MANAGER: &str = "razer.devices";

    async fn connect_bus() -> AppResult<Bus> {
        let bus = Bus::active(SERVICE, true).await?;
        if !bus
            .introspect(ROOT)
            .await?
            .method(MANAGER, "getDevices", "", "as")
        {
            return Err(invalid_data("OpenRazer non espone getDevices() -> as."));
        }
        Ok(bus)
    }

    async fn manager() -> AppResult<Bus> {
        if let Ok(bus) = connect_bus().await {
            return Ok(bus);
        }

        let startup_error = crate::process::run(
            "systemctl",
            &["--user", "start", "openrazer-daemon"],
            Duration::from_secs(5),
        )
        .await
        .err();
        let startup_detail = match &startup_error {
            Some(error) => {
                let detail = format!(
                    "systemctl --user start openrazer-daemon: {:?}; {}; {}",
                    error.code,
                    error.message,
                    bounded_text(error.detail.as_deref().unwrap_or("Nessun dettaglio."))
                );
                eprintln!("Avvio OpenRazer non riuscito: {detail}");
                detail
            }
            None => "systemctl --user start openrazer-daemon: completato.".to_owned(),
        };
        tokio::time::sleep(Duration::from_millis(350)).await;

        let result = connect_bus().await;
        result.map_err(|error| {
            let message = if error.code == ErrorCode::ProviderMissing {
                "OpenRazer non attivo sul session bus dopo il tentativo di avvio automatico. Verificare openrazer-daemon.service, driver e permessi dell'utente.".to_owned()
            } else {
                format!(
                    "OpenRazer non utilizzabile dopo il tentativo di avvio automatico: {}",
                    error.message
                )
            };
            let detail = format!(
                "{startup_detail}\nDBus ({:?}): {}; {}",
                error.code,
                error.message,
                bounded_text(error.detail.as_deref().unwrap_or("Nessun dettaglio."))
            );
            AppError::new(error.code, message).detail(detail)
        })
    }

    async fn serials(bus: &Bus) -> AppResult<Vec<String>> {
        let values: Vec<String> = bus.call(ROOT, MANAGER, "getDevices", &()).await?;
        if values.len() > MAX_DEVICES {
            return Err(invalid_data("Troppi dispositivi OpenRazer."));
        }
        if values.iter().any(|serial| {
            valid_serial(serial).as_deref() != Some(serial.as_str())
                || !serial
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        }) {
            return Err(invalid_data("Seriale/percorso OpenRazer non valido."));
        }
        Ok(values)
    }

    pub(crate) async fn discover(usb: &[UsbMetadata], include_batteries: bool) -> Discovery {
        let mut discovery = Discovery::default();
        let bus = match manager().await {
            Ok(bus) => bus,
            Err(error) => {
                discovery.diagnostics.push(diagnostic(
                    ProviderKind::Openrazer,
                    "openrazer",
                    &error,
                ));
                return discovery;
            }
        };
        let serials = match serials(&bus).await {
            Ok(serials) => serials,
            Err(error) => {
                discovery.diagnostics.push(diagnostic(
                    ProviderKind::Openrazer,
                    "openrazer-devices",
                    &error,
                ));
                return discovery;
            }
        };
        let mut seen = HashSet::new();
        for serial in &serials {
            if serials.iter().filter(|s| *s == serial).count() != 1 || !seen.insert(serial) {
                discovery.diagnostics.push(diagnostic(
                    ProviderKind::Openrazer,
                    "openrazer-serial",
                    &invalid_data("Seriale OpenRazer duplicato; istanza non identificabile."),
                ));
                continue;
            }
            match read_device(&bus, serial, usb, include_batteries).await {
                Ok((device, diagnostics)) => {
                    discovery.devices.push(device);
                    discovery.diagnostics.extend(diagnostics);
                }
                Err(error) => discovery.diagnostics.push(diagnostic(
                    ProviderKind::Openrazer,
                    format!("openrazer:{serial}"),
                    &error,
                )),
            }
        }
        discovery
    }

    async fn read_device(
        bus: &Bus,
        serial: &str,
        usb: &[UsbMetadata],
        include_batteries: bool,
    ) -> AppResult<(Device, Vec<Diagnostic>)> {
        let path = format!("/org/razer/device/{serial}");
        let api = bus.introspect(&path).await?;
        if !api.method(MISC_INTERFACE, "getVidPid", "", "ai") {
            return Err(invalid_data("OpenRazer getVidPid() -> ai non disponibile."));
        }
        let ids: Vec<i32> = bus.call(&path, MISC_INTERFACE, "getVidPid", &()).await?;
        let (vendor, product) = parse_vid_pid(&ids)?;
        let mut diagnostics = Vec::new();
        let mut identity = DeviceIdentity {
            serial: Some(serial.to_owned()),
            vendor_id: Some(vendor),
            product_id: Some(product),
            backend_path: Some(path.clone()),
            ..DeviceIdentity::default()
        };
        let usb_identities: Vec<_> = usb.iter().map(|u| u.identity.clone()).collect();
        if let Some(index) = unique_identity_match(&identity, &usb_identities) {
            identity.physical_path = usb[index].identity.physical_path.clone();
            identity.transport = usb[index].identity.transport.clone();
        } else {
            let matching_usb: Vec<_> = usb
                .iter()
                .filter(|u| {
                    u.identity.vendor_id == Some(vendor) && u.identity.product_id == Some(product)
                })
                .collect();
            if matching_usb.len() == 1 {
                identity.physical_path = matching_usb[0].identity.physical_path.clone();
                identity.transport = matching_usb[0].identity.transport.clone();
            }
        }
        let mut name = if product == VIPER_PRODUCT {
            "Razer Viper V3 HyperSpeed".to_owned()
        } else {
            format!("Razer {vendor:04x}:{product:04x}")
        };
        if api.method(MISC_INTERFACE, "getDeviceName", "", "s") {
            match bus
                .call::<_, String>(&path, MISC_INTERFACE, "getDeviceName", &())
                .await
            {
                Ok(value) if !value.trim().is_empty() => name = bounded_text(value.trim()),
                Ok(_) => {}
                Err(error) => diagnostics.push(diagnostic(
                    ProviderKind::Openrazer,
                    format!("{path}:name"),
                    &error,
                )),
            }
        }
        if api.method(MISC_INTERFACE, "getFirmware", "", "s") {
            match bus
                .call::<_, String>(&path, MISC_INTERFACE, "getFirmware", &())
                .await
            {
                Ok(value) => identity.firmware = Some(bounded_text(value.trim())),
                Err(error) => diagnostics.push(diagnostic(
                    ProviderKind::Openrazer,
                    format!("{path}:firmware"),
                    &error,
                )),
            }
        }
        let mut kind = if product == 0x00b3 {
            DeviceKind::Receiver
        } else if product == VIPER_PRODUCT {
            DeviceKind::Mouse
        } else {
            DeviceKind::Unknown
        };
        if product != 0x00b3 && api.method(MISC_INTERFACE, "getDeviceType", "", "s") {
            match bus
                .call::<_, String>(&path, MISC_INTERFACE, "getDeviceType", &())
                .await
            {
                Ok(value) => {
                    kind = match value.as_str() {
                        "mouse" => DeviceKind::Mouse,
                        "headset" => DeviceKind::Headset,
                        _ => DeviceKind::Unknown,
                    }
                }
                Err(error) => diagnostics.push(diagnostic(
                    ProviderKind::Openrazer,
                    format!("{path}:type"),
                    &error,
                )),
            }
        }
        let mut maximum = None;
        if api.method(DPI_INTERFACE, "maxDPI", "", "i") {
            match bus
                .call::<_, i32>(&path, DPI_INTERFACE, "maxDPI", &())
                .await
            {
                Ok(value) => {
                    maximum = u32::try_from(value)
                        .ok()
                        .filter(|v| *v > 0 && *v <= u32::from(u16::MAX))
                }
                Err(error) => diagnostics.push(diagnostic(
                    ProviderKind::Openrazer,
                    format!("{path}:maxDPI"),
                    &error,
                )),
            }
        }
        let scalar =
            product != VIPER_PRODUCT && api.method(DPI_INTERFACE, "availableDPI", "", "ai");
        let mut discrete = None;
        if scalar {
            let result: AppResult<Vec<i32>> =
                bus.call(&path, DPI_INTERFACE, "availableDPI", &()).await;
            match result.and_then(|values| {
                if values.len() > 300 {
                    return Err(invalid_data("Lista DPI OpenRazer troppo grande."));
                }
                values
                    .into_iter()
                    .map(|value| {
                        u32::try_from(value)
                            .map_err(|_| invalid_data("availableDPI contiene valori negativi."))
                    })
                    .collect::<AppResult<Vec<_>>>()
                    .and_then(|values| ordered_values(values, 300))
            }) {
                Ok(values) => discrete = Some(values),
                Err(error) => diagnostics.push(diagnostic(
                    ProviderKind::Openrazer,
                    format!("{path}:availableDPI"),
                    &error,
                )),
            }
        }
        let config_dir = directories::BaseDirs::new()
            .map(|dirs| dirs.config_dir().join("openrazer"))
            .unwrap_or_else(|| std::path::PathBuf::from("/home/shadymui/.config/openrazer"));
        let persistence_content =
            std::fs::read_to_string(config_dir.join("persistence.conf")).unwrap_or_default();
        let (persisted_x, persisted_y, persisted_rate) =
            parse_existing_config(&persistence_content, serial, identity.serial.as_deref());

        let mut current_dpi = None;
        if api.method(DPI_INTERFACE, "getDPIStages", "", "(ya(qq))") {
            if let Ok((active_stage, stages)) = bus
                .call::<_, (u8, Vec<(u16, u16)>)>(&path, DPI_INTERFACE, "getDPIStages", &())
                .await
            {
                if !stages.is_empty()
                    && active_stage >= 1
                    && (active_stage as usize) <= stages.len()
                {
                    let stage = stages[(active_stage - 1) as usize];
                    if stage.0 > 0 && stage.1 > 0 {
                        let stage_dpi = DpiValue {
                            x: u32::from(stage.0),
                            y: u32::from(stage.1),
                        };
                        current_dpi = Some(stage_dpi);
                        if api.method(DPI_INTERFACE, "setDPI", "qq", "") {
                            let _ = bus
                                .call::<_, ()>(&path, DPI_INTERFACE, "setDPI", &(stage.0, stage.1))
                                .await;
                        }
                    }
                }
            }
        }
        if current_dpi.is_none() && api.method(DPI_INTERFACE, "getDPI", "", "ai") {
            let result: AppResult<Vec<i32>> = bus.call(&path, DPI_INTERFACE, "getDPI", &()).await;
            match result.and_then(|values| parse_dpi(&values, scalar)) {
                Ok(value) => current_dpi = Some(value),
                Err(error) => {
                    if let (Some(x), Some(y)) = (persisted_x, persisted_y) {
                        if x > 0 && y > 0 && (!scalar || x == y) {
                            current_dpi = Some(DpiValue { x, y });
                            if api.method(DPI_INTERFACE, "setDPI", "qq", "") {
                                if let (Ok(x_wire), Ok(y_wire)) =
                                    (u16::try_from(x), u16::try_from(y))
                                {
                                    let _ = bus
                                        .call::<_, ()>(
                                            &path,
                                            DPI_INTERFACE,
                                            "setDPI",
                                            &(x_wire, y_wire),
                                        )
                                        .await;
                                }
                            }
                        }
                    }
                    if current_dpi.is_none() {
                        diagnostics.push(diagnostic(
                            ProviderKind::Openrazer,
                            format!("{path}:getDPI"),
                            &error,
                        ));
                    }
                }
            }
        }
        let mut supported_rates = None;
        if api.method(MISC_INTERFACE, "getSupportedPollRates", "", "aq") {
            match bus
                .call::<_, Vec<u16>>(&path, MISC_INTERFACE, "getSupportedPollRates", &())
                .await
            {
                Ok(values) => match ordered_values(values.into_iter().map(u32::from).collect(), 32)
                {
                    Ok(values) => supported_rates = Some(values),
                    Err(error) => diagnostics.push(diagnostic(
                        ProviderKind::Openrazer,
                        format!("{path}:rates"),
                        &error,
                    )),
                },
                Err(error) => diagnostics.push(diagnostic(
                    ProviderKind::Openrazer,
                    format!("{path}:rates"),
                    &error,
                )),
            }
        }
        let mut current_rate = None;
        if api.method(MISC_INTERFACE, "getPollRate", "", "i") {
            match bus
                .call::<_, i32>(&path, MISC_INTERFACE, "getPollRate", &())
                .await
            {
                Ok(value) => {
                    current_rate = u32::try_from(value)
                        .ok()
                        .filter(|v| *v > 0 && *v <= u32::from(u16::MAX))
                }
                Err(error) => diagnostics.push(diagnostic(
                    ProviderKind::Openrazer,
                    format!("{path}:polling"),
                    &error,
                )),
            }
        }
        if let Some(target_rate) = persisted_rate {
            if supported_rates
                .as_ref()
                .map_or(true, |rates| rates.contains(&target_rate))
            {
                if current_rate.is_none()
                    || current_rate == Some(0)
                    || (current_rate == Some(500) && target_rate != 500)
                {
                    if api.method(MISC_INTERFACE, "setPollRate", "q", "") {
                        if let Ok(rate_wire) = u16::try_from(target_rate) {
                            let _ = bus
                                .call::<_, ()>(&path, MISC_INTERFACE, "setPollRate", &(rate_wire,))
                                .await;
                        }
                    }
                    current_rate = Some(target_rate);
                }
            }
        }
        let mut battery_value = None;
        if include_batteries
            && (api.method(POWER_INTERFACE, "getBattery", "", "d")
                || api.method(POWER_INTERFACE, "isCharging", "", "b"))
        {
            let mut percentage = None;
            let mut charging = None;
            if api.method(POWER_INTERFACE, "getBattery", "", "d") {
                match bus
                    .call::<_, f64>(&path, POWER_INTERFACE, "getBattery", &())
                    .await
                {
                    Ok(value) => percentage = Some(value),
                    Err(error) => diagnostics.push(diagnostic(
                        ProviderKind::Openrazer,
                        format!("{path}:battery"),
                        &error,
                    )),
                }
            }
            if product != VIPER_PRODUCT && api.method(POWER_INTERFACE, "isCharging", "", "b") {
                match bus
                    .call::<_, bool>(&path, POWER_INTERFACE, "isCharging", &())
                    .await
                {
                    Ok(value) => charging = Some(value),
                    Err(error) => diagnostics.push(diagnostic(
                        ProviderKind::Openrazer,
                        format!("{path}:charging"),
                        &error,
                    )),
                }
            }
            battery_value = Some(battery(percentage, charging, product, now_secs()));
        }
        let mut capabilities = DeviceCapabilities {
            dpi: dpi_capability(&api, product, maximum, discrete, current_dpi),
            polling: polling_capability(&api, product, supported_rates, current_rate),
            remapping: None,
        };
        let ambiguous_usb = usb_identities
            .iter()
            .filter(|candidate| identities_match(&identity, candidate))
            .count()
            > 1;
        if kind != DeviceKind::Mouse || ambiguous_usb {
            let reason = "Istanza mouse non verificata o seriale USB ambiguo.".to_owned();
            if let Some(cap) = &mut capabilities.dpi {
                cap.writable = false;
                cap.reason = Some(reason.clone());
            }
            if let Some(cap) = &mut capabilities.polling {
                cap.writable = false;
                cap.reason = Some(reason);
            }
        }
        let support = if capabilities.dpi.as_ref().is_some_and(|c| c.writable)
            && capabilities.polling.as_ref().is_some_and(|c| c.writable)
        {
            SupportLevel::Supported
        } else {
            SupportLevel::Partial
        };
        let mut limitations = vec!["Unico proprietario delle scritture Razer: OpenRazer; l'app tenta l'avvio del daemon tramite systemctl --user.".to_owned(),
            "Nessun remapping, RGB, LOD, calibrazione o selettore chimica batteria in questa versione.".to_owned(),
            "getPollRate e uno stato del daemon, non una misura fisica della frequenza.".to_owned()];
        if product == VIPER_PRODUCT {
            limitations.push("Batteria AA del Viper V3 HyperSpeed: non ricaricabile nel mouse, charging sempre false.".to_owned());
        }
        Ok((
            Device {
                id: format!("openrazer:{serial}"),
                name,
                brand: Brand::Razer,
                kind,
                provider: ProviderKind::Openrazer,
                identity,
                availability: AvailabilityCode::Available,
                support,
                capabilities,
                values: DeviceValues {
                    dpi: current_dpi,
                    polling_rate: current_rate,
                    battery: battery_value,
                    remapping: None,
                    staged: false,
                },
                output_node_names: Vec::new(),
                limitations,
            },
            diagnostics,
        ))
    }

    pub fn parse_existing_config(
        contents: &str,
        storage_name: &str,
        serial: Option<&str>,
    ) -> (Option<u32>, Option<u32>, Option<u32>) {
        let mut cur_section: Option<String> = None;
        let mut default_dpi_x = None;
        let mut default_dpi_y = None;
        let mut default_rate = None;
        let mut matched_dpi_x = None;
        let mut matched_dpi_y = None;
        let mut matched_rate = None;

        for line in contents.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                let sec = trimmed[1..trimmed.len() - 1].trim();
                cur_section = Some(sec.to_string());
                continue;
            }
            let Some((k, v)) = trimmed.split_once('=') else {
                continue;
            };
            let key = k.trim();
            let val = v.trim();
            if let Some(sec) = &cur_section {
                let is_default = sec.eq_ignore_ascii_case("DEFAULT");
                let is_match = sec == storage_name || serial.is_some_and(|s| s == sec);
                if is_default {
                    if key == "dpi_x" {
                        default_dpi_x = val.parse().ok();
                    } else if key == "dpi_y" {
                        default_dpi_y = val.parse().ok();
                    } else if key == "poll_rate" {
                        default_rate = val.parse().ok();
                    }
                } else if is_match {
                    if key == "dpi_x" {
                        matched_dpi_x = val.parse().ok();
                    } else if key == "dpi_y" {
                        matched_dpi_y = val.parse().ok();
                    } else if key == "poll_rate" {
                        matched_rate = val.parse().ok();
                    }
                }
            }
        }
        (
            matched_dpi_x.or(default_dpi_x),
            matched_dpi_y.or(default_dpi_y),
            matched_rate.or(default_rate),
        )
    }

    async fn authoritative(bus: &Bus, previous: &Device) -> AppResult<Device> {
        bus.ensure_owner(SERVICE).await?;
        let serial = previous
            .identity
            .serial
            .as_deref()
            .and_then(valid_serial)
            .ok_or_else(|| invalid_data("Seriale OpenRazer non verificato."))?;
        let active_serials = serials(bus).await?;
        let target_serial = if active_serials.contains(&serial) {
            serial
        } else if active_serials.len() == 1 {
            active_serials[0].clone()
        } else if let Some(matched) = active_serials.iter().find(|s| {
            previous.identity.backend_path.as_deref() == Some(&format!("/org/razer/device/{s}"))
                || (serial.starts_with("UNKWN") && s.starts_with("UNKWN"))
        }) {
            matched.clone()
        } else {
            return Err(AppError::new(
                ErrorCode::DeviceNotFound,
                "Istanza OpenRazer scollegata o ambigua.",
            ));
        };
        let usb = tokio::task::spawn_blocking(crate::devices::read_usb)
            .await
            .map_err(|error| invalid_data("Worker USB interrotto.").detail(error.to_string()))??;
        let (device, _) = read_device(bus, &target_serial, &usb, false).await?;
        if (!identities_match(&previous.identity, &device.identity) && active_serials.len() != 1)
            || device.kind != DeviceKind::Mouse
            || device.brand != Brand::Razer
        {
            return Err(AppError::new(
                ErrorCode::DeviceNotFound,
                "Identita OpenRazer cambiata; aggiornare la discovery.",
            ));
        }
        Ok(device)
    }

    pub(crate) async fn set_dpi(previous: &Device, x: u32, y: u32) -> AppResult<()> {
        let bus = manager().await?;
        let capability = previous
            .capabilities
            .dpi
            .as_ref()
            .filter(|c| c.writable)
            .ok_or_else(|| {
                AppError::new(
                    ErrorCode::Unsupported,
                    "DPI OpenRazer attualmente di sola lettura.",
                )
            })?;
        if !capability.values.contains(x)
            || !capability.values.contains(y)
            || (!capability.separate_axes && x != y)
        {
            return Err(AppError::new(
                ErrorCode::Validation,
                "DPI non ammessi dal modello OpenRazer attivo.",
            ));
        }
        let x_wire = u16::try_from(x)
            .map_err(|_| AppError::new(ErrorCode::Validation, "DPI oltre UInt16."))?;
        let y_wire = if capability.separate_axes {
            u16::try_from(y)
                .map_err(|_| AppError::new(ErrorCode::Validation, "DPI oltre UInt16."))?
        } else {
            0
        };
        let path = previous
            .identity
            .backend_path
            .as_deref()
            .ok_or_else(|| invalid_data("Percorso OpenRazer assente."))?;

        bus.call::<_, ()>(path, DPI_INTERFACE, "setDPI", &(x_wire, y_wire))
            .await?;

        // If on-board DPI stages are exposed, sync to flash memory VARSTORE
        let api = bus.introspect(path).await.ok();
        if let Some(api) = &api {
            if api.method(DPI_INTERFACE, "getDPIStages", "", "(ya(qq))")
                && api.method(DPI_INTERFACE, "setDPIStages", "ya(qq)", "")
            {
                if let Ok((active_stage, stages)) = bus
                    .call::<_, (u8, Vec<(u16, u16)>)>(path, DPI_INTERFACE, "getDPIStages", &())
                    .await
                {
                    if !stages.is_empty()
                        && active_stage >= 1
                        && (active_stage as usize) <= stages.len()
                    {
                        let mut new_stages = stages;
                        let y_stage = if capability.separate_axes {
                            y_wire
                        } else {
                            x_wire
                        };
                        new_stages[(active_stage - 1) as usize] = (x_wire, y_stage);
                        let _ = bus
                            .call::<_, ()>(
                                path,
                                DPI_INTERFACE,
                                "setDPIStages",
                                &(active_stage, new_stages),
                            )
                            .await;
                    }
                }
            }
        }

        let readback = authoritative(&bus, previous).await.map_err(|error| {
            AppError::new(
                ErrorCode::NotConfirmed,
                "DPI inviati ma dispositivo/readback non confermati.",
            )
            .detail(error.to_string())
        })?;
        if readback.values.dpi != Some(DpiValue { x, y }) {
            return Err(AppError::new(
                ErrorCode::NotConfirmed,
                "OpenRazer non ha confermato i DPI richiesti.",
            ));
        }

        persist_openrazer_config(
            previous.identity.serial.as_deref(),
            path,
            Some(DpiValue { x, y }),
            previous.values.polling_rate,
        );

        Ok(())
    }

    pub(crate) async fn set_polling_rate(previous: &Device, rate: u32) -> AppResult<()> {
        let bus = manager().await?;
        let capability = previous
            .capabilities
            .polling
            .as_ref()
            .filter(|c| c.writable)
            .ok_or_else(|| {
                AppError::new(
                    ErrorCode::Unsupported,
                    "Polling OpenRazer attualmente di sola lettura.",
                )
            })?;
        if !capability.values.contains(&rate) {
            return Err(AppError::new(
                ErrorCode::Validation,
                "Rate non ammesso da OpenRazer per il modello attivo.",
            ));
        }
        let rate_wire = u16::try_from(rate)
            .map_err(|_| AppError::new(ErrorCode::Validation, "Rate oltre UInt16."))?;
        let path = previous
            .identity
            .backend_path
            .as_deref()
            .ok_or_else(|| invalid_data("Percorso OpenRazer assente."))?;
        bus.call::<_, ()>(path, MISC_INTERFACE, "setPollRate", &(rate_wire,))
            .await?;
        let readback = authoritative(&bus, previous).await.map_err(|error| {
            AppError::new(
                ErrorCode::NotConfirmed,
                "Polling inviato ma readback non confermato.",
            )
            .detail(error.to_string())
        })?;
        if readback.values.polling_rate != Some(rate) {
            return Err(AppError::new(
                ErrorCode::NotConfirmed,
                "OpenRazer non ha confermato il polling richiesto.",
            ));
        }
        persist_openrazer_config(
            previous.identity.serial.as_deref(),
            path,
            previous.values.dpi,
            Some(rate),
        );
        Ok(())
    }

    #[allow(dead_code)]
    pub(crate) async fn check_hardware_change(
        device: &Device,
    ) -> Option<(Option<DpiValue>, Option<u32>)> {
        let path = device.identity.backend_path.as_deref()?;
        let bus = manager().await.ok()?;
        let api = bus.introspect(path).await.ok()?;
        let mut new_dpi = None;
        let mut new_rate = None;

        if api.method(DPI_INTERFACE, "getDPIStages", "", "(ya(qq))") {
            if let Ok((active_stage, stages)) = bus
                .call::<_, (u8, Vec<(u16, u16)>)>(path, DPI_INTERFACE, "getDPIStages", &())
                .await
            {
                if !stages.is_empty()
                    && active_stage >= 1
                    && (active_stage as usize) <= stages.len()
                {
                    let stage = stages[(active_stage - 1) as usize];
                    if stage.0 > 0 && stage.1 > 0 {
                        let stage_dpi = DpiValue {
                            x: u32::from(stage.0),
                            y: u32::from(stage.1),
                        };
                        if device.values.dpi != Some(stage_dpi) {
                            new_dpi = Some(stage_dpi);
                        }
                    }
                }
            }
        } else if api.method(DPI_INTERFACE, "getDPI", "", "ai") {
            if let Ok(values) = bus
                .call::<_, Vec<i32>>(path, DPI_INTERFACE, "getDPI", &())
                .await
            {
                if let Ok(dpi) = parse_dpi(&values, false) {
                    if dpi.x > 0 && dpi.y > 0 && device.values.dpi != Some(dpi) {
                        new_dpi = Some(dpi);
                    }
                }
            }
        }

        if api.method(MISC_INTERFACE, "getPollRate", "", "i") {
            if let Ok(val) = bus
                .call::<_, i32>(path, MISC_INTERFACE, "getPollRate", &())
                .await
            {
                if let Ok(rate) = u32::try_from(val) {
                    if rate > 0 && device.values.polling_rate != Some(rate) {
                        new_rate = Some(rate);
                    }
                }
            }
        }

        if new_dpi.is_some() || new_rate.is_some() {
            if let Some(dpi) = new_dpi {
                let x_wire = u16::try_from(dpi.x).unwrap_or(0);
                let y_wire = u16::try_from(dpi.y).unwrap_or(0);
                if api.method(DPI_INTERFACE, "setDPI", "qq", "") {
                    let _ = bus
                        .call::<_, ()>(path, DPI_INTERFACE, "setDPI", &(x_wire, y_wire))
                        .await;
                }
            }
            let effective_dpi = new_dpi.or(device.values.dpi);
            let effective_rate = new_rate.or(device.values.polling_rate);
            persist_openrazer_config(
                device.identity.serial.as_deref(),
                path,
                effective_dpi,
                effective_rate,
            );
            Some((new_dpi, new_rate))
        } else {
            None
        }
    }

    pub fn format_persistence_config(
        existing: &str,
        storage_name: &str,
        serial: Option<&str>,
        dpi: Option<DpiValue>,
        poll_rate: Option<u32>,
    ) -> String {
        let (mut cur_dpi_x, mut cur_dpi_y, mut cur_rate) =
            parse_existing_config(existing, storage_name, serial);
        if let Some(dpi) = dpi {
            cur_dpi_x = Some(dpi.x);
            cur_dpi_y = Some(dpi.y);
        }
        if let Some(rate) = poll_rate {
            cur_rate = Some(rate);
        }

        let dpi_x_val = cur_dpi_x.unwrap_or(1800);
        let dpi_y_val = cur_dpi_y.unwrap_or(1800);
        let rate_val = cur_rate.unwrap_or(1000);

        let mut sections: Vec<(String, Vec<(String, String)>)> = Vec::new();
        let mut section_indices: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        let mut current_sec: Option<String> = None;

        for line in existing.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                let sec = trimmed[1..trimmed.len() - 1].trim().to_string();
                if sec.eq_ignore_ascii_case("DEFAULT") {
                    current_sec = None;
                    continue;
                }
                current_sec = Some(sec.clone());
                if !section_indices.contains_key(&sec) {
                    let idx = sections.len();
                    section_indices.insert(sec.clone(), idx);
                    sections.push((sec, Vec::new()));
                }
            } else if let Some(sec) = &current_sec {
                if let Some((k, v)) = trimmed.split_once('=') {
                    let k = k.trim().to_string();
                    let v = v.trim().to_string();
                    if let Some(&idx) = section_indices.get(sec) {
                        sections[idx].1.push((k, v));
                    }
                }
            }
        }

        let is_target = |sec: &str| -> bool {
            sec == storage_name || serial.is_some_and(|s| s == sec) || sec.starts_with("UNKWN")
        };

        for (sec, entries) in sections.iter_mut() {
            if is_target(sec) {
                entries.retain(|(k, _)| k != "dpi_x" && k != "dpi_y" && k != "poll_rate");
                entries.push(("dpi_x".to_string(), dpi_x_val.to_string()));
                entries.push(("dpi_y".to_string(), dpi_y_val.to_string()));
                entries.push(("poll_rate".to_string(), rate_val.to_string()));
            }
        }

        let mut target_names = Vec::new();
        if !storage_name.is_empty() {
            target_names.push(storage_name.to_string());
        }
        if let Some(s) = serial {
            if !s.is_empty() && !target_names.contains(&s.to_string()) {
                target_names.push(s.to_string());
            }
        }
        for i in 0..=4096 {
            let unkwn_sec = format!("UNKWN{i:012}");
            target_names.push(unkwn_sec);
        }

        for target in target_names {
            if !section_indices.contains_key(&target) {
                let idx = sections.len();
                section_indices.insert(target.clone(), idx);
                sections.push((
                    target,
                    vec![
                        ("dpi_x".to_string(), dpi_x_val.to_string()),
                        ("dpi_y".to_string(), dpi_y_val.to_string()),
                        ("poll_rate".to_string(), rate_val.to_string()),
                    ],
                ));
            }
        }

        let mut lines = Vec::new();
        lines.push("[DEFAULT]".to_string());
        lines.push(format!("dpi_x = {dpi_x_val}"));
        lines.push(format!("dpi_y = {dpi_y_val}"));
        lines.push(format!("poll_rate = {rate_val}"));
        lines.push(String::new());

        for (sec, entries) in sections {
            lines.push(format!("[{sec}]"));
            for (k, v) in entries {
                lines.push(format!("{k} = {v}"));
            }
            lines.push(String::new());
        }

        lines.join("\n")
    }

    pub(crate) fn persist_openrazer_config(
        serial: Option<&str>,
        path_or_storage: &str,
        dpi: Option<DpiValue>,
        poll_rate: Option<u32>,
    ) {
        let storage_name = path_or_storage.rsplit('/').next().unwrap_or("").trim();
        let config_dir = directories::BaseDirs::new()
            .map(|dirs| dirs.config_dir().join("openrazer"))
            .unwrap_or_else(|| std::path::PathBuf::from("/home/shadymui/.config/openrazer"));
        let _ = std::fs::create_dir_all(&config_dir);
        let persistence_path = config_dir.join("persistence.conf");
        let existing = std::fs::read_to_string(&persistence_path).unwrap_or_default();
        let output = format_persistence_config(&existing, storage_name, serial, dpi, poll_rate);
        let _ = std::fs::write(&persistence_path, output);
    }
}
