use crate::models::*;
use crate::providers::{hex_id, invalid_data};
use serde::Deserialize;
use std::collections::HashSet;

pub const MANAGER_INTERFACE: &str = "org.freedesktop.ratbag1.Manager";
pub const DEVICE_INTERFACE: &str = "org.freedesktop.ratbag1.Device";
pub const PROFILE_INTERFACE: &str = "org.freedesktop.ratbag1.Profile";
pub const RESOLUTION_INTERFACE: &str = "org.freedesktop.ratbag1.Resolution";

pub fn validate_api_version(version: i32) -> AppResult<()> {
    if version != 2 {
        return Err(AppError::new(
            ErrorCode::Unsupported,
            "ratbagd APIVersion deve essere esattamente 2.",
        )
        .detail(format!("APIVersion={version}")));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    pub transport: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub product_version: u32,
}

pub fn parse_model(model: &str) -> AppResult<Model> {
    let fields: Vec<_> = model.split(':').collect();
    if fields.len() != 4 || model.len() > 128 || !matches!(fields[0], "usb" | "bluetooth") {
        return Err(invalid_data(
            "Model ratbag non verificato: previsto bus:VID:PID:version.",
        ));
    }
    let vendor_id =
        hex_id(fields[1]).ok_or_else(|| invalid_data("Vendor Model ratbag non valido."))?;
    let product_id =
        hex_id(fields[2]).ok_or_else(|| invalid_data("Product Model ratbag non valido."))?;
    if vendor_id != 0x046d {
        return Err(AppError::new(
            ErrorCode::Unsupported,
            "Il Model ratbag non identifica un dispositivo Logitech.",
        ));
    }
    let product_version = fields[3]
        .parse()
        .map_err(|_| invalid_data("Versione prodotto Model ratbag non valida."))?;
    Ok(Model {
        transport: fields[0].to_owned(),
        vendor_id,
        product_id,
        product_version,
    })
}

pub fn unsafe_profile_build(model: &Model) -> bool {
    // Neither APIVersion nor FirmwareVersion proves the daemon's INDEX_OFFSET backport.
    model.product_id == 0xc08b
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionValue {
    Scalar(u32),
    Xy(u32, u32),
}

impl ResolutionValue {
    pub fn from_wire(
        property_signature: &str,
        inner_signature: &str,
        values: &[u32],
    ) -> AppResult<Self> {
        if property_signature != "v" || values.contains(&0) {
            return Err(invalid_data(
                "Resolution ratbag deve preservare la variant della proprieta.",
            ));
        }
        match (inner_signature, values) {
            ("u", [value]) => Ok(Self::Scalar(*value)),
            ("(uu)", [x, y]) => Ok(Self::Xy(*x, *y)),
            _ => Err(invalid_data(
                "Tipo interno Resolution ratbag non scalar UInt32 o coppia UInt32.",
            )),
        }
    }

    pub fn inner_signature(&self) -> &'static str {
        match self {
            Self::Scalar(_) => "u",
            Self::Xy(_, _) => "(uu)",
        }
    }

    pub fn dpi(self) -> DpiValue {
        match self {
            Self::Scalar(value) => DpiValue { x: value, y: value },
            Self::Xy(x, y) => DpiValue { x, y },
        }
    }

    pub fn compatible_capabilities(self, capabilities: &[u32]) -> bool {
        capabilities.contains(&1) == matches!(self, Self::Xy(_, _))
    }

    pub fn with_dpi(
        self,
        x: u32,
        y: u32,
        allowed: &[u32],
        capabilities: &[u32],
    ) -> AppResult<Self> {
        if !self.compatible_capabilities(capabilities) {
            return Err(invalid_data(
                "Capability assi ratbag incoerente con il tipo Resolution.",
            ));
        }
        if !allowed.contains(&x) || !allowed.contains(&y) {
            return Err(AppError::new(
                ErrorCode::Validation,
                "Ogni asse DPI deve appartenere a Resolution.Resolutions.",
            ));
        }
        match self {
            Self::Scalar(_) if x != y => Err(AppError::new(
                ErrorCode::Validation,
                "Il mouse ratbag non espone assi DPI separati.",
            )),
            Self::Scalar(_) => Ok(Self::Scalar(x)),
            Self::Xy(_, _) => Ok(Self::Xy(x, y)),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProfileFlags {
    pub path: String,
    pub is_active: Option<bool>,
    pub is_dirty: Option<bool>,
    pub disabled: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResolutionFlags {
    pub path: String,
    pub is_active: Option<bool>,
    pub disabled: Option<bool>,
}

pub fn active_profile(profiles: &[ProfileFlags]) -> AppResult<usize> {
    let mut paths = HashSet::new();
    if profiles.is_empty()
        || profiles.len() > 64
        || profiles
            .iter()
            .any(|p| p.is_active.is_none() || !paths.insert(&p.path))
    {
        return Err(invalid_data(
            "Profili ratbag mancanti, duplicati o flag IsActive sconosciuti.",
        ));
    }
    let active: Vec<_> = profiles
        .iter()
        .enumerate()
        .filter(|(_, p)| p.is_active == Some(true))
        .collect();
    if active.len() != 1 || active[0].1.disabled == Some(true) {
        return Err(AppError::new(
            ErrorCode::Unsupported,
            "Il provider non espone un unico profilo attivo coerente.",
        ));
    }
    Ok(active[0].0)
}

pub fn active_resolution(resolutions: &[ResolutionFlags]) -> AppResult<usize> {
    let mut paths = HashSet::new();
    if resolutions.is_empty()
        || resolutions.len() > 64
        || resolutions
            .iter()
            .any(|p| p.is_active.is_none() || !paths.insert(&p.path))
    {
        return Err(invalid_data(
            "Resolution ratbag mancanti, duplicate o IsActive sconosciuto.",
        ));
    }
    let active: Vec<_> = resolutions
        .iter()
        .enumerate()
        .filter(|(_, p)| p.is_active == Some(true))
        .collect();
    if active.len() != 1 || active[0].1.disabled == Some(true) {
        return Err(AppError::new(
            ErrorCode::Unsupported,
            "Il provider non espone un'unica resolution attiva coerente.",
        ));
    }
    Ok(active[0].0)
}

pub fn clean_profiles(profiles: &[ProfileFlags]) -> AppResult<()> {
    if profiles.is_empty() || profiles.iter().any(|profile| profile.is_dirty.is_none()) {
        return Err(invalid_data(
            "IsDirty non verificato per tutti i profili: scritture bloccate.",
        ));
    }
    if profiles
        .iter()
        .any(|profile| profile.is_dirty == Some(true))
    {
        return Err(AppError::new(ErrorCode::Busy, "Un profilo ratbag e gia dirty, anche se inattivo. Nessun Commit per modifiche di altri client."));
    }
    Ok(())
}

#[derive(Debug, Clone, Default)]
pub struct CommitEvidence {
    pub staged: bool,
    pub submitted: bool,
    pub queued: bool,
    pub clean_signal: bool,
    pub resync: bool,
    pub conflict: bool,
}

impl CommitEvidence {
    pub fn stage_readback(&mut self, desired_matches: bool, active_dirty: bool, other_dirty: bool) {
        self.staged = desired_matches && active_dirty && !other_dirty;
        self.conflict |= other_dirty;
    }

    pub fn begin_commit(&mut self) {
        self.submitted = true;
    }

    pub fn commit_reply(&mut self, status: u32) {
        self.queued = status == 0;
    }

    pub fn dirty_signal(&mut self, active_profile: bool, dirty: bool) {
        if !active_profile && dirty {
            self.conflict = true;
        }
        if self.staged && !self.submitted && active_profile && !dirty {
            self.conflict = true;
        }
        if self.submitted && active_profile {
            if dirty {
                self.conflict = true;
            } else if self.staged {
                self.clean_signal = true;
            }
        }
    }

    pub fn can_confirm(
        &self,
        desired_readback: bool,
        all_clean: bool,
        active_unchanged: bool,
    ) -> bool {
        self.staged
            && self.submitted
            && self.queued
            && self.clean_signal
            && !self.resync
            && !self.conflict
            && desired_readback
            && all_clean
            && active_unchanged
    }
}

#[cfg(target_os = "linux")]
pub fn decode_resolution(value: &zbus::zvariant::OwnedValue) -> AppResult<ResolutionValue> {
    use zbus::zvariant::Value;
    let Value::Value(inner) = &**value else {
        return Err(invalid_data("Resolution variant appiattita dal provider."));
    };
    match &**inner {
        Value::U32(value) => ResolutionValue::from_wire("v", "u", &[*value]),
        Value::Structure(structure)
            if structure.signature().to_string() == "(uu)" && structure.fields().len() == 2 =>
        {
            let x = u32::try_from(&structure.fields()[0])
                .map_err(|_| invalid_data("DPI X ratbag non UInt32."))?;
            let y = u32::try_from(&structure.fields()[1])
                .map_err(|_| invalid_data("DPI Y ratbag non UInt32."))?;
            ResolutionValue::from_wire("v", "(uu)", &[x, y])
        }
        _ => Err(invalid_data("Resolution ratbag con tipo interno inatteso.")),
    }
}

#[cfg(target_os = "linux")]
pub fn resolution_set_value(resolution: ResolutionValue) -> zbus::zvariant::Value<'static> {
    use zbus::zvariant::Value;
    let inner = match resolution {
        ResolutionValue::Scalar(value) => Value::U32(value),
        ResolutionValue::Xy(x, y) => Value::from((x, y)),
    };
    // Properties.Set's outer v contains the Resolution property's own v, then u/(uu).
    Value::Value(Box::new(inner))
}

#[cfg(target_os = "linux")]
pub(crate) use linux::{discover, set_dpi, set_polling_rate};

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::providers::{
        bounded_text,
        dbus::{self, property, Bus},
        diagnostic, identities_match, ordered_values, Introspection, MAX_DEVICES,
    };
    use std::{collections::HashMap, time::Duration};
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

    const SERVICE: &str = "org.freedesktop.ratbag1";
    const ROOT: &str = "/org/freedesktop/ratbag1";

    struct State {
        device: Device,
        profiles: Vec<ProfileFlags>,
        profile_path: Option<String>,
        resolution_path: Option<String>,
        resolution_paths: Vec<String>,
        guarded_paths: Vec<String>,
        resolution_value: Option<ResolutionValue>,
        resolution_capabilities: Vec<u32>,
        commit_supported: bool,
    }

    async fn manager() -> AppResult<Bus> {
        let bus = Bus::active(SERVICE, false).await.map_err(|error| {
            if error.code == ErrorCode::ProviderMissing {
                AppError::new(ErrorCode::ProviderMissing, "ratbagd non attivo. L'app non lo avvia: il probe del daemon puo cambiare la modalita onboard; avvio manuale con consenso.")
            } else { error }
        })?;
        let api = bus.introspect(ROOT).await?;
        if !api.property(MANAGER_INTERFACE, "APIVersion", "i", false)
            || !api.property(MANAGER_INTERFACE, "Devices", "ao", false)
        {
            return Err(invalid_data("Introspection Manager ratbag incompatibile."));
        }
        let version: i32 = bus.get(ROOT, MANAGER_INTERFACE, "APIVersion").await?;
        validate_api_version(version)?;
        Ok(bus)
    }

    async fn device_paths(bus: &Bus) -> AppResult<Vec<String>> {
        let paths: Vec<OwnedObjectPath> = bus.get(ROOT, MANAGER_INTERFACE, "Devices").await?;
        let mut seen = HashSet::new();
        if paths.len() > MAX_DEVICES
            || paths.iter().any(|path| {
                !path
                    .as_str()
                    .starts_with("/org/freedesktop/ratbag1/device/")
                    || !seen.insert(path.as_str().to_owned())
            })
        {
            return Err(invalid_data(
                "Manager.Devices ratbag contiene percorsi duplicati o inattesi.",
            ));
        }
        Ok(paths
            .into_iter()
            .map(|path| path.as_str().to_owned())
            .collect())
    }

    async fn device_metadata(
        bus: &Bus,
        path: &str,
    ) -> AppResult<(Device, Model, Vec<String>, Introspection)> {
        let api = bus.introspect(path).await?;
        if !api.property(DEVICE_INTERFACE, "Model", "s", false)
            || !api.property(DEVICE_INTERFACE, "DeviceType", "u", false)
            || !api.property(DEVICE_INTERFACE, "Profiles", "ao", false)
        {
            return Err(invalid_data("Introspection Device ratbag incompatibile."));
        }
        let properties = bus.get_all(path, DEVICE_INTERFACE).await?;
        let device_type: u32 = property(&properties, "DeviceType")?;
        if device_type != 2 {
            return Err(AppError::new(
                ErrorCode::Unsupported,
                "DeviceType ratbag non e Mouse=2.",
            ));
        }
        let model = parse_model(&property::<String>(&properties, "Model")?)?;
        let profiles: Vec<OwnedObjectPath> = property(&properties, "Profiles")?;
        if profiles.is_empty() || profiles.len() > 64 {
            return Err(invalid_data("Numero profili ratbag non valido."));
        }
        let model_identity = model.clone();
        let path_owned = path.to_owned();
        let mut identity = tokio::task::spawn_blocking(move || {
            crate::devices::ratbag_identity(
                &path_owned,
                model_identity.vendor_id,
                model_identity.product_id,
            )
        })
        .await
        .map_err(|error| {
            invalid_data("Worker identita ratbag interrotto.").detail(error.to_string())
        })?;
        identity.transport = Some(model.transport.clone());
        identity.firmware = property::<String>(&properties, "FirmwareVersion")
            .ok()
            .map(|value| bounded_text(&value));
        let name = property::<String>(&properties, "Name")
            .ok()
            .filter(|name| !name.trim().is_empty())
            .map(|name| bounded_text(&name))
            .unwrap_or_else(|| format!("Mouse Logitech {:04x}", model.product_id));
        let mut limitations = vec!["Proprieta ratbag sono cache del daemon: Commit=0 indica solo accodamento.".to_owned(),
            "Unico proprietario Logitech: ratbagd gia attivo; nessun Refresh, rollback condiviso, auto-retry o policy polkit aggiunta.".to_owned()];
        if matches!(model.product_id, 0x4074 | 0x4093) {
            limitations.push("PID logico HID++ del mouse associato, non PID USB del receiver; trasporto verificato separatamente.".to_owned());
        }
        if unsafe_profile_build(&model) {
            limitations.push("G502 HERO: build/backport INDEX_OFFSET 874c0173 non verificabile tramite queste API. Scritture bloccate, senza presumere la correzione upstream master.".to_owned());
        }
        Ok((
            Device {
                id: format!("ratbag:{path}"),
                name,
                brand: Brand::Logitech,
                kind: DeviceKind::Mouse,
                provider: ProviderKind::Ratbag,
                identity,
                availability: AvailabilityCode::Available,
                support: SupportLevel::Partial,
                capabilities: DeviceCapabilities::default(),
                values: DeviceValues::default(),
                output_node_names: Vec::new(),
                limitations,
            },
            model,
            profiles
                .into_iter()
                .map(|path| path.as_str().to_owned())
                .collect(),
            api,
        ))
    }

    async fn read_state(
        bus: &Bus,
        metadata: (Device, Model, Vec<String>, Introspection),
    ) -> AppResult<(State, Vec<Diagnostic>)> {
        let (device, model, profile_paths, api) = metadata;
        let mut diagnostics = Vec::new();
        let mut profiles = Vec::new();
        let mut profile_properties = Vec::new();
        let mut profile_apis = Vec::new();
        let mut blocked = None;
        let mut guarded_paths = profile_paths.clone();
        for path in &profile_paths {
            if !path.starts_with("/org/freedesktop/ratbag1/profile/") {
                return Err(invalid_data("Percorso Profile ratbag inatteso."));
            }
            let result = async {
                let api = bus.introspect(path).await?;
                if !api.property(PROFILE_INTERFACE, "IsActive", "b", false)
                    || !api.property(PROFILE_INTERFACE, "IsDirty", "b", false)
                    || !api.property(PROFILE_INTERFACE, "Resolutions", "ao", false)
                {
                    return Err(invalid_data("Introspection Profile ratbag incompatibile."));
                }
                Ok((api, bus.get_all(path, PROFILE_INTERFACE).await?))
            }
            .await;
            match result {
                Ok((api, properties)) => {
                    for name in ["Resolutions", "Buttons", "Leds"] {
                        if api.property(PROFILE_INTERFACE, name, "ao", false) {
                            match property::<Vec<OwnedObjectPath>>(&properties, name) {
                                Ok(paths) if paths.len() <= 512 && paths.iter().all(|path| path.as_str().starts_with("/org/freedesktop/ratbag1/")) => {
                                    guarded_paths.extend(paths.into_iter().map(|path| path.as_str().to_owned()));
                                }
                                _ => blocked = Some("Oggetti del profilo non verificati: impossibile escludere scritture concorrenti.".to_owned()),
                            }
                        }
                    }
                    if api.property(PROFILE_INTERFACE, "Disabled", "b", false)
                        && property::<bool>(&properties, "Disabled").is_err()
                    {
                        blocked = Some("Flag Disabled del profilo non verificato.".to_owned());
                    }
                    profiles.push(ProfileFlags {
                        path: path.clone(),
                        is_active: property(&properties, "IsActive").ok(),
                        is_dirty: property(&properties, "IsDirty").ok(),
                        disabled: if api.property(PROFILE_INTERFACE, "Disabled", "b", false) {
                            property(&properties, "Disabled").ok()
                        } else {
                            None
                        },
                    });
                    profile_properties.push(properties);
                    profile_apis.push(api);
                }
                Err(error) => {
                    diagnostics.push(diagnostic(ProviderKind::Ratbag, path, &error));
                    profiles.push(ProfileFlags {
                        path: path.clone(),
                        is_active: None,
                        is_dirty: None,
                        disabled: None,
                    });
                    profile_properties.push(HashMap::new());
                    profile_apis.push(Introspection::default());
                }
            }
        }
        if let Err(error) = clean_profiles(&profiles) {
            blocked = Some(error.message);
        }
        let profile_index = match active_profile(&profiles) {
            Ok(index) => Some(index),
            Err(error) => {
                diagnostics.push(diagnostic(
                    ProviderKind::Ratbag,
                    format!("{}:active-profile", device.id),
                    &error,
                ));
                blocked = Some(error.message);
                None
            }
        };
        let commit_supported = api.method(DEVICE_INTERFACE, "Commit", "", "u")
            && api.signal(DEVICE_INTERFACE, "Resync", "");
        if !commit_supported {
            blocked =
                Some("Commit/Resync non verificati nell'introspection installata.".to_owned());
        }
        if device.identity.serial.is_none() && device.identity.physical_path.is_none() {
            blocked = Some(
                "Istanza ratbag senza seriale o percorso sysfs verificato: sola lettura."
                    .to_owned(),
            );
        }
        if unsafe_profile_build(&model) {
            blocked =
                Some("G502 HERO: build con correzione profilo attivo non verificata.".to_owned());
        }
        let mut state = State {
            device,
            profiles,
            profile_path: profile_index.map(|index| profile_paths[index].clone()),
            resolution_path: None,
            resolution_paths: Vec::new(),
            guarded_paths,
            resolution_value: None,
            resolution_capabilities: Vec::new(),
            commit_supported,
        };
        let mut dpi_values = DpiValues::Unknown;
        let mut dpi_reason = None;
        let mut polling_values = Vec::new();
        let mut polling_reason = None;
        let mut polling_writable = false;
        let mut dpi_writable = false;
        if let Some(profile_index) = profile_index {
            let profile_api = &profile_apis[profile_index];
            let profile = &profile_properties[profile_index];
            let polling = (|| -> AppResult<(Vec<u32>, u32)> {
                if !profile_api.property(PROFILE_INTERFACE, "ReportRates", "au", false)
                    || !profile_api.property(PROFILE_INTERFACE, "ReportRate", "u", false)
                {
                    return Err(invalid_data("ReportRates/ReportRate non verificati."));
                }
                Ok((
                    ordered_values(property(profile, "ReportRates")?, 32)?,
                    property(profile, "ReportRate")?,
                ))
            })();
            match polling {
                Ok((rates, rate)) => {
                    polling_writable =
                        profile_api.property(PROFILE_INTERFACE, "ReportRate", "u", true)
                            && rates.contains(&rate);
                    polling_values = rates;
                    state.device.values.polling_rate = (rate > 0).then_some(rate);
                    if !polling_writable {
                        polling_reason = Some(
                            "ReportRate corrente non valido o proprieta non scrivibile.".to_owned(),
                        );
                    }
                }
                Err(error) => {
                    diagnostics.push(diagnostic(
                        ProviderKind::Ratbag,
                        format!("{}:polling", state.device.id),
                        &error,
                    ));
                    polling_reason = Some(error.message);
                }
            }
            let resolution_paths: AppResult<Vec<OwnedObjectPath>> =
                property(profile, "Resolutions");
            let resolution_result = async {
                let paths = resolution_paths?;
                if paths.is_empty() || paths.len() > 64 {
                    return Err(invalid_data("Numero Resolution ratbag non valido."));
                }
                let mut flags = Vec::new();
                let mut properties = Vec::new();
                let mut apis = Vec::new();
                for path in paths {
                    let path = path.as_str();
                    if !path.starts_with("/org/freedesktop/ratbag1/resolution/") {
                        return Err(invalid_data("Percorso Resolution ratbag inatteso."));
                    }
                    let api = bus.introspect(path).await?;
                    if !api.property(RESOLUTION_INTERFACE, "IsActive", "b", false) {
                        return Err(invalid_data("Resolution.IsActive non verificato."));
                    }
                    let values = bus.get_all(path, RESOLUTION_INTERFACE).await?;
                    if api.property(RESOLUTION_INTERFACE, "IsDisabled", "b", false) {
                        let _: bool = property(&values, "IsDisabled")?;
                    }
                    flags.push(ResolutionFlags {
                        path: path.to_owned(),
                        is_active: property(&values, "IsActive").ok(),
                        disabled: if api.property(RESOLUTION_INTERFACE, "IsDisabled", "b", false) {
                            property(&values, "IsDisabled").ok()
                        } else {
                            None
                        },
                    });
                    properties.push(values);
                    apis.push(api);
                }
                let index = active_resolution(&flags)?;
                state.resolution_paths = flags.iter().map(|flag| flag.path.clone()).collect();
                state.resolution_path = Some(flags[index].path.clone());
                let api = &apis[index];
                let properties = &properties[index];
                if !api.property(RESOLUTION_INTERFACE, "Resolution", "v", false)
                    || !api.property(RESOLUTION_INTERFACE, "Resolutions", "au", false)
                    || !api.property(RESOLUTION_INTERFACE, "Capabilities", "au", false)
                {
                    return Err(invalid_data(
                        "Introspection Resolution ratbag incompatibile.",
                    ));
                }
                let resolution = decode_resolution(
                    properties
                        .get("Resolution")
                        .ok_or_else(|| invalid_data("Resolution absente."))?,
                )?;
                let capabilities: Vec<u32> = property(properties, "Capabilities")?;
                if capabilities.len() > 32 || !resolution.compatible_capabilities(&capabilities) {
                    return Err(invalid_data(
                        "Capabilities e tipo scalar/XY ratbag incoerenti.",
                    ));
                }
                let allowed = ordered_values(property(properties, "Resolutions")?, 300)?;
                let current = resolution.dpi();
                let current_valid = allowed.contains(&current.x) && allowed.contains(&current.y);
                dpi_writable = api.property(RESOLUTION_INTERFACE, "Resolution", "v", true);
                state.device.values.dpi = Some(current);
                state.resolution_value = Some(resolution);
                state.resolution_capabilities = capabilities;
                dpi_values = DpiValues::Discrete { values: allowed };
                if !dpi_writable {
                    dpi_reason =
                        Some("Resolution non scrivibile.".to_owned());
                } else if !current_valid {
                    dpi_reason = Some("DPI corrente fuori elenco: scegliere un valore verificato prima di Applica.".to_owned());
                }
                Ok::<(), AppError>(())
            }
            .await;
            if let Err(error) = resolution_result {
                diagnostics.push(diagnostic(
                    ProviderKind::Ratbag,
                    format!("{}:dpi", state.device.id),
                    &error,
                ));
                dpi_reason = Some(error.message.clone());
                if state.resolution_path.is_none() {
                    blocked = Some(error.message);
                }
            }
        }
        if let Some(reason) = &blocked {
            dpi_reason = Some(reason.clone());
            polling_reason = Some(reason.clone());
            dpi_writable = false;
            polling_writable = false;
        }
        state.device.capabilities = DeviceCapabilities {
            dpi: Some(DpiCapability {
                values: dpi_values,
                writable: dpi_writable,
                separate_axes: state
                    .resolution_value
                    .is_some_and(|value| matches!(value, ResolutionValue::Xy(_, _))),
                reason: dpi_reason,
            }),
            polling: Some(PollingCapability {
                values: polling_values,
                writable: polling_writable,
                reason: polling_reason,
            }),
            remapping: None,
        };
        state.device.values.staged = state
            .profiles
            .iter()
            .any(|profile| profile.is_dirty != Some(false));
        state.device.support = if dpi_writable && polling_writable {
            SupportLevel::Supported
        } else {
            SupportLevel::Partial
        };
        Ok((state, diagnostics))
    }

    pub(crate) async fn discover() -> Discovery {
        let mut discovery = Discovery::default();
        let bus = match manager().await {
            Ok(bus) => bus,
            Err(error) => {
                discovery
                    .diagnostics
                    .push(diagnostic(ProviderKind::Ratbag, "ratbag", &error));
                return discovery;
            }
        };
        let paths = match device_paths(&bus).await {
            Ok(paths) => paths,
            Err(error) => {
                discovery.diagnostics.push(diagnostic(
                    ProviderKind::Ratbag,
                    "ratbag-devices",
                    &error,
                ));
                return discovery;
            }
        };
        for path in paths {
            let result = match device_metadata(&bus, &path).await {
                Ok(metadata) => read_state(&bus, metadata).await,
                Err(error) => Err(error),
            };
            match result {
                Ok((state, diagnostics)) => {
                    discovery.devices.push(state.device);
                    discovery.diagnostics.extend(diagnostics);
                }
                Err(error) => {
                    discovery
                        .diagnostics
                        .push(diagnostic(ProviderKind::Ratbag, path, &error))
                }
            }
        }
        discovery
    }

    async fn resolve(bus: &Bus, previous: &Device) -> AppResult<State> {
        let mut candidates = Vec::new();
        for path in device_paths(bus).await? {
            if let Ok(metadata) = device_metadata(bus, &path).await {
                if identities_match(&previous.identity, &metadata.0.identity) {
                    candidates.push(metadata);
                }
            }
        }
        if candidates.len() != 1 {
            return Err(AppError::new(ErrorCode::DeviceNotFound, "Istanza ratbag scollegata, riciclata o ambigua; Model/nome da soli non sono identita."));
        }
        let (state, _) = read_state(
            bus,
            candidates
                .pop()
                .ok_or_else(|| invalid_data("Istanza ratbag assente."))?,
        )
        .await?;
        Ok(state)
    }

    #[derive(Debug, Clone, Copy)]
    enum Mutation {
        Dpi(ResolutionValue),
        Polling(u32),
    }

    impl Mutation {
        fn matches(self, state: &State) -> bool {
            match self {
                Self::Dpi(value) => state.resolution_value == Some(value),
                Self::Polling(rate) => state.device.values.polling_rate == Some(rate),
            }
        }
    }

    #[derive(Debug)]
    enum Observed {
        Dirty(Option<bool>),
        Resolution(Option<ResolutionValue>),
        Rate(Option<u32>),
        Other,
    }

    #[derive(Debug)]
    enum Event {
        Resync(String),
        Property {
            path: String,
            interface: String,
            name: String,
            value: Observed,
        },
        Invalid,
        OwnerChanged,
    }

    fn signal_events(message: &zbus::Message, owner: &str) -> Vec<Event> {
        let header = message.header();
        let path = header.path().map(|path| path.as_str()).unwrap_or_default();
        let interface = header
            .interface()
            .map(|interface| interface.as_str())
            .unwrap_or_default();
        let member = header
            .member()
            .map(|member| member.as_str())
            .unwrap_or_default();
        if interface == "org.freedesktop.DBus" && member == "NameOwnerChanged" {
            return match message.body().deserialize::<(String, String, String)>() {
                Ok((name, _, new)) if name == SERVICE && new != owner => vec![Event::OwnerChanged],
                Ok(_) => Vec::new(),
                Err(_) => vec![Event::Invalid],
            };
        }
        if interface == DEVICE_INTERFACE && member == "Resync" {
            return if message.body().deserialize::<()>().is_ok() {
                vec![Event::Resync(path.to_owned())]
            } else {
                vec![Event::Invalid]
            };
        }
        if interface != dbus::PROPERTIES || member != "PropertiesChanged" {
            return Vec::new();
        }
        let Ok((interface, changed, invalidated)) =
            message
                .body()
                .deserialize::<(String, HashMap<String, OwnedValue>, Vec<String>)>()
        else {
            return vec![Event::Invalid];
        };
        if path.len() > 4096
            || interface.len() > 255
            || changed.len() > 128
            || invalidated.len() > 128
        {
            return vec![Event::Invalid];
        }
        let mut events = Vec::new();
        for (name, value) in changed {
            if name.len() > 255 {
                return vec![Event::Invalid];
            }
            let value = match name.as_str() {
                "IsDirty" => Observed::Dirty(bool::try_from(&value).ok()),
                "Resolution" => Observed::Resolution(decode_resolution(&value).ok()),
                "ReportRate" => Observed::Rate(u32::try_from(&value).ok()),
                _ => Observed::Other,
            };
            events.push(Event::Property {
                path: path.to_owned(),
                interface: interface.clone(),
                name,
                value,
            });
        }
        for name in invalidated {
            events.push(Event::Property {
                path: path.to_owned(),
                interface: interface.clone(),
                name,
                value: Observed::Other,
            });
        }
        events
    }

    fn observe(evidence: &mut CommitEvidence, state: &State, mutation: Mutation, event: Event) {
        match event {
            Event::Invalid | Event::OwnerChanged => evidence.conflict = true,
            Event::Resync(path) => {
                if Some(path.as_str()) == state.device.identity.backend_path.as_deref() {
                    evidence.resync = true;
                }
            }
            Event::Property {
                path,
                interface,
                name,
                value,
            } => {
                let profile = state.profiles.iter().any(|profile| profile.path == path);
                let resolution = state.resolution_paths.contains(&path);
                if !state.guarded_paths.contains(&path) {
                    return;
                }
                if !profile && !resolution {
                    evidence.conflict = true;
                    return;
                }
                let active_profile = Some(path.as_str()) == state.profile_path.as_deref();
                let active_resolution = Some(path.as_str()) == state.resolution_path.as_deref();
                if interface == PROFILE_INTERFACE && profile && name == "IsDirty" {
                    if let Observed::Dirty(Some(dirty)) = value {
                        evidence.dirty_signal(active_profile, dirty);
                    } else {
                        evidence.conflict = true;
                    }
                    return;
                }
                let expected = match (mutation, interface.as_str(), name.as_str(), value) {
                    (
                        Mutation::Dpi(desired),
                        RESOLUTION_INTERFACE,
                        "Resolution",
                        Observed::Resolution(Some(value)),
                    ) => active_resolution && desired == value,
                    (
                        Mutation::Polling(desired),
                        PROFILE_INTERFACE,
                        "ReportRate",
                        Observed::Rate(Some(value)),
                    ) => active_profile && desired == value,
                    _ => false,
                };
                if !expected {
                    evidence.conflict = true;
                }
            }
        }
    }

    struct Monitor {
        events: tokio::sync::mpsc::Receiver<Event>,
        flush_sender: tokio::sync::mpsc::UnboundedSender<tokio::sync::oneshot::Sender<()>>,
        overflow: std::sync::Arc<std::sync::atomic::AtomicBool>,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for Monitor {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn next_signal(
        streams: &mut [zbus::MessageStream],
    ) -> Option<zbus::Result<zbus::Message>> {
        std::future::poll_fn(|cx| {
            for stream in &mut *streams {
                let result =
                    zbus::export::futures_core::Stream::poll_next(std::pin::Pin::new(stream), cx);
                if result.is_ready() {
                    return result;
                }
            }
            std::task::Poll::Pending
        })
        .await
    }

    impl Monitor {
        async fn subscribe(bus: &Bus) -> AppResult<Self> {
            use std::sync::{
                atomic::{AtomicBool, Ordering},
                Arc,
            };
            let signals = zbus::MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .sender(bus.owner.as_str())
                .map_err(dbus::error)?
                .path_namespace(ROOT)
                .map_err(dbus::error)?
                .build()
                .to_owned();
            let owners = zbus::MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .sender("org.freedesktop.DBus")
                .map_err(dbus::error)?
                .interface("org.freedesktop.DBus")
                .map_err(dbus::error)?
                .member("NameOwnerChanged")
                .map_err(dbus::error)?
                .arg(0, SERVICE)
                .map_err(dbus::error)?
                .build()
                .to_owned();
            let mut streams = Vec::new();
            for rule in [signals, owners] {
                let stream = tokio::time::timeout(
                    dbus::CALL_TIMEOUT,
                    zbus::MessageStream::for_match_rule(rule, &bus.connection, Some(512)),
                )
                .await
                .map_err(|_| {
                    AppError::new(ErrorCode::Timeout, "Sottoscrizione segnali ratbag scaduta.")
                })?
                .map_err(dbus::error)?;
                streams.push(stream);
            }
            let (sender, events) = tokio::sync::mpsc::channel(512);
            let (flush_sender, mut flush_receiver) =
                tokio::sync::mpsc::unbounded_channel::<tokio::sync::oneshot::Sender<()>>();
            let overflow = Arc::new(AtomicBool::new(false));
            let failed = overflow.clone();
            let owner = bus.owner.clone();
            let task = tokio::spawn(async move {
                enum Action {
                    Message(Option<zbus::Result<zbus::Message>>),
                    Flush(Option<tokio::sync::oneshot::Sender<()>>),
                }
                let emit = |message: zbus::Result<zbus::Message>| match message {
                    Ok(message) => {
                        for event in signal_events(&message, &owner) {
                            if sender.try_send(event).is_err() {
                                failed.store(true, Ordering::Release);
                            }
                        }
                    }
                    Err(_) => {
                        failed.store(true, Ordering::Release);
                        let _ = sender.try_send(Event::Invalid);
                    }
                };
                loop {
                    let action = tokio::select! {
                        message = next_signal(&mut streams) => Action::Message(message),
                        flush = flush_receiver.recv() => Action::Flush(flush),
                    };
                    match action {
                        Action::Message(Some(message)) => emit(message),
                        Action::Message(None) | Action::Flush(None) => {
                            failed.store(true, Ordering::Release);
                            return;
                        }
                        Action::Flush(Some(ack)) => {
                            // Replies and signals share one socket-reader order. Once a Get reply
                            // has arrived, draining ready signals catches every preceding Resync.
                            for stream in &mut streams {
                                let mut count = 0;
                                loop {
                                    let message = std::future::poll_fn(|cx| {
                                        std::task::Poll::Ready(
                                            match zbus::export::futures_core::Stream::poll_next(
                                                std::pin::Pin::new(&mut *stream),
                                                cx,
                                            ) {
                                                std::task::Poll::Ready(message) => message,
                                                std::task::Poll::Pending => None,
                                            },
                                        )
                                    })
                                    .await;
                                    let Some(message) = message else {
                                        break;
                                    };
                                    emit(message);
                                    count += 1;
                                    if count >= 512 {
                                        failed.store(true, Ordering::Release);
                                        break;
                                    }
                                }
                            }
                            let _ = ack.send(());
                        }
                    }
                }
            });
            Ok(Self {
                events,
                flush_sender,
                overflow,
                task,
            })
        }

        async fn flush(&mut self) -> AppResult<Vec<Event>> {
            let (sender, receiver) = tokio::sync::oneshot::channel();
            self.flush_sender
                .send(sender)
                .map_err(|_| not_confirmed("Monitor segnali ratbag interrotto."))?;
            tokio::time::timeout(dbus::CALL_TIMEOUT, receiver)
                .await
                .map_err(|_| not_confirmed("Barriera segnali ratbag scaduta."))?
                .map_err(|_| not_confirmed("Monitor segnali ratbag interrotto."))?;
            if self.overflow.load(std::sync::atomic::Ordering::Acquire) {
                return Err(not_confirmed(
                    "Segnali persi o bus disconnesso: nessuna conferma hardware.",
                ));
            }
            let mut events = Vec::new();
            while let Ok(event) = self.events.try_recv() {
                events.push(event);
            }
            Ok(events)
        }
    }

    fn not_confirmed(message: &str) -> AppError {
        AppError::new(ErrorCode::NotConfirmed, message)
    }

    fn same_active(a: &State, b: &State) -> bool {
        identities_match(&a.device.identity, &b.device.identity)
            && a.profile_path == b.profile_path
            && a.resolution_path == b.resolution_path
            && a.resolution_capabilities == b.resolution_capabilities
            && a.resolution_paths == b.resolution_paths
            && a.guarded_paths == b.guarded_paths
            && a.device.capabilities.dpi.as_ref().map(|c| &c.values)
                == b.device.capabilities.dpi.as_ref().map(|c| &c.values)
            && a.device.capabilities.polling.as_ref().map(|c| &c.values)
                == b.device.capabilities.polling.as_ref().map(|c| &c.values)
            && a.profiles
                .iter()
                .map(|p| (&p.path, p.is_active, p.disabled))
                .eq(b
                    .profiles
                    .iter()
                    .map(|p| (&p.path, p.is_active, p.disabled)))
    }

    async fn transact(bus: &Bus, state: State, mutation: Mutation) -> AppResult<()> {
        clean_profiles(&state.profiles)?;
        if !state.commit_supported {
            return Err(AppError::new(
                ErrorCode::Unsupported,
                "Commit/Resync ratbag non verificati.",
            ));
        }
        let mut monitor = Monitor::subscribe(bus).await?;
        let mut evidence = CommitEvidence::default();
        for event in monitor.flush().await? {
            observe(&mut evidence, &state, mutation, event);
        }
        bus.ensure_owner(SERVICE).await?;
        let current = resolve(bus, &state.device).await?;
        clean_profiles(&current.profiles)?;
        if !same_active(&state, &current)
            || state.device.values.dpi != current.device.values.dpi
            || state.device.values.polling_rate != current.device.values.polling_rate
            || evidence.conflict
            || evidence.resync
        {
            return Err(AppError::new(
                ErrorCode::Busy,
                "Stato ratbag modificato prima dello staging; nessuna scrittura eseguita.",
            ));
        }
        let (path, interface, name, value) = match mutation {
            Mutation::Dpi(value) => (
                state.resolution_path.as_deref(),
                RESOLUTION_INTERFACE,
                "Resolution",
                resolution_set_value(value),
            ),
            Mutation::Polling(rate) => (
                state.profile_path.as_deref(),
                PROFILE_INTERFACE,
                "ReportRate",
                Value::U32(rate),
            ),
        };
        let path = path.ok_or_else(|| invalid_data("Percorso attivo ratbag assente."))?;
        bus.call::<_, ()>(path, dbus::PROPERTIES, "Set", &(interface, name, value))
            .await?;
        let staged = resolve(bus, &state.device).await.map_err(|error| {
            not_confirmed("Staging ratbag inviato ma readback non disponibile.")
                .detail(error.to_string())
        })?;
        let active_dirty = staged
            .profiles
            .iter()
            .find(|profile| Some(profile.path.as_str()) == staged.profile_path.as_deref())
            .is_some_and(|profile| profile.is_dirty == Some(true));
        let other_dirty = staged.profiles.iter().any(|profile| {
            profile.is_dirty.is_none()
                || (Some(profile.path.as_str()) != staged.profile_path.as_deref()
                    && profile.is_dirty == Some(true))
        });
        evidence.stage_readback(mutation.matches(&staged), active_dirty, other_dirty);
        for event in monitor.flush().await? {
            observe(&mut evidence, &state, mutation, event);
        }
        if !evidence.staged
            || !same_active(&state, &staged)
            || !unchanged_other_value(&state, &staged, mutation)
            || evidence.conflict
            || evidence.resync
        {
            return Err(not_confirmed("Staging ratbag non verificato o stato concorrente. Nessun Commit/rollback/auto-retry."));
        }
        bus.ensure_owner(SERVICE).await.map_err(|error| {
            not_confirmed("Provider cambiato dopo lo staging.").detail(error.to_string())
        })?;
        evidence.begin_commit();
        let device_path = state
            .device
            .identity
            .backend_path
            .as_deref()
            .ok_or_else(|| invalid_data("Percorso Device ratbag assente."))?;
        let status: u32 = bus
            .call(device_path, DEVICE_INTERFACE, "Commit", &())
            .await
            .map_err(|error| {
                not_confirmed("Commit ratbag inviato ma accodamento non confermato.")
                    .detail(error.to_string())
            })?;
        evidence.commit_reply(status);
        if !evidence.queued {
            return Err(not_confirmed(
                "ratbagd non ha accodato il Commit. Nessun retry automatico.",
            ));
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            for event in monitor.flush().await? {
                observe(&mut evidence, &state, mutation, event);
            }
            if evidence.resync || evidence.conflict {
                return Err(not_confirmed("Resync o modifica concorrente durante Commit: risultato non confermato, nessun rollback promesso."));
            }
            if evidence.clean_signal {
                let readback = resolve(bus, &state.device).await.map_err(|error| {
                    not_confirmed(
                        "Commit completato dal daemon ma readback finale non disponibile.",
                    )
                    .detail(error.to_string())
                })?;
                bus.ensure_owner(SERVICE).await.map_err(|error| {
                    not_confirmed("Provider cambiato durante Commit.").detail(error.to_string())
                })?;
                for event in monitor.flush().await? {
                    observe(&mut evidence, &state, mutation, event);
                }
                if evidence.can_confirm(
                    mutation.matches(&readback),
                    clean_profiles(&readback.profiles).is_ok(),
                    same_active(&state, &readback)
                        && unchanged_other_value(&state, &readback, mutation),
                ) {
                    return Ok(());
                }
                return Err(not_confirmed("Segnale di fine Commit senza readback/stato coerenti. La cache desiderata non e una conferma."));
            }
            let event = tokio::time::timeout_at(deadline, monitor.events.recv()).await
                .map_err(|_| not_confirmed("Commit solo accodato: transizione dirty->clean non osservata entro il timeout."))?
                .ok_or_else(|| not_confirmed("Monitor ratbag terminato prima della conferma."))?;
            observe(&mut evidence, &state, mutation, event);
        }
    }

    fn unchanged_other_value(before: &State, after: &State, mutation: Mutation) -> bool {
        match mutation {
            Mutation::Dpi(_) => {
                before.device.values.polling_rate == after.device.values.polling_rate
            }
            Mutation::Polling(_) => before.resolution_value == after.resolution_value,
        }
    }

    pub(crate) async fn set_dpi(previous: &Device, x: u32, y: u32) -> AppResult<()> {
        let bus = manager().await?;
        let state = resolve(&bus, previous).await?;
        clean_profiles(&state.profiles)?;
        let capability = state
            .device
            .capabilities
            .dpi
            .as_ref()
            .filter(|c| c.writable)
            .ok_or_else(|| {
                AppError::new(
                    ErrorCode::Unsupported,
                    "DPI ratbag di sola lettura per lo stato/build attivi.",
                )
            })?;
        let DpiValues::Discrete { values } = &capability.values else {
            return Err(invalid_data("Lista DPI ratbag non verificata."));
        };
        let current = state
            .resolution_value
            .ok_or_else(|| invalid_data("Tipo Resolution ratbag non verificato."))?;
        let desired = current.with_dpi(x, y, values, &state.resolution_capabilities)?;
        transact(&bus, state, Mutation::Dpi(desired)).await
    }

    pub(crate) async fn set_polling_rate(previous: &Device, rate: u32) -> AppResult<()> {
        let bus = manager().await?;
        let state = resolve(&bus, previous).await?;
        clean_profiles(&state.profiles)?;
        let capability = state
            .device
            .capabilities
            .polling
            .as_ref()
            .filter(|c| c.writable)
            .ok_or_else(|| {
                AppError::new(
                    ErrorCode::Unsupported,
                    "Polling ratbag di sola lettura per lo stato/build attivi.",
                )
            })?;
        if !capability.values.contains(&rate) {
            return Err(AppError::new(
                ErrorCode::Validation,
                "Rate non presente in Profile.ReportRates.",
            ));
        }
        transact(&bus, state, Mutation::Polling(rate)).await
    }
}
