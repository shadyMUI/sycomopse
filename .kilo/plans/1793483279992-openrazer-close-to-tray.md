# OpenRazer e Close-to-Tray

Stato: pronto per implementazione. Le due scelte sulla tray sono state confermate dall'utente.

## Obiettivo

Avviare OpenRazer prima dell'ispezione DBus e mantenere Peripheral Hub attivo quando viene chiusa la finestra principale, senza interrompere audio PipeWire, DSP surround, monitor hardware o configurazione DPI/report rate.

## Evidenze

- `src-tauri/Cargo.toml:28,33`: `tray-icon`, Tokio `process` e `time` sono gia abilitati; Tauri risolto nel lockfile: 2.12.5.
- `src-tauri/src/providers/openrazer.rs:179-193`: `manager()` controlla direttamente `Bus::active`; il messaggio afferma che l'app non avvia il servizio.
- `src-tauri/src/lib.rs:691-707,712-731`: tray e CloseRequested Linux esistono gia. La X nasconde solo se la creazione tray e riuscita e un StatusNotifier host risulta registrato; altrimenti mantiene aperta la finestra. Non e corretto diagnosticare questa versione come priva di close-to-tray su Linux.
- `src-tauri/src/lib.rs:520-558`: `request_exit` interrompe i worker, attende il monitor, arresta/ripristina l'audio e infine chiama `app.exit(0)`. Non sostituirlo con un exit diretto.
- `src-tauri/src/lib.rs:626-674`: worker e timer batterie sono Rust/Tokio, non JavaScript. SIGINT/SIGTERM richiedono gia l'uscita ordinata.
- `src-tauri/src/lib.rs:739-747`: ogni ExitRequested non pronto attualmente avvia shutdown. Va distinto dall'uscita esplicita.
- `src-tauri/src/commands.rs:129-132`: `exit_app` esiste ed e registrato nell'invoke handler.
- `src/App.tsx:54-90`, `src/api.ts:66-70`: il frontend sottoscrive snapshot; nessuna attivita audio/hardware critica dipende da timer JS o dalla visibilita.
- `src-tauri/capabilities/default.json`: finestra `main` e listener eventi; gestione tray/window Rust non richiede nuovi permessi IPC.
- Worktree iniziale: modifica preesistente a `src-tauri/src/providers/headsetcontrol.rs`. Non alterarla, ripristinarla o includerla come modifica del task.

## Decisioni Confermate

- Tray Tauri standard con menu su Linux. L'utente accetta che click/doppio click Linux non realizzino un toggle: questi eventi non sono emessi da Tauri. Niente backend StatusNotifier custom.
- Se creazione tray o host StatusNotifier falliscono, la X mantiene visibile la finestra con diagnostica, senza terminare il processo. Nascondere solo con tray utilizzabile.
- Conservare shutdown audio ordinato, single-instance e menu finestra esistente. Tutte le azioni Esci convergono su `request_exit`, come `exit_app`. La X Linux non avvia shutdown; SIGTERM/SIGINT continuano a farlo per il cleanup del sistema operativo.
- Autostart del daemon a ogni ingresso in `manager()`, non autostart dell'app al login. `systemctl start` e idempotente e non deve diventare restart/enable.

Fonti API verificate: https://v2.tauri.app/learn/system-tray/#listen-to-tray-events e https://docs.rs/tauri/2.12.5/tauri/macro.include_image.html. La macro PNG risolve il percorso da CARGO_MANIFEST_DIR; non richiede decodifica immagini runtime.

## 1. Cargo e Config

Nessuna modifica necessaria a `Cargo.toml`: conservare queste righe gia presenti, inclusa l'opzionalita desktop.

```toml
tauri = { version = "2", features = ["tray-icon"], optional = true }
tokio = { version = "1", features = ["process", "time", "sync", "io-util", "rt-multi-thread", "macros", "signal"] }
```

Nessuna modifica a `tauri.conf.json`, capabilities, Cargo.lock o main.rs. `main` esiste, il PNG e gia in `icons/`, e la tray sara creata in Rust. Non aggiungere permessi shell/window al frontend, `image-png`, chiavi tray Tauri v1 o nuovi plugin.

## 2. Processo e Provider

In `src-tauri/src/process.rs`, estendere solo l'allowlist e aggiungere `assert!(allowed("systemctl"));` al test `rejects_shells_and_arbitrary_executables`. Il runner esistente usa Tokio, limiti output, timeout, kill e reap: non duplicarlo.

```rust
fn allowed(executable: &str) -> bool {
    matches!(
        executable,
        "pw-dump" | "wpctl" | "pipewire" | "headsetcontrol" | "ratbagd" | "systemctl"
    )
}
```

In `src-tauri/src/providers/openrazer.rs`, dentro `mod linux`, sostituire l'import HashSet con `use std::{collections::HashSet, time::Duration};` e sostituire interamente `manager()` con:

```rust
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
```

Il fallimento systemctl non impedisce l'uso di un daemon gia registrato su DBus, ad esempio avviato manualmente senza unit installata. Il fallimento DBus/introspection mantiene il proprio ErrorCode e include l'esito di avvio. `bounded_text` mantiene entrambi i dettagli entro il limite AppError, senza sommergere la causa DBus con stderr lungo. Tutti gli argomenti sono costanti: niente shell, sudo o comandi provenienti dal frontend.

Aggiornare solo la prima stringa di `limitations` a `openrazer.rs:503` in:

```rust
"Unico proprietario delle scritture Razer: OpenRazer; l'app tenta l'avvio del daemon tramite systemctl --user."
```

Mantenere invariati discovery, firme DBus, NoAutoStart, identity checks e readback. Il servizio OpenRazer e posseduto da systemd, non dall'app: non arrestarlo durante `request_exit`.

## 3. Helper e Routing Menu

In `src-tauri/src/lib.rs`, gli import menu/tray/AppHandle/Manager/StdMutex sono gia presenti. Sostituire `show_window`, aggiungere `toggle_window` e sostituire `menu_event` con questi blocchi completi:

```rust
#[cfg(feature = "desktop")]
fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if let Err(error) = window.show() {
            eprintln!("Mostra finestra: {error}");
        }
        if let Err(error) = window.unminimize() {
            eprintln!("Ripristino finestra: {error}");
        }
        if let Err(error) = window.set_focus() {
            eprintln!("Focus finestra: {error}");
        }
    }
}

#[cfg(feature = "desktop")]
fn toggle_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    match (window.is_visible(), window.is_minimized()) {
        (Ok(true), Ok(false)) => {
            if let Err(error) = window.hide() {
                eprintln!("Nascondi finestra: {error}");
            }
        }
        (Ok(_), Ok(_)) => show_window(app),
        (Err(error), _) | (_, Err(error)) => {
            eprintln!("Stato finestra: {error}");
            show_window(app);
        }
    }
}

#[cfg(feature = "desktop")]
fn menu_event(app: &AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
        "open" | "tray-open" => show_window(app),
        "surround-off" => {
            let app = app.clone();
            let state = app.state::<Arc<AppState>>().inner().clone();
            tauri::async_runtime::spawn(async move {
                let _ = state
                    .mutate_audio(&app, AudioMutation::Surround(false))
                    .await;
            });
        }
        "exit" | "tray-exit" => request_exit(app.clone()),
        _ => {}
    }
}
```

Conservare UNA registrazione globale `.on_menu_event(menu_event)` sul Builder esistente. Rimuovere la registrazione duplicata dal TrayIconBuilder: i callback menu Tauri sono globali, non limitati al singolo menu.

## 4. Setup Tray

Nel Builder di `run()`, mantenere plugin e invoke handler esistenti, incluso `commands::exit_app`, e sostituire l'intera closure `.setup(...)` con:

```rust
.setup(|app| {
    let state = Arc::new(AppState::new()?);
    app.manage(state.clone());
    let open = MenuItem::with_id(app, "open", "Apri", true, None::<&str>)?;
    let off = MenuItem::with_id(
        app, "surround-off", "Disattiva 7.1", cfg!(target_os = "linux"), None::<&str>,
    )?;
    let separator = PredefinedMenuItem::separator(app)?;
    let exit = MenuItem::with_id(app, "exit", "Esci", true, None::<&str>)?;
    let window_menu = Menu::with_items(app, &[&open, &off, &separator, &exit])?;
    app.set_menu(window_menu)?;

    let tray_open = MenuItem::with_id(app, "tray-open", "Mostra / Apri", true, None::<&str>)?;
    let tray_separator = PredefinedMenuItem::separator(app)?;
    let tray_exit = MenuItem::with_id(app, "tray-exit", "Esci", true, None::<&str>)?;
    let tray_menu = Menu::with_items(app, &[&tray_open, &tray_separator, &tray_exit])?;
    let skip_release = StdMutex::new(false);
    let tray = TrayIconBuilder::with_id("peripheral-hub")
        .icon(tauri::include_image!("icons/32x32.png"))
        .tooltip("Peripheral Hub")
        .menu(&tray_menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(move |tray, event| match event {
            TrayIconEvent::Click {
                button: MouseButton::Left, button_state: MouseButtonState::Down, ..
            } => {
                *skip_release.lock().unwrap_or_else(|error| error.into_inner()) = false;
            }
            TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } => {
                // Windows: il primo rilascio ha gia commutato la finestra.
                *skip_release.lock().unwrap_or_else(|error| error.into_inner()) = true;
            }
            TrayIconEvent::Click {
                button: MouseButton::Left, button_state: MouseButtonState::Up, ..
            } => {
                let skip = std::mem::take(
                    &mut *skip_release.lock().unwrap_or_else(|error| error.into_inner()),
                );
                if !skip {
                    toggle_window(tray.app_handle());
                }
            }
            _ => {}
        })
        .build(app);
    match tray {
        Ok(_) => state.tray_created.store(true, Ordering::SeqCst),
        Err(error) => eprintln!("Impossibile creare la tray: {error}"),
    }
    #[cfg(target_os = "linux")]
    start_workers(app.handle(), &state);
    Ok(())
})
```

Tauri conserva la tray nella propria resource table dopo build: non e necessario aggiungere un altro handle ad AppState. PNG incorporato a build time, nessun path runtime fragile. Il reset su Down evita una soppressione rimasta pendente; la sequenza Windows Down/Up/DoubleClick/Up produce un solo toggle, senza un timer arbitrario. Su Linux questi callback e il flag left-click non hanno effetto: il ripristino supportato e il menu, come accettato dall'utente.

## 5. CloseRequested e Uscita

Mantenere `tray_host_available()` con timeout e controllo host gia presenti. Nel Builder sostituire la closure `.on_window_event(...)` con:

```rust
.on_window_event(|window, event| {
    #[cfg(target_os = "linux")]
    {
        if window.label() != "main" {
            return;
        }
        let tauri::WindowEvent::CloseRequested { api, .. } = event else {
            return;
        };
        api.prevent_close();
        let window = window.clone();
        let app = window.app_handle().clone();
        let state = app.state::<Arc<AppState>>().inner().clone();
        if state.shutdown_started.load(Ordering::SeqCst) {
            return;
        }
        tauri::async_runtime::spawn(async move {
            let available = state.tray_created.load(Ordering::SeqCst)
                && tray_host_available().await;
            if state.shutdown_started.load(Ordering::SeqCst) {
                return;
            }
            let hide_error = if available {
                match window.hide() {
                    Ok(()) => {
                        state.publish(&app, |snapshot| {
                            snapshot.diagnostics.retain(|item| item.id != "tray-unavailable");
                        }).await;
                        return;
                    }
                    Err(error) => Some(error.to_string()),
                }
            } else {
                None
            };
            show_window(&app);
            state.publish(&app, |snapshot| {
                snapshot.diagnostics.retain(|item| item.id != "tray-unavailable");
                let mut diagnostic = Diagnostic::new(
                    "tray-unavailable", ProviderKind::App,
                    if hide_error.is_some() { AvailabilityCode::Unknown } else { AvailabilityCode::Unsupported },
                    Severity::Warning,
                    if hide_error.is_some() {
                        "Impossibile nascondere la finestra: resta aperta. Usa Esci per ripristinare l'audio e chiudere."
                    } else {
                        "Tray non utilizzabile: la finestra resta aperta. Usa Esci per ripristinare l'audio e chiudere."
                    },
                );
                diagnostic.detail = hide_error;
                snapshot.diagnostics.push(diagnostic);
            }).await;
        });
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (window, event);
})
```

Non chiamare shutdown o abort worker da questo handler; non distruggere il webview. La diagnostica tray precedente viene eliminata dopo un hide riuscito. Il fallback controlla l'host a ogni X; se il pannello scompare quando la finestra e gia nascosta, si puo recuperarla rilanciando l'app tramite single-instance. Non aggiungere un nuovo monitor di pannello in questo task.

Sostituire il callback finale `app.run(...)` con:

```rust
app.run(|app, event| {
    if let tauri::RunEvent::ExitRequested { api, .. } = event {
        let state = app.state::<Arc<AppState>>();
        if !state.exit_ready.load(Ordering::SeqCst) {
            api.prevent_exit();
            #[cfg(not(target_os = "linux"))]
            request_exit(app.clone());
        }
    }
});
```

La guardia Linux impedisce exit accidentali senza innescare shutdown; il ramo non-Linux conserva il lifecycle attuale su piattaforme hardware unsupported. Mantenere integralmente `request_exit`, `exit_app`, registrazione segnali e `start_workers`: shutdown idempotente, abort/await worker, attesa monitor fino a 3 secondi, arresto DSP/ripristino default audio, `exit_ready = true`, infine `app.exit(0)`. Nessun exit diretto dai menu prima del cleanup.

## Confini e Non-Obiettivi

- Nessuna nuova politica di applicazione automatica dei profili audio: oggi i profili sono applicati esplicitamente.
- Nessun autostart dell'app al login, installazione pacchetti, modifica unit systemd o elevazione privilegi.
- Il report rate hardware viene impostato via provider, non generato da polling frontend. Hide preserva il processo e le impostazioni, non rende scrivibili mouse non supportati.
- Restano le politiche esistenti che fermano surround su perdita del target, guasto PipeWire/DSP o HRTF invalido. Non modificarle per simulare continuita.
- Nessuna migrazione di dati o profili; nessun nuovo comando exit duplicato; nessuna modifica al file headsetcontrol gia dirty.

## Verifica Dopo Implementazione

1. Controllare diff finale e preservazione di modifiche esterne. Eseguire dalla root `npm test` e `npm run build`; da `src-tauri`, `cargo fmt --all -- --check`, `cargo test --no-default-features --locked`, `cargo test --locked`, `cargo clippy --all-targets --locked -- -D warnings`. Verificare anche build desktop Linux reale; Windows non copre il provider cfg Linux. Segnalare problemi preesistenti senza formattare o correggere file esterni al task.
2. OpenRazer Linux: daemon inizialmente fermo -> avvio applicazione -> systemctl start precede DBus -> dispositivi disponibili; daemon gia attivo -> nessun restart; unit/eseguibile mancanti e org.razer presente -> provider continua; assenza sia servizio sia org.razer -> diagnostica esplicita; timeout/AccessDenied -> codici e dettagli non perduti. Verificare con `systemctl --user show openrazer-daemon.service --property=LoadState,ActiveState,SubState --no-pager`.
3. Tray Linux sul desktop target: icona e menu visibili, X ripetuta non termina processo, Mostra ripristina anche finestra minimizzata, una sola icona/istanza, menu utilizzabile anche se frontend non ha snapshot. Host assente/creazione fallita -> finestra visibile e warning; host recuperato -> hide riuscito e warning eliminato. Restart del pannello e riapertura tramite seconda istanza. Su Windows verificare singolo click e sequenza doppio click con un solo toggle; non dichiarare supportati gli eventi click Linux.
4. Background: attivare surround/profilo, nascondere la finestra, osservare monitor e DSP ancora attivi, hotplug e aggiornamenti batterie, riaprire e verificare snapshot coerente; verificare report rate via readback. Osservazione audio con `wpctl status` e `pw-dump -N`.
5. Esci tray ed exit_app: filtro terminato e default audio ripristinato, task/monitor conclusi, processo terminato, OpenRazer systemd non arrestato dall'app. Ripetizione Esci durante una mutazione/dialogo: nessun doppio shutdown. SIGTERM/SIGINT mantengono cleanup.

## Stato della Validazione

Solo analisi read-only. Nessun sorgente modificato e nessun build/test eseguito. L'ambiente e Windows; le verifiche runtime CachyOS/Arch richiedono Linux con session bus, PipeWire e tray host. Un tentativo di rilevare rustc e stato negato dalla policy degli strumenti e non e stato aggirato. Il subagent ha verificato `git diff --check` senza errori, con solo avviso LF/CRLF sul file headsetcontrol preesistente.

## Consegna

Modificare soltanto `src-tauri/src/providers/openrazer.rs`, `src-tauri/src/process.rs` e `src-tauri/src/lib.rs`, con test mirati nell'allowlist esistente. Formattare i blocchi Rust applicati secondo lo stile del progetto. Consegnare riepilogo modifiche e verifiche effettive, evidenziando che Cargo/config non richiedono cambiamenti e che su Linux la tray si usa tramite menu. Nessuna migrazione, installazione pacchetti o altra decisione bloccante aperta.
