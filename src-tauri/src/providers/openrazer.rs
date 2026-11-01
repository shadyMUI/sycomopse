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
    let getter = api.method(DPI_INTERFACE, "getDPI", "", "ai");
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
pub(crate) use linux::{discover, set_dpi, set_polling_rate};

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

    async fn manager() -> AppResult<Bus> {
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

        let result: AppResult<Bus> = async {
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
        .await;

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
        let mut current_dpi = None;
        if api.method(DPI_INTERFACE, "getDPI", "", "ai") {
            let result: AppResult<Vec<i32>> = bus.call(&path, DPI_INTERFACE, "getDPI", &()).await;
            match result.and_then(|values| parse_dpi(&values, scalar)) {
                Ok(value) => current_dpi = Some(value),
                Err(error) => diagnostics.push(diagnostic(
                    ProviderKind::Openrazer,
                    format!("{path}:getDPI"),
                    &error,
                )),
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

    async fn authoritative(bus: &Bus, previous: &Device) -> AppResult<Device> {
        bus.ensure_owner(SERVICE).await?;
        let serial = previous
            .identity
            .serial
            .as_deref()
            .and_then(valid_serial)
            .ok_or_else(|| invalid_data("Seriale OpenRazer non verificato."))?;
        if serials(bus).await?.iter().filter(|s| *s == &serial).count() != 1 {
            return Err(AppError::new(
                ErrorCode::DeviceNotFound,
                "Istanza OpenRazer scollegata o ambigua.",
            ));
        }
        let usb = tokio::task::spawn_blocking(crate::devices::read_usb)
            .await
            .map_err(|error| invalid_data("Worker USB interrotto.").detail(error.to_string()))??;
        let (device, _) = read_device(bus, &serial, &usb, false).await?;
        if !identities_match(&previous.identity, &device.identity)
            || device.identity.backend_path != previous.identity.backend_path
            || previous
                .identity
                .physical_path
                .as_ref()
                .is_some_and(|path| device.identity.physical_path.as_ref() != Some(path))
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
        let device = authoritative(&bus, previous).await?;
        let capability = device
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
        let path = device
            .identity
            .backend_path
            .as_deref()
            .ok_or_else(|| invalid_data("Percorso OpenRazer assente."))?;
        bus.call::<_, ()>(path, DPI_INTERFACE, "setDPI", &(x_wire, y_wire))
            .await?;
        let readback = authoritative(&bus, &device).await.map_err(|error| {
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
        Ok(())
    }

    pub(crate) async fn set_polling_rate(previous: &Device, rate: u32) -> AppResult<()> {
        let bus = manager().await?;
        let device = authoritative(&bus, previous).await?;
        let capability = device
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
        let rate = u16::try_from(rate)
            .map_err(|_| AppError::new(ErrorCode::Validation, "Rate oltre UInt16."))?;
        let path = device
            .identity
            .backend_path
            .as_deref()
            .ok_or_else(|| invalid_data("Percorso OpenRazer assente."))?;
        bus.call::<_, ()>(path, MISC_INTERFACE, "setPollRate", &(rate,))
            .await?;
        let readback = authoritative(&bus, &device).await.map_err(|error| {
            AppError::new(
                ErrorCode::NotConfirmed,
                "Polling inviato ma readback non confermato.",
            )
            .detail(error.to_string())
        })?;
        if readback.values.polling_rate != Some(u32::from(rate)) {
            return Err(AppError::new(
                ErrorCode::NotConfirmed,
                "OpenRazer non ha confermato il polling richiesto.",
            ));
        }
        Ok(())
    }
}
