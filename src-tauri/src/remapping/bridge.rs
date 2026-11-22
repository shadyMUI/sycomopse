use crate::models::{AppError, AppResult, ErrorCode};
use super::types::*;
use serde::Serialize;
use std::path::Path;
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
pub mod linux {
    use super::*;
    use crate::providers::dbus::{self, Bus};
    pub const SERVICE: &str = "io.peripheralhub.InputRemapper";
    const PATH: &str = "/io/peripheralhub/InputRemapper";
    const INTERFACE: &str = "io.peripheralhub.InputRemapper1";
    // Root startup/cleanup is synchronous (4s startup + up to 8s teardown).
    const CALL_TIMEOUT: Duration = Duration::from_secs(16);
    const STOP_AGENT_ARGUMENTS: &[&str] = &["--user", "stop", AGENT_UNIT];

    pub struct Bridge {
        pub(super) bus: Bus,
    }

    fn parse<T: serde::de::DeserializeOwned>(json: String) -> AppResult<T> {
        if json.len() > MAX_BYTES {
            return Err(invalid("Risposta bridge troppo grande."));
        }
        serde_json::from_str(&json)
            .map_err(|error| invalid("Risposta bridge non valida.").detail(error.to_string()))
    }

    fn bridge_error(mut error: AppError) -> AppError {
        let detail = error.detail.as_deref().unwrap_or("");
        if detail.contains(".Busy") || detail.contains(".Conflict") || detail.contains(".Stale") {
            error.code = ErrorCode::Busy;
        } else if detail.contains(".Validation") || detail.contains(".InvalidPreset") {
            error.code = ErrorCode::Validation;
        }
        error
    }

    impl Bridge {
        async fn call<B, R>(&self, interface: &str, method: &str, body: &B) -> AppResult<R>
        where
            B: Serialize + zbus::zvariant::DynamicType,
            R: for<'de> zbus::zvariant::DynamicDeserialize<'de>,
        {
            let request = async {
                let proxy: zbus::Proxy<'_> = zbus::proxy::Builder::new(&self.bus.connection)
                    .destination(self.bus.owner.as_str())
                    .map_err(dbus::error)?
                    .path(PATH)
                    .map_err(dbus::error)?
                    .interface(interface)
                    .map_err(dbus::error)?
                    .cache_properties(zbus::proxy::CacheProperties::No)
                    .build()
                    .await
                    .map_err(dbus::error)?;
                let reply: Option<R> = proxy
                    .call_with_flags(method, zbus::proxy::MethodFlags::NoAutoStart.into(), body)
                    .await
                    .map_err(dbus::error)?;
                reply.ok_or_else(|| invalid("Risposta bridge mancante."))
            };
            tokio::time::timeout(CALL_TIMEOUT, request)
                .await
                .map_err(|_| {
                    AppError::new(
                        ErrorCode::Timeout,
                        "Richiesta al bridge scaduta; esito della rimappatura non confermato.",
                    )
                })?
        }

        pub async fn connect() -> AppResult<Self> {
            let bus = Bus::active(SERVICE, false).await.map_err(|error| AppError::new(error.code, "Backend di rimappatura non disponibile: installare o riparare i servizi Peripheral Hub.").detail(error.message))?;
            let bridge = Self { bus };
            let xml: String = bridge
                .call("org.freedesktop.DBus.Introspectable", "Introspect", &())
                .await?;
            let api = crate::providers::parse_introspection(&xml)?;
            for (method, input, output) in [
                ("GetVersion", "", "u"),
                ("Inventory", "", "s"),
                ("State", "s", "s"),
                ("Apply", "stsss", "s"),
                ("Stop", "ss", "s"),
            ] {
                if !api.method(INTERFACE, method, input, output) {
                    return Err(AppError::new(
                        ErrorCode::Unsupported,
                        "Protocollo del bridge non compatibile; riparare l'installazione.",
                    ));
                }
            }
            let version: u32 = bridge.call(INTERFACE, "GetVersion", &()).await?;
            if version != 1 {
                return Err(invalid("Versione bridge non supportata."));
            }
            bridge.bus.ensure_owner(SERVICE).await?;
            Ok(bridge)
        }

        pub async fn inventory(&self) -> AppResult<Inventory> {
            self.bus.ensure_owner(SERVICE).await?;
            let inventory: Inventory = parse(self.call(INTERFACE, "Inventory", &()).await?)?;
            inventory.validate()?;
            self.bus.ensure_owner(SERVICE).await?;
            Ok(inventory)
        }

        pub async fn state(&self, fingerprint: &str) -> AppResult<BridgeState> {
            let state: BridgeState = parse(
                self.call(INTERFACE, "State", &(fingerprint,))
                    .await
                    .map_err(bridge_error)?,
            )?;
            if state.version != 1
                || state
                    .fingerprint
                    .as_deref()
                    .is_some_and(|value| value != fingerprint)
                || !matches!(
                    state.state.as_str(),
                    "UNKNOWN"
                        | "STARTING"
                        | "RUNNING"
                        | "STOPPED"
                        | "FAILED"
                        | "NO_GRAB"
                        | "UPGRADE_EVDEV"
                )
            {
                return Err(invalid("Stato bridge non coerente."));
            }
            self.bus.ensure_owner(SERVICE).await?;
            Ok(state)
        }

        pub async fn apply(
            &self,
            input: &InputDevice,
            generation: u64,
            preset: &str,
            expected: &str,
            directory: &Path,
        ) -> AppResult<()> {
            self.bus.ensure_owner(SERVICE).await?;
            let directory = directory
                .to_str()
                .ok_or_else(|| invalid("Percorso preset non UTF-8."))?;
            let _: BridgeState = parse(
                self.call(
                    INTERFACE,
                    "Apply",
                    &(&input.fingerprint, generation, preset, expected, directory),
                )
                .await
                .map_err(bridge_error)?,
            )?;
            let deadline = Instant::now() + Duration::from_secs(4);
            loop {
                let state = self.state(&input.fingerprint).await?;
                if state.state == "RUNNING"
                    && state.preset.as_deref() == Some(preset)
                    && state.owner_uid == Some(current_uid())
                {
                    return Ok(());
                }
                if state.state != "STARTING" || Instant::now() >= deadline {
                    return Err(AppError::new(
                        ErrorCode::NotConfirmed,
                        "Rimappatura non confermata: nessun successo simulato.",
                    )
                    .detail(state.message.as_deref().unwrap_or(&state.state)));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }

        pub async fn stop(&self, fingerprint: &str, expected: &str) -> AppResult<()> {
            self.bus.ensure_owner(SERVICE).await?;
            let _: BridgeState = parse(
                self.call(INTERFACE, "Stop", &(fingerprint, expected))
                    .await
                    .map_err(bridge_error)?,
            )?;
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                let state = self.state(fingerprint).await?;
                if matches!(state.state.as_str(), "STOPPED" | "UNKNOWN") && state.preset.is_none() {
                    return Ok(());
                }
                if state
                    .preset
                    .as_deref()
                    .is_some_and(|value| value != expected)
                {
                    return Err(AppError::new(
                        ErrorCode::Busy,
                        "Preset cambiato durante la disattivazione.",
                    ));
                }
                if Instant::now() >= deadline {
                    return Err(AppError::new(
                        ErrorCode::NotConfirmed,
                        "Arresto della rimappatura non confermato.",
                    ));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }

        pub async fn stop_owned(&self, fingerprint: &str) -> AppResult<()> {
            crate::remapping::stop_owned(self, fingerprint).await
        }
    }

    impl RemappingBackend for Bridge {
        fn owner(&self) -> &str {
            &self.bus.owner
        }

        async fn inventory(&self) -> AppResult<Inventory> {
            Bridge::inventory(self).await
        }

        async fn state(&self, fingerprint: &str) -> AppResult<BridgeState> {
            Bridge::state(self, fingerprint).await
        }

        async fn stop(&self, fingerprint: &str, expected: &str) -> AppResult<()> {
            Bridge::stop(self, fingerprint, expected).await
        }

        async fn apply(
            &self,
            input: &InputDevice,
            generation: u64,
            preset: &str,
            expected: &str,
            directory: &Path,
        ) -> AppResult<()> {
            Bridge::apply(self, input, generation, preset, expected, directory).await
        }
    }

    pub async fn quiesce_agent() -> AppResult<()> {
        // An explicit user-unit stop cancels queued starts and Restart=on-failure.
        // Never stop the root bridge or any stock/foreign service.
        crate::process::run("systemctl", STOP_AGENT_ARGUMENTS, Duration::from_secs(5))
            .await
            .map(|_| ())
    }

    pub async fn active_session() -> AppResult<bool> {
        let bus = Bus::active("org.freedesktop.login1", false).await?;
        let sessions: Vec<(String, u32, String, String, zbus::zvariant::OwnedObjectPath)> = bus
            .call(
                "/org/freedesktop/login1",
                "org.freedesktop.login1.Manager",
                "ListSessions",
                &(),
            )
            .await?;
        let mut active = Vec::new();
        for (_, uid, _, seat, path) in sessions {
            if seat != "seat0" {
                continue;
            }
            let kind: String = bus
                .get(path.as_str(), "org.freedesktop.login1.Session", "Type")
                .await?;
            let remote: bool = bus
                .get(path.as_str(), "org.freedesktop.login1.Session", "Remote")
                .await?;
            let running: bool = bus
                .get(path.as_str(), "org.freedesktop.login1.Session", "Active")
                .await?;
            if matches!(kind.as_str(), "x11" | "wayland") && !remote && running {
                active.push(uid);
            }
        }
        Ok(active.len() == 1 && active[0] == current_uid())
    }

    #[cfg(test)]
    #[test]
    fn emergency_stop_targets_only_our_user_unit() {
        assert_eq!(STOP_AGENT_ARGUMENTS, &["--user", "stop", AGENT_UNIT]);
        assert_ne!(AGENT_UNIT, SERVICE);
    }
}

