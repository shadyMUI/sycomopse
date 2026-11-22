pub mod headsetcontrol;
pub mod openrazer;
pub mod ratbag;
pub mod upower;

use crate::models::{
    AppError, AppResult, AvailabilityCode, DeviceIdentity, Diagnostic, ErrorCode, ProviderKind,
    Severity,
};
use std::collections::BTreeMap;

pub const MAX_PROVIDER_BYTES: usize = 256 * 1024;
pub const MAX_DEVICES: usize = 256;

pub fn invalid_data(message: &str) -> AppError {
    AppError::new(ErrorCode::InvalidData, message)
}

pub fn bounded_text(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(256)
        .collect()
}

pub fn valid_serial(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 256
        || value.chars().any(char::is_control)
        || value.chars().all(|c| c == '0' || c == ':' || c == '-')
        || matches!(
            value.to_ascii_lowercase().as_str(),
            "unknown" | "none" | "n/a"
        )
    {
        None
    } else {
        Some(value.to_owned())
    }
}

pub fn hex_id(value: &str) -> Option<u16> {
    let value = value.trim();
    let value = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    if value.is_empty() || value.len() > 4 || !value.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u16::from_str_radix(value, 16).ok()
}

pub fn identities_match(a: &DeviceIdentity, b: &DeviceIdentity) -> bool {
    if a.vendor_id.zip(b.vendor_id).is_some_and(|(a, b)| a != b)
        || a.product_id.zip(b.product_id).is_some_and(|(a, b)| a != b)
    {
        return false;
    }
    let a_serial = a.serial.as_deref().and_then(valid_serial);
    let b_serial = b.serial.as_deref().and_then(valid_serial);
    let a_dummy = a_serial.as_ref().is_some_and(|s| s.starts_with("UNKWN"));
    let b_dummy = b_serial.as_ref().is_some_and(|s| s.starts_with("UNKWN"));
    if !a_dummy
        && !b_dummy
        && a_serial
            .as_ref()
            .zip(b_serial.as_ref())
            .is_some_and(|(a, b)| a != b)
    {
        return false;
    }
    let physical_match = a
        .physical_path
        .as_deref()
        .zip(b.physical_path.as_deref())
        .map(|(a, b)| {
            a.starts_with("/sys/devices/")
                && a == b
                && !a.split('/').any(|p| matches!(p, "." | ".."))
        });
    match physical_match {
        Some(matches) => matches,
        None => {
            if a_dummy || b_dummy {
                a.vendor_id.is_some() && a.vendor_id == b.vendor_id && a.product_id == b.product_id
            } else {
                a_serial.zip(b_serial).is_some_and(|(a, b)| a == b)
            }
        }
    }
}

pub fn unique_identity_match(
    identity: &DeviceIdentity,
    candidates: &[DeviceIdentity],
) -> Option<usize> {
    let mut matches = candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| identities_match(identity, candidate));
    let (index, _) = matches.next()?;
    matches.next().is_none().then_some(index)
}

pub fn ordered_values(mut values: Vec<u32>, maximum_count: usize) -> AppResult<Vec<u32>> {
    if values.is_empty() || values.len() > maximum_count || values.contains(&0) {
        return Err(invalid_data("Lista di valori hardware vuota o non valida."));
    }
    values.sort_unstable();
    if values.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(invalid_data(
            "Il provider ha restituito valori hardware duplicati.",
        ));
    }
    Ok(values)
}

pub fn diagnostic(provider: ProviderKind, id: impl Into<String>, error: &AppError) -> Diagnostic {
    let code = match error.code {
        ErrorCode::ProviderMissing => AvailabilityCode::ProviderMissing,
        ErrorCode::PermissionDenied => AvailabilityCode::PermissionDenied,
        ErrorCode::UnsupportedPlatform => AvailabilityCode::UnsupportedPlatform,
        ErrorCode::Unsupported => AvailabilityCode::Unsupported,
        ErrorCode::DeviceNotFound => AvailabilityCode::Absent,
        _ => AvailabilityCode::Unknown,
    };
    let mut diagnostic = Diagnostic::new(id, provider, code, Severity::Warning, &error.message);
    diagnostic.detail = error.detail.clone();
    diagnostic
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MethodSignature {
    pub input: String,
    pub output: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertySignature {
    pub signature: String,
    pub readable: bool,
    pub writable: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Interface {
    pub methods: BTreeMap<String, MethodSignature>,
    pub properties: BTreeMap<String, PropertySignature>,
    pub signals: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub struct Introspection {
    pub interfaces: BTreeMap<String, Interface>,
}

impl Introspection {
    pub fn method(&self, interface: &str, method: &str, input: &str, output: &str) -> bool {
        self.interfaces
            .get(interface)
            .and_then(|i| i.methods.get(method))
            .is_some_and(|m| m.input == input && m.output == output)
    }

    pub fn property(
        &self,
        interface: &str,
        property: &str,
        signature: &str,
        writable: bool,
    ) -> bool {
        self.interfaces
            .get(interface)
            .and_then(|i| i.properties.get(property))
            .is_some_and(|p| p.signature == signature && p.readable && (!writable || p.writable))
    }

    pub fn signal(&self, interface: &str, signal: &str, signature: &str) -> bool {
        self.interfaces
            .get(interface)
            .and_then(|i| i.signals.get(signal))
            .is_some_and(|s| s == signature)
    }
}

// A bounded parser for the DBus introspection grammar, without fetching a DTD. Keeping
// this small, strict grammar portable also lets the installed API contract be fixture-tested.
pub fn parse_introspection(xml: &str) -> AppResult<Introspection> {
    let malformed = || invalid_data("Introspection DBus non valida o troppo grande.");
    if xml.len() > MAX_PROVIDER_BYTES {
        return Err(malformed());
    }
    let mut result = Introspection::default();
    let mut stack: Vec<String> = Vec::new();
    let mut interface_name: Option<String> = None;
    let mut member_name: Option<String> = None;
    let mut cursor = 0;
    let mut root_seen = false;
    let mut elements = 0;
    while cursor < xml.len() {
        let rest = &xml[cursor..];
        let Some(start) = rest.find('<') else {
            if !rest.trim().is_empty() {
                return Err(malformed());
            }
            break;
        };
        if !rest[..start].trim().is_empty() {
            return Err(malformed());
        }
        cursor += start;
        let rest = &xml[cursor..];
        if rest.starts_with("<!--") {
            cursor += rest.find("-->").ok_or_else(malformed)? + 3;
            continue;
        }
        if rest.starts_with("<?") {
            cursor += rest.find("?>").ok_or_else(malformed)? + 2;
            continue;
        }
        let mut quote = None;
        let mut end = None;
        for (index, byte) in rest.bytes().enumerate().skip(1) {
            match (quote, byte) {
                (Some(q), b) if q == b => quote = None,
                (None, b'\'' | b'"') => quote = Some(byte),
                (None, b'>') => {
                    end = Some(index);
                    break;
                }
                _ => {}
            }
        }
        let end = end.ok_or_else(malformed)?;
        let tag = rest[1..end].trim();
        cursor += end + 1;
        if tag.starts_with("!DOCTYPE ") {
            if root_seen || tag.contains('[') {
                return Err(malformed());
            }
            continue;
        }
        if tag.starts_with('!') {
            return Err(malformed());
        }
        if let Some(name) = tag.strip_prefix('/') {
            let name = name.trim();
            if stack.pop().as_deref() != Some(name) {
                return Err(malformed());
            }
            if matches!(name, "method" | "signal") {
                member_name = None;
            }
            if name == "interface" {
                interface_name = None;
            }
            continue;
        }
        elements += 1;
        if elements > 8192 || stack.len() >= 16 {
            return Err(malformed());
        }
        let self_closing = tag.ends_with('/');
        let tag = if self_closing {
            tag[..tag.len() - 1].trim_end()
        } else {
            tag
        };
        let split = tag.find(char::is_whitespace).unwrap_or(tag.len());
        let name = &tag[..split];
        let attrs = parse_attributes(&tag[split..])?;
        let attr = |key: &str| attrs.get(key).cloned().ok_or_else(malformed);
        let parent = stack.last().map(String::as_str);
        match name {
            "node" => {
                if parent.is_none() {
                    if root_seen {
                        return Err(malformed());
                    }
                    root_seen = true;
                } else if parent != Some("node") {
                    return Err(malformed());
                }
            }
            "interface" => {
                if stack.len() != 1 || parent != Some("node") {
                    return Err(malformed());
                }
                let name = attr("name")?;
                if result
                    .interfaces
                    .insert(name.clone(), Interface::default())
                    .is_some()
                {
                    return Err(malformed());
                }
                if !self_closing {
                    interface_name = Some(name);
                }
            }
            "method" | "signal" => {
                if parent != Some("interface") {
                    return Err(malformed());
                }
                let interface = result
                    .interfaces
                    .get_mut(interface_name.as_deref().ok_or_else(malformed)?)
                    .ok_or_else(malformed)?;
                let member = attr("name")?;
                let duplicate = if name == "method" {
                    interface
                        .methods
                        .insert(member.clone(), MethodSignature::default())
                        .is_some()
                } else {
                    interface
                        .signals
                        .insert(member.clone(), String::new())
                        .is_some()
                };
                if duplicate {
                    return Err(malformed());
                }
                if !self_closing {
                    member_name = Some(member);
                }
            }
            "arg" => {
                let interface = result
                    .interfaces
                    .get_mut(interface_name.as_deref().ok_or_else(malformed)?)
                    .ok_or_else(malformed)?;
                let member = member_name.as_deref().ok_or_else(malformed)?;
                let signature = attr("type")?;
                if signature.is_empty() || signature.len() > 255 || !signature.is_ascii() {
                    return Err(malformed());
                }
                let direction = attrs.get("direction").map(String::as_str);
                match parent {
                    Some("method") => {
                        let method = interface.methods.get_mut(member).ok_or_else(malformed)?;
                        match direction.unwrap_or("in") {
                            "in" => method.input.push_str(&signature),
                            "out" => method.output.push_str(&signature),
                            _ => return Err(malformed()),
                        }
                        if method.input.len() > 255 || method.output.len() > 255 {
                            return Err(malformed());
                        }
                    }
                    Some("signal") if direction.is_none() || direction == Some("out") => {
                        let signal = interface.signals.get_mut(member).ok_or_else(malformed)?;
                        signal.push_str(&signature);
                        if signal.len() > 255 {
                            return Err(malformed());
                        }
                    }
                    _ => return Err(malformed()),
                }
            }
            "property" => {
                if parent != Some("interface") {
                    return Err(malformed());
                }
                let access = attr("access")?;
                if !matches!(access.as_str(), "read" | "write" | "readwrite") {
                    return Err(malformed());
                }
                let property = PropertySignature {
                    signature: attr("type")?,
                    readable: access != "write",
                    writable: access != "read",
                };
                let interface = result
                    .interfaces
                    .get_mut(interface_name.as_deref().ok_or_else(malformed)?)
                    .ok_or_else(malformed)?;
                if interface
                    .properties
                    .insert(attr("name")?, property)
                    .is_some()
                {
                    return Err(malformed());
                }
            }
            "annotation" => {
                if !matches!(
                    parent,
                    Some("interface" | "method" | "signal" | "property" | "arg")
                ) {
                    return Err(malformed());
                }
            }
            _ => return Err(malformed()),
        }
        if !self_closing {
            stack.push(name.to_owned());
        }
    }
    if !root_seen || !stack.is_empty() {
        return Err(malformed());
    }
    Ok(result)
}

fn parse_attributes(mut text: &str) -> AppResult<BTreeMap<String, String>> {
    let malformed = || invalid_data("Attributi XML DBus non validi.");
    let mut result = BTreeMap::new();
    while !text.trim().is_empty() {
        text = text.trim_start();
        let end = text
            .find(|c: char| c == '=' || c.is_whitespace())
            .ok_or_else(malformed)?;
        let name = &text[..end];
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b':' | b'-'))
        {
            return Err(malformed());
        }
        text = text[end..]
            .trim_start()
            .strip_prefix('=')
            .ok_or_else(malformed)?
            .trim_start();
        let quote = text
            .chars()
            .next()
            .filter(|c| matches!(c, '\'' | '"'))
            .ok_or_else(malformed)?;
        text = &text[1..];
        let end = text.find(quote).ok_or_else(malformed)?;
        let raw = &text[..end];
        if raw.contains('<') {
            return Err(malformed());
        }
        let mut value = String::new();
        let mut remaining = raw;
        while let Some(index) = remaining.find('&') {
            value.push_str(&remaining[..index]);
            let entity_end = remaining[index..].find(';').ok_or_else(malformed)? + index;
            value.push(match &remaining[index..=entity_end] {
                "&amp;" => '&',
                "&quot;" => '"',
                "&apos;" => '\'',
                "&lt;" => '<',
                "&gt;" => '>',
                _ => return Err(malformed()),
            });
            remaining = &remaining[entity_end + 1..];
        }
        value.push_str(remaining);
        if result.insert(name.to_owned(), value).is_some() || result.len() > 16 {
            return Err(malformed());
        }
        text = &text[end + 1..];
        if !text.is_empty() && !text.starts_with(char::is_whitespace) {
            return Err(malformed());
        }
    }
    Ok(result)
}

#[cfg(target_os = "linux")]
pub(crate) mod dbus {
    use super::*;
    use serde::Serialize;
    use std::{collections::HashMap, time::Duration};
    use zbus::{
        proxy::{CacheProperties, MethodFlags},
        zvariant::{DynamicDeserialize, DynamicType, OwnedValue},
        Connection,
    };

    pub const CALL_TIMEOUT: Duration = Duration::from_secs(2);
    pub const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

    pub struct Bus {
        pub connection: Connection,
        pub owner: String,
    }

    pub fn error(error: zbus::Error) -> AppError {
        let detail = error.to_string();
        let code = if detail.contains("AccessDenied")
            || detail.contains("NotAuthorized")
            || detail.contains("AuthFailed")
        {
            ErrorCode::PermissionDenied
        } else if detail.contains("NameHasNoOwner") || detail.contains("ServiceUnknown") {
            ErrorCode::ProviderMissing
        } else if detail.contains("UnknownObject") {
            ErrorCode::DeviceNotFound
        } else if detail.contains("UnknownMethod")
            || detail.contains("UnknownProperty")
            || detail.contains("UnknownInterface")
        {
            ErrorCode::Unsupported
        } else {
            ErrorCode::CommandFailed
        };
        AppError::new(code, "Il provider DBus non ha completato la richiesta.").detail(detail)
    }

    pub async fn connection(session: bool) -> AppResult<Connection> {
        tokio::time::timeout(CALL_TIMEOUT, async {
            if session {
                Connection::session().await
            } else {
                Connection::system().await
            }
        })
        .await
        .map_err(|_| AppError::new(ErrorCode::Timeout, "Connessione al bus DBus scaduta."))?
        .map_err(error)
    }

    impl Bus {
        pub async fn active(service: &str, session: bool) -> AppResult<Self> {
            let connection = connection(session).await?;
            let bus = Self {
                connection,
                owner: "org.freedesktop.DBus".to_owned(),
            };
            let owner: String = bus
                .call(
                    "/org/freedesktop/DBus",
                    "org.freedesktop.DBus",
                    "GetNameOwner",
                    &(service,),
                )
                .await?;
            if !owner.starts_with(':') || owner.len() > 255 {
                return Err(invalid_data("Proprietario DBus non valido."));
            }
            Ok(Self {
                connection: bus.connection,
                owner,
            })
        }

        pub async fn call<B, R>(
            &self,
            path: &str,
            interface: &str,
            method: &str,
            body: &B,
        ) -> AppResult<R>
        where
            B: Serialize + DynamicType,
            R: for<'de> DynamicDeserialize<'de>,
        {
            let operation = async {
                let proxy: zbus::Proxy<'_> = zbus::proxy::Builder::new(&self.connection)
                    .destination(self.owner.as_str())
                    .map_err(error)?
                    .path(path)
                    .map_err(error)?
                    .interface(interface)
                    .map_err(error)?
                    .cache_properties(CacheProperties::No)
                    .build()
                    .await
                    .map_err(error)?;
                let reply: Option<R> = proxy
                    .call_with_flags(method, MethodFlags::NoAutoStart.into(), body)
                    .await
                    .map_err(error)?;
                reply.ok_or_else(|| invalid_data("Risposta DBus mancante."))
            };
            tokio::time::timeout(CALL_TIMEOUT, operation)
                .await
                .map_err(|_| {
                    AppError::new(ErrorCode::Timeout, "Richiesta al provider DBus scaduta.")
                })?
        }

        pub async fn introspect(&self, path: &str) -> AppResult<Introspection> {
            let xml: String = self
                .call(
                    path,
                    "org.freedesktop.DBus.Introspectable",
                    "Introspect",
                    &(),
                )
                .await?;
            parse_introspection(&xml)
        }

        pub async fn ensure_owner(&self, service: &str) -> AppResult<()> {
            let daemon = Self {
                connection: self.connection.clone(),
                owner: "org.freedesktop.DBus".to_owned(),
            };
            let owner: String = daemon
                .call(
                    "/org/freedesktop/DBus",
                    "org.freedesktop.DBus",
                    "GetNameOwner",
                    &(service,),
                )
                .await?;
            if owner != self.owner {
                return Err(AppError::new(
                    ErrorCode::DeviceNotFound,
                    "Proprietario del provider DBus cambiato; richiesta non ripetuta.",
                ));
            }
            Ok(())
        }

        pub async fn get<T>(&self, path: &str, interface: &str, property: &str) -> AppResult<T>
        where
            T: TryFrom<OwnedValue>,
            T::Error: std::fmt::Display,
        {
            let value: OwnedValue = self
                .call(path, PROPERTIES, "Get", &(interface, property))
                .await?;
            T::try_from(value).map_err(|error| {
                invalid_data("Tipo della proprieta DBus non valido.").detail(error.to_string())
            })
        }

        pub async fn get_all(
            &self,
            path: &str,
            interface: &str,
        ) -> AppResult<HashMap<String, OwnedValue>> {
            let values: HashMap<String, OwnedValue> =
                self.call(path, PROPERTIES, "GetAll", &(interface,)).await?;
            if values.len() > 128 {
                return Err(invalid_data("Troppe proprieta DBus."));
            }
            Ok(values)
        }
    }

    pub fn property<T>(properties: &HashMap<String, OwnedValue>, name: &str) -> AppResult<T>
    where
        T: TryFrom<OwnedValue>,
        T::Error: std::fmt::Display,
    {
        let value = properties
            .get(name)
            .ok_or_else(|| invalid_data("Proprieta DBus obbligatoria mancante.").detail(name))?;
        let value = value.try_clone().map_err(|error| {
            invalid_data("Proprieta DBus non copiabile.").detail(error.to_string())
        })?;
        T::try_from(value).map_err(|error| {
            invalid_data("Tipo della proprieta DBus non valido.").detail(format!("{name}: {error}"))
        })
    }
}
