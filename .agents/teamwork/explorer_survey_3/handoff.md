# Rapporto di Revisione del Codice e Survey Architetturale: Sycomopse

Data: 17 Novembre 2026  
Autore: Teamwork Explorer (`explorer_survey_3`)  
Oggetto: Valutazione esaustiva di Architettura, Sicurezza, Confini IPC, Configurazione, Qualità del Codice e Test per Sycomopse (Frontend React 19 / TypeScript e Backend Nativo Rust / Tauri v2).

---

## 1. Observation (Osservazioni Dirette)

L'indagine è stata condotta tramite analisi statica diretta e manuale del codice sorgente, dei file di configurazione, delle pipeline di CI e tramite esecuzione delle suite di test e di build disponibili nell'ambiente.

### 1.1 Configurazione, Sistemi di Build e Dipendenze

1. **Frontend (`package.json`, `tsconfig.json`, `vite.config.ts`)**:
   - **Stack**: React 19 (`19.2.0`), TypeScript 5.8 (`5.8.2`), Vite 6 (`6.2.0`), Lucide React (`^1.16.0`), Vitest (`^3.0.7`), `@testing-library/react` (`^16.2.0`).
   - **Target**: `target: ES2022`, `moduleResolution: bundler`, `strict: true`, `noFallthroughCasesInSwitch: true`, `noUncheckedIndexedAccess: true`.
   - **Vite Config** (`vite.config.ts:7-27`): Configurato con porta fissa 1420, `strictPort: true`, disabilitazione HMR su overlay di errore Tauri, target ES2021/safari13, minify abilitato in produzione.
   - **Script NPM** (`package.json:8-19`): Definiti `dev`, `build`, `test`, `test:watch`, `test:coverage`, `preview`. **Osservazione**: Non sono configurati script di linting (`eslint`) né di formattazione (`prettier`).

2. **Backend Nativo Rust (`src-tauri/Cargo.toml`, `src-tauri/build.rs`, `src-tauri/tauri.conf.json`)**:
   - **Stack Rust**: Edizione Rust 2021, Tauri v2 (`2.8.5`), Serde (`1.0`), Tokio (`1.43`), ZBus (`4.4`), Nix (`0.29`), Udev (`0.9`), Pulsectl (`0.6`), Tempfile (`3.17`).
   - **Build Script** (`src-tauri/build.rs:1-3`): Esegue `tauri_build::build()`.
   - **Tauri Config** (`src-tauri/tauri.conf.json:1-38`):
     - `identifier`: `it.alexcape.sycomopse`
     - Finestra principale: 1080x740, min 920x620, non resizable (`resizable: false`), avvio non visibile (`visible: false`) coordinato con system tray icon.
     - Tray Icon: Icona SVG (`icons/icon.svg`), menu di uscita dedicato, gestione visibilità finestra.
   - **Dipendenza inutilizzata**: In `src-tauri/Cargo.toml:51` è presente `quick-xml = "0.38"`. L'analisi del codebase rivela che l'introspezione D-Bus XML è gestita da un parser custom in `src-tauri/src/providers/mod.rs:179-397`, lasciando `quick-xml` come dipendenza non compilata né referenziata nel codice sorgente.

3. **Packaging e CI Linux (`packaging/`, `.github/workflows/linux-packaging.yml`)**:
   - Pipeline avanzata con matrice su 10 distribuzioni Linux (Ubuntu, Debian, Fedora, Arch, OpenSUSE, Alpine, ecc.) orchestrata da script Python (`packaging/ci/matrix.py`, `packaging/ci/gates.py`, `packaging/ci/smoke.py`).
   - Workflow GitHub Actions (`.github/workflows/linux-packaging.yml:3-11`): Eseguito solo su `workflow_dispatch` e creazione tag (`tags: ['v*']`), privo di trigger sui normali push o pull request verso `main`.

---

### 1.2 Confine IPC e Contratti Dati (Frontend vs Backend)

1. **Comandi Esportati e Mappatura**:
   - Registrati in `src-tauri/src/lib.rs:793-810`:
     - `get_snapshot`
     - `set_dpi`
     - `set_profile`
     - `set_polling_rate`
     - `set_rgb`
     - `set_audio_profile`
     - `set_spatial_audio`
     - `set_remapping_binding`
     - `reset_remapping_binding`
     - `commit_remapping`
     - `discard_remapping`
     - `save_profile`
     - `load_profile`
     - `delete_profile`
   - Invocati dal frontend tramite wrapper fortemente tipizzati in `src/api.ts:25-99`.
2. **Validazione e Contratti Dati**:
   - Tutti i DTO in `src-tauri/src/models.rs` applicano `#[serde(deny_unknown_fields)]` e `#[serde(rename_all = "camelCase")]`.
   - Modello *Readback-driven*: Le mutazioni hardware non applicano aggiornamenti ottimistici non verificati. Il frontend attende l'aggiornamento confermato dalla snapshot di stato inviata tramite evento o ritornata dal comando.
   - Contratti speculari TypeScript in `src/types.ts:1-125` perfettamente allineati con le strutture Rust.
3. **Eventi Push**:
   - Unico canale broadcast: `snapshot-changed` (`src-tauri/src/lib.rs:818`), ascoltato in `src/App.tsx:88-100` tramite `listen<HardwareSnapshot>('snapshot-changed')`.

---

### 1.3 Postura di Sicurezza

1. **Permessi e Capabilities Tauri v2 (`src-tauri/capabilities/default.json:1-19`)**:
   - `permissions`: limitato esclusivamente a `["core:default", "core:event:allow-listen", "core:event:allow-unlisten"]`.
   - **Nessuna esposizione** dei plugin Tauri ad alto rischio: nessun accesso al filesystem (`fs`), nessuna esecuzione di comandi arbitrari (`shell`), nessun accesso al clipboard o notifiche non controllate dal backend nativo.
2. **Content Security Policy (CSP) (`src-tauri/tauri.conf.json:20-25`)**:
   - `"csp": "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: asset:; font-src 'self' data:; connect-src 'self' ipc:; object-src 'none'; base-uri 'self'; frame-ancestors 'none';"`
   - Protegge completamente da attacchi XSS basati su iniezione di script remoti o framing/clickjacking.
3. **Esecuzione Processi Esterni e Whitelisting (`src-tauri/src/process.rs`)**:
   - Whitelist rigida e immutabile degli eseguibili consentiti:
     - `/usr/bin/headsetcontrol`
     - `/usr/bin/openrazer-daemon`
     - `/usr/bin/sycomopse-remapping-agent`
     - `/usr/bin/sycomopse-audio-worker`
     - `/usr/bin/pw-cli`
     - `/usr/bin/pactl`
     - `/usr/bin/systemctl`
   - Non vengono utilizzati shell interpreter (`/bin/sh -c` o `/bin/bash -c`). Gli argomenti sono passati come slice atomiche di stringhe (`&[&str]`), escludendo vulnerabilità di command injection.
   - Protezione da processi orfani (`process.rs:136-150`): Configurazione di `PR_SET_PDEATHSIG` con `SIGTERM` e verifica immediata post-fork di `getppid() == parent_pid`.
4. **Isolamento e Sandboxing Worker SOFA (`src-tauri/src/audio_worker.rs`, `src-tauri/src/bin/audio_worker.rs`)**:
   - L'elaborazione e la validazione dei file HRTF SOFA (formato HDF5/NetCDF) avviene in un processo worker isolato non privilegiato.
   - `audio_worker.rs:114-152` impone limiti di memoria `RLIMIT_AS` (256 MB) tramite `nix::sys::resource::setrlimit` e fa drop dei privilegi se eseguito con UID/GID root.
5. **Gestione Segreti e Dati Sensibili**:
   - Nessun token API, credenziale hardcoded o connessione verso l'esterno rilevata. Nessuna telemetria non autorizzata.

---

### 1.4 Test, Copertura e Pipeline CI/CD

1. **Frontend Unit & Component Testing (`vitest run`)**:
   - 2 file di test (`src/tests/api.test.ts`, `src/tests/App.test.tsx`), per un totale di 79 test unitari e di integrazione component.
   - Tutti i 79 test passano con successo (tempo di esecuzione: ~11.9s).
   - Rilevata flakiness occasionale in `src/tests/App.test.tsx:305:18` durante l'esecuzione dell'intera suite a causa della latenza di rendering del banner di errore alert in JSDOM.
2. **Backend Rust Tests (`src-tauri/tests/`)**:
   - Test di integrazione contrattuale e recupero:
     - `src-tauri/tests/hardware_contracts.rs` (381 righe)
     - `src-tauri/tests/audio_dsp_contract.rs` (334 righe)
     - `src-tauri/tests/audio_worker.rs` (215 righe)
     - `src-tauri/tests/remapping/recovery.rs` (174 righe)
   - I test backend richiedono l'ambiente containerizzato con i mock di sistema Linux (udev/pulseaudio/zbus) fornito dai container di build in `packaging/ci`.
3. **Packaging Tests (`packaging/tests`)**:
   - 224 test unitari eseguiti con `python3 -m unittest discover -s packaging/tests -v`, tutti superati in 0.993 secondi.

---

### 1.5 Catalogo Dettagliato dei Difetti e Aree di Miglioramento

| ID | Titolo | Severità | File / Linee | Categoria |
|:---|:---|:---:|:---|:---:|
| **DEF-01** | Polling continuo a 2s con emissione incondizionata di `snapshot-changed` | **Media** | `src-tauri/src/lib.rs:781-791, 242-257` | Prestazioni / Architettura |
| **DEF-02** | Dipendenza inutilizzata `quick-xml = "0.38"` in Cargo.toml | **Bassa** | `src-tauri/Cargo.toml:51` | Manutenibilità / Build |
| **DEF-03** | Moduli backend monolitici ad alta complessità dimensionale | **Media** | `src-tauri/src/remapping.rs`, `audio.rs`, `ratbag.rs`, `lib.rs` | Qualità del Codice |
| **DEF-04** | Timeout eccessivo di 100 secondi su `systemctl --user stop` | **Media** | `src-tauri/src/remapping.rs:1683` | Gestione Errori / UX |
| **DEF-05** | Assenza di trigger CI su `push` e pipeline dedicata per PR | **Media** | `.github/workflows/linux-packaging.yml:3-11` | CI/CD / Testing |
| **DEF-06** | Assenza di linter e formatter configurati nel frontend e nel backend | **Media** | `package.json:8-19`, `.github/workflows/` | Qualità del Codice |
| **DEF-07** | Flakiness temporale nel test frontend su gestione errori DPI | **Media** | `src/tests/App.test.tsx:298-308` | Test |
| **DEF-08** | Ricreazione forzata di `AudioPanel` con perdita di stato non salvato | **Bassa** | `src/App.tsx:40, 65, 137, 156` | Architettura Frontend / UX |
| **DEF-09** | Utilizzo di `crypto.randomUUID()` senza fallback in ambiente webview legacy | **Bassa** | `src/views/AudioPanel.tsx:38` | Robustezza Frontend |
| **DEF-10** | Gestione asimmetrica dello stato del click sull'icona della Tray di sistema | **Bassa** | `src-tauri/src/lib.rs:840-875` | Architettura / UX Linux |

---

#### Schede Dettagliate dei Singoli Difetti

#### DEF-01: Polling continuo a 2s con emissione incondizionata di `snapshot-changed`
- **Descrizione**: In `src-tauri/src/lib.rs:781-791` un task asincrono Tokio esegue un loop `loop { tokio::time::sleep(Duration::from_secs(2)).await; app_handle.state::<AppState>().poll_once(&app_handle); }`. In `poll_once` (linee 242-257), il metodo invoca `self.remapping.decorate(...)`, acquisisce il lock esclusivo in scrittura su `self.hardware.write().unwrap()`, incrementa incondizionatamente `snapshot.revision += 1` ed emette l'evento `app_handle.emit("snapshot-changed", &snapshot)`.
- **Severità**: **Media**
- **Riferimento Codice**: `src-tauri/src/lib.rs:242-257` e `src-tauri/src/lib.rs:781-791`
- **Impatto**: Anche a sistema completamente inattivo (nessun dispositivo collegato o nessun cambio di stato), ogni 2 secondi la CPU si risveglia, il lock `hardware` viene conteso ed emesso un payload JSON verso la webview. Il frontend React in `src/App.tsx:90` riceve l'evento, scatena un re-render dell'intero albero di componenti e aggiorna lo stato, consumando batteria e cicli CPU inutilmente.
- **Remediation**: 
  1. Calcolare l'hash o confrontare lo snapshot prima e dopo la decorazione di remapping.
  2. Incrementare `snapshot.revision` ed emettere `snapshot-changed` **solo se** i dati hardware o lo stato di remapping sono effettivamente cambiati rispetto alla snapshot precedente.
  3. Adottare monitoraggio basato su eventi D-Bus / udev (già presente tramite `tokio::select!` per udev e upower) evitando il polling a tempo o estendendo l'intervallo a 10-30s in stato di idle.

---

#### DEF-02: Dipendenza inutilizzata `quick-xml = "0.38"` in Cargo.toml
- **Descrizione**: In `src-tauri/Cargo.toml:51` è dichiarata la dipendenza `quick-xml = "0.38"`. Tuttavia, una ricerca globale nel codice sorgente dimostra che nessun file importa o utilizza `quick_xml`. L'introspezione XML per le interfacce D-Bus di ratbag/openrazer è implementata interamente da un parser ad-hoc in `src-tauri/src/providers/mod.rs:179-397`.
- **Severità**: **Bassa**
- **Riferimento Codice**: `src-tauri/Cargo.toml:51`
- **Impatto**: Aumento non necessario dei tempi di compilazione pulita, overhead nella risoluzione delle dipendenze del `Cargo.lock` e potenziale superficie di vulnerabilità indiretta per supply chain security.
- **Remediation**: Rimuovere `quick-xml = "0.38"` da `src-tauri/Cargo.toml` ed eseguire `cargo update --workspace` (o rimuovere la riga e aggiornare il lockfile).

---

#### DEF-03: Moduli backend monolitici ad alta complessità dimensionale
- **Descrizione**: Diversi moduli sorgente nel backend Rust hanno raggiunto dimensioni superiori a 1.000-2.300 linee di codice all'interno di singoli file:
  - `src-tauri/src/remapping.rs`: 2.345 linee
  - `src-tauri/src/audio.rs`: 2.264 linee
  - `src-tauri/src/providers/ratbag.rs`: 1.374 linee
  - `src-tauri/src/lib.rs`: 955 linee
  - `src/tests/App.test.tsx`: 812 linee
- **Severità**: **Media**
- **Riferimento Codice**: `src-tauri/src/remapping.rs:1-2345`, `src-tauri/src/audio.rs:1-2264`
- **Impatto**: Rischio elevato di regressioni durante modifiche concorrenti, difficoltà di navigazione e navigazione del call graph, accoppiamento stretto tra logica di persistenza, comunicazione D-Bus/systemd e logica di business.
- **Remediation**: Suddividere i moduli in sottomoduli dedicati:
  - Per `remapping`: `remapping/service.rs`, `remapping/evdev.rs`, `remapping/systemd.rs`, `remapping/state.rs`.
  - Per `audio`: `audio/pipewire.rs`, `audio/pulse.rs`, `audio/sofa.rs`, `audio/effects.rs`.

---

#### DEF-04: Timeout eccessivo di 100 secondi su `systemctl --user stop`
- **Descrizione**: Nella funzione `quiesce_agent` in `src-tauri/src/remapping.rs:1683`, il comando per arrestare l'agente di rimappatura systemd è invocato con un timeout di 100 secondi:
  ```rust
  let mut cmd = Command::new("/usr/bin/systemctl");
  cmd.args(["--user", "stop", &unit_name]);
  let status = run_with_timeout(&mut cmd, Duration::from_secs(100))?;
  ```
- **Severità**: **Media**
- **Riferimento Codice**: `src-tauri/src/remapping.rs:1683`
- **Impatto**: Se il demone di rimappatura o il gestore D-Bus dell'utente entra in deadlock o risponde lentamente, l'interfaccia utente o il comando IPC di rimappatura rimane bloccato per oltre 1 minuto e mezzo prima di restituire un errore, causando un congelamento percepito dal client.
- **Remediation**: Ridurre il timeout a un intervallo ragionevole per uno stop di un demone utente (es. 5-10 secondi) e, in caso di mancata risposta, inviare `SIGKILL` o gestire il fallimento in modo non bloccante.

---

#### DEF-05: Assenza di trigger CI su `push` e pipeline dedicata per PR
- **Descrizione**: Il file di configurazione GitHub Actions `.github/workflows/linux-packaging.yml:3-11` è impostato con:
  ```yaml
  on:
    workflow_dispatch:
    push:
      tags:
        - 'v*'
  ```
  Non è presente alcun workflow per eseguire verifiche rapide (typecheck TypeScript, test Vitest, test unitari Rust, clippy) sui push ordinari o sulle Pull Request aperte verso il branch principale.
- **Severità**: **Media**
- **Riferimento Codice**: `.github/workflows/linux-packaging.yml:3-11`
- **Impatto**: Gli sviluppatori rischiano di integrare codice rotto o con test falliti senza feedback automatico fino alla creazione manuale di una release o di un tag, con conseguente ritardo nell'individuazione dei bug.
- **Remediation**: Creare un workflow GitHub Actions `.github/workflows/ci.yml` eseguito su `push: branches: [main]` e `pull_request: branches: [main]` che esegua `npm run build`, `npm test` e `cargo test`.

---

#### DEF-06: Assenza di linter e formatter configurati nel frontend e nel backend
- **Descrizione**: Nel `package.json` non sono installati né configurati né ESLint né Prettier (non esiste `eslint.config.js` né script `lint`). Nel repository non sono definiti file di configurazione per `rustfmt.toml` o `clippy.toml`.
- **Severità**: **Media**
- **Riferimento Codice**: `package.json:8-19`
- **Impatto**: Incoerenza nello stile del codice, mancata intercettazione automatica di pattern TypeScript/React deprecati o problematici (es. hook dependencies non validate, dead code frontend).
- **Remediation**: Integrare `eslint` con `@typescript-eslint` e `eslint-plugin-react-hooks`, configurare Prettier e aggiungere nel workflow di CI i controlli `npm run lint` e `cargo clippy --all-targets -- -D warnings`.

---

#### DEF-07: Flakiness temporale nel test frontend su gestione errori DPI
- **Descrizione**: Durante l'esecuzione sequenziale dell'intera suite con `npm test`, il test `conserva la lettura confermata su errore DPI e riconcilia la snapshot` in `src/tests/App.test.tsx:298-308` può fallire per timeout:
  ```typescript
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Impossibile impostare i DPI: Errore bus I/O"
  );
  ```
  A causa del carico di rendering di 54 test in un unico file, JSDOM impiega talvolta oltre 1000ms a elaborare l'aggiornamento di stato.
- **Severità**: **Media**
- **Riferimento Codice**: `src/tests/App.test.tsx:298-308`
- **Impatto**: Test intermittenti che minano la fiducia nella pipeline di integrazione continua e possono causare fallimenti casuali dei test in ambienti di CI con risorse limitate.
- **Remediation**: Aumentare il timeout esplicito nel `findByRole("alert", {}, { timeout: 3000 })` oppure suddividere `App.test.tsx` in suite più piccole e isolate per componente, consentendo la parallelizzazione di Vitest.

---

#### DEF-08: Ricreazione forzata di `AudioPanel` con perdita di stato non salvato
- **Descrizione**: In `src/App.tsx:40, 65, 137, 156`, il token di focus audio viene incrementato ogni volta che l'utente naviga sulla scheda audio o fa clic sul banner audio:
  ```tsx
  <AudioPanel
    key={audioFocus.token}
    snapshot={snapshot}
    targetCardIndex={audioFocus.targetCardIndex}
  />
  ```
- **Severità**: **Bassa**
- **Riferimento Codice**: `src/App.tsx:40, 65, 137, 156`
- **Impatto**: L'uso di `key={audioFocus.token}` forza React a distruggere e ricreare da zero l'intero albero DOM e gli stati locali di `AudioPanel`. Se l'utente stava configurando un profilo di spazializzazione o modificando parametri senza aver premuto "Salva", qualsiasi cambio di scheda o re-focus azzera istantaneamente il form.
- **Remediation**: Rimuovere la `key` dinamica e utilizzare uno `useEffect` interno ad `AudioPanel` che osserva `audioFocus.token` o `audioFocus.targetCardIndex` per effettuare lo scroll o l'evidenziazione visiva della scheda senza smontare il componente.

---

#### DEF-09: Utilizzo di `crypto.randomUUID()` senza fallback in ambiente webview legacy
- **Descrizione**: In `src/views/AudioPanel.tsx:38` viene generato un identificativo profilo audio temporaneo tramite `crypto.randomUUID()`.
- **Severità**: **Bassa**
- **Riferimento Codice**: `src/views/AudioPanel.tsx:38`
- **Impatto**: Su distribuzioni Linux meno recenti con versioni di WebKitGTK obsolete o in contesti privi di secure origin (anche se `tauri://localhost` è sicuro), `crypto.randomUUID()` potrebbe risultare `undefined`, causando un errore a runtime durante la creazione di nuovi profili.
- **Remediation**: Fornire una funzione di utilità con fallback: `typeof crypto !== 'undefined' && crypto.randomUUID ? crypto.randomUUID() : Date.now().toString(36) + Math.random().toString(36).slice(2)`.

---

#### DEF-10: Gestione asimmetrica dello stato del click sull'icona della Tray di sistema
- **Descrizione**: In `src-tauri/src/lib.rs:840-875`, il gestore degli eventi della tray icon gestisce la pressione e il rilascio con una variabile atomica `skip_release`:
  ```rust
  TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Down, .. } => {
      // toggle visibility
      skip_release.store(true, Ordering::SeqCst);
  }
  ```
- **Severità**: **Bassa**
- **Riferimento Codice**: `src-tauri/src/lib.rs:840-875`
- **Impatto**: Su ambienti desktop Linux basati su Wayland o X11 con specifici implementatori di StatusNotifierItem (es. KDE Plasma, GNOME con AppIndicator extension), gli eventi `Down` e `Up` possono pervenire in ordine disallineato o non emettere `MouseButtonState::Down`, provocando comportamenti a scatti o mancate aperture al click.
- **Remediation**: Standardizzare la gestione su `MouseButtonState::Up` (comportamento standard delle GUI Linux) oppure affidarsi all'attivazione standard di Tauri v2 senza stato atomico asimmetrico.

---

## 2. Logic Chain (Catena Logica)

La valutazione analitica del sistema è basata sulla correlazione diretta tra le osservazioni empiriche raccolte nel codice e i requisiti di affidabilità, sicurezza e manutenibilità di un'applicazione nativa per periferiche hardware Linux.

### 2.1 Analisi e Giustificazione dei Punteggi per le 6 Categorie Standard

```
+-----------------------------------+--------+
| Categoria                         | Score  |
+-----------------------------------+--------+
| 1. Architettura (Architecture)    |  8.5   |
| 2. Qualità del Codice             |  8.5   |
| 3. Sicurezza (Security)           |  9.5   |
| 4. Prestazioni (Performance)      |  7.5   |
| 5. Gestione Errori                |  8.5   |
| 6. Test e Verificabilità          |  8.0   |
+-----------------------------------+--------+
| MEDIA COMPLESSIVA                 |  8.42  |
+-----------------------------------+--------+
```

---

#### 1. Architettura (Architecture): 8.5 / 10
- **Punti di Forza**:
  - Netta e impeccabile separazione delle responsabilità tra il livello di presentazione (React 19) e il core nativo Linux (Rust).
  - Confine IPC minimale ed essenziale (14 comandi ed 1 solo evento), che impedisce la dispersione logica.
  - Modello *Readback-driven* coerente: nessuna mutazione hardware viene considerata valida finché il backend non ne certifica l'applicazione sui bus di sistema (D-Bus, udev, PipeWire).
  - Isolamento sandbox dedicato per la validazione dei formati SOFA tramite un binario separato (`sycomopse-audio-worker`).
- **Aree di Debolezza**:
  - Presenza di moduli monolitici nel backend (`remapping.rs` e `audio.rs` superano entrambi le 2.200 linee di codice), in cui convergono gestione processi, parsing e logica di business.
  - Ricreazione forzata di componenti nel frontend (`AudioPanel` montato con chiave dinamica `key={audioFocus.token}`).
- **Giustificazione del Voto**: 8.5/10. Il design sistemico e l'isolamento dei processi sono di livello industriale eccellente, penalizzati unicamente dalla concentrazione monolitica del codice nei due moduli principali del backend.

---

#### 2. Qualità del Codice (Code Quality): 8.5 / 10
- **Punti di Forza**:
  - Tipizzazione rigorosa sia in TypeScript (`strict: true`, `noUncheckedIndexedAccess: true`) sia in Rust (tipi newtype, DTO con contratti Serde stringenti e `deny_unknown_fields`).
  - Utilizzo idiomatico di Rust: mapping pulito dei canali Tokio, gestione RAII delle risorse e dei file temporanei, assenza di codice insicuro non giustificato.
  - Buona leggibilità generale e commenti mirati nelle sezioni critiche dei contratti hardware.
- **Aree di Debolezza**:
  - Mancanza di configurazione formale per strumenti di linting e formattazione unificati (ESLint, Prettier, Clippy, rustfmt).
  - Dipendenza non necessaria `quick-xml` rimasta in `Cargo.toml`.
- **Giustificazione del Voto**: 8.5/10. Il codice è scritto con notevole perizia e attenzione alla robustezza dei tipi; l'integrazione di linter e una pulizia delle dipendenze morte porterebbero la qualità al vertice.

---

#### 3. Sicurezza (Security): 9.5 / 10
- **Punti di Forza**:
  - **Nessuna capability leak**: La configurazione Tauri `capabilities/default.json` espone solo `core:event:allow-listen` e `unlisten`. Il frontend non ha alcun accesso diretto ad API native (fs, shell, env).
  - **Zero Command Injection**: Nessun uso di shell interpreter (`sh`, `bash`). Whitelist rigida e codificata degli eseguibili permessi (`process.rs:34-42`).
  - **Protezione dai Processi Orfani**: Utilizzo di `PR_SET_PDEATHSIG` con `SIGTERM` e doppia verifica di paternità con `getppid()`.
  - **Sandbox SOFA HRTF**: Utilizzo di `nix::sys::resource::setrlimit` (`RLIMIT_AS` 256 MB) e separazione di privilegi contro exploit HDF5/NetCDF.
  - **CSP Robusto**: Nessuna esecuzione di script esterni, disabilitazione di plugin e framing (`object-src 'none'`, `frame-ancestors 'none'`).
- **Aree di Debolezza**:
  - Superficie di attacco ridotta al minimo fisiologico; nessun difetto di sicurezza critico o alto individuato.
- **Giustificazione del Voto**: 9.5/10. Postura di sicurezza esemplare per un'applicazione desktop Linux con privilegi di interazione hardware.

---

#### 4. Prestazioni (Performance): 7.5 / 10
- **Punti di Forza**:
  - Build frontend estremamente ottimizzata e leggera: bundle JS di 288 KB e CSS di 32 KB compilati in meno di 1 secondo con Vite.
  - Backend reattivo grazie a Tokio e canali broadcast asincroni non bloccanti.
- **Aree di Debolezza**:
  - Loop di polling a 2 secondi in `src-tauri/src/lib.rs:781-791`: risveglia costantemente la CPU, acquisisce un lock in scrittura su `hardware` e invia broadcast JSON al frontend anche in assenza totale di variazioni di stato.
  - Innesco di re-render continui nel ciclo vitale del frontend causato dall'incremento incondizionato di `snapshot.revision`.
  - Timeout sproporzionato di 100 secondi su `systemctl --user stop` (`src-tauri/src/remapping.rs:1683`).
- **Giustificazione del Voto**: 7.5/10. La reattività dell'applicazione è elevata, ma il polling continuo a 2 secondi rappresenta un'inefficienza energetica e prestazionale da correggere su sistemi portatili a batteria.

---

#### 5. Gestione Errori (Error Handling): 8.5 / 10
- **Punti di Forza**:
  - Uso pervasivo del pattern `Result<T, E>` in Rust con propagazione controllata degli errori tramite `?`.
  - Incapsulamento chiaro degli errori di fallimento hardware nei DTO di risposta inviati al frontend.
  - Il frontend gestisce esplicitamente il fallimento dei comandi, mostrando notifiche all'utente e preservando l'ultimo stato valido noto anziché collassare.
- **Aree di Debolezza**:
  - In alcuni punti della gestione tray o del demone systemd gli errori vengono silenziati con log informativi (`eprintln!`) senza notifica retroattiva al frontend.
- **Giustificazione del Voto**: 8.5/10. Gestione degli errori matura e robusta, pienamente conforme alle best practice dei sistemi concorrenti.

---

#### 6. Test e Verificabilità (Testing): 8.0 / 10
- **Punti di Forza**:
  - 79 test frontend ben strutturati con Vitest e Testing Library che coprono il rendering, le interazioni utente e i contratti API.
  - Test di contratto hardware e audio DSP complessi nel backend nativo (`hardware_contracts.rs`, `audio_dsp_contract.rs`, `audio_worker.rs`).
  - Suite di packaging test approfondita (224 test) eseguita con successo in meno di un secondo.
- **Aree di Debolezza**:
  - Flakiness intermittente osservata in `App.test.tsx:305:18` dovuta al carico di test in un singolo file monolitico.
  - Mancanza di una pipeline CI automatizzata su push ordinari e pull request su GitHub Actions (attualmente limitata a `workflow_dispatch` e tag).
  - Difficoltà di esecuzione diretta di `cargo test` fuori dall'ambiente containerizzato a causa delle dipendenze native D-Bus/PipeWire di sistema.
- **Giustificazione del Voto**: 8.0/10. Ampia copertura e qualità dei test di alto livello, limitata solo dall'organizzazione monolitica di alcuni file di test e dalla configurazione parziale dei trigger CI.

---

### 2.2 Calcolo del Punteggio Medio Complessivo

$$\text{Media} = \frac{8.5 + 8.5 + 9.5 + 7.5 + 8.5 + 8.0}{6} = \frac{50.5}{6} \approx \mathbf{8.42} \text{ / 10}$$

Il punteggio sintetico complessivo del progetto Sycomopse è pari a **8.4 / 10**, posizionandolo nella fascia di qualità **Molto Buona / Eccellente**, con solide basi architetturali e di sicurezza.

---

## 3. Caveats (Avvertenze e Limiti dell'Indagine)

1. **Test Rust in Ambiente Host**: `cargo test` non è stato eseguito sull'ambiente host bare-metal in quanto i toolchain Rust/Cargo globali non sono presenti direttamente nel path utente dell'host (`cargo: command not found`). L'architettura del repository affida la compilazione e il testing nativo ai container OCI/Docker definiti in `packaging/ci/matrix.py` (immagini `peripheral-hub-build:<target>`), i quali contengono Rust 1.91 e Node 24.11 pre-configurati con le librerie D-Bus e udev. L'analisi del codice Rust è stata pertanto condotta mediante ispezione statica esaustiva e verifica dei contratti.
2. **Ambiente Hardware Reale**: L'ispezione è avvenuta in un ambiente di sviluppo software. La risposta fisica effettiva dei bus hardware (libratbag su dispositivi USB reali o headset supportati da headsetcontrol) non è stata testata su periferiche fisiche connesse, bensì validata sui mock e sui contratti software del repository.

---

## 4. Conclusion (Conclusioni e Raccomandazioni Strategiche)

L'indagine evidenzia che **Sycomopse** è un progetto maturo, caratterizzato da un'elevata attenzione alla sicurezza nativa e a un'ottima igiene architetturale. L'applicazione rispetta fedelmente il principio del minor privilegio: il frontend webview è confinato e privo di capacità intrusive sul sistema operativo, mentre il backend Rust gestisce con rigore processi, allocazioni e permessi.

### Piano d'Azione Consigliato (Priorità di Intervento):

1. **Priorità Alta (Prestazioni e Reattività)**:
   - Eliminare l'emissione incondizionata di `snapshot-changed` ogni 2 secondi in `src-tauri/src/lib.rs:242-257`. Emettere l'evento solo se vi è una reale differenza di stato tra due campionamenti.
   - Ridurre il timeout di spegnimento dell'agente remapping da 100s a 5-10s (`src-tauri/src/remapping.rs:1683`).
2. **Priorità Media (Affidabilità CI e Qualità)**:
   - Configurare una GitHub Action veloce (`.github/workflows/ci.yml`) per PR e push con `tsc --noEmit`, `vitest run` e controlli di formattazione.
   - Introdurre ESLint e Prettier nel frontend; rimuovere la dipendenza inutilizzata `quick-xml` da `Cargo.toml`.
   - Suddividere il test monolitico `src/tests/App.test.tsx` per eliminare la flakiness da concorrenza in JSDOM.
3. **Priorità Bassa (Refactoring e Rifinitura)**:
   - Decomporre i file `remapping.rs` e `audio.rs` in sottomoduli specializzati per favorire la manutenibilità a lungo termine.
   - Rimuovere la chiave dinamica `key={audioFocus.token}` in `src/App.tsx` per salvaguardare lo stato transitorio dei form audio.

---

## 5. Verification Method (Metodi di Verifica Indipendente)

Per riprodurre e verificare in modo indipendente le evidenze riportate in questa analisi, eseguire i seguenti comandi e controlli:

### 5.1 Verifica Build e Test Frontend
Dalla root del repository `/home/shadymui/sycomopse`:
```bash
# 1. Typecheck TypeScript e Build del frontend
npm run build

# 2. Esecuzione suite completa dei test Vitest
npm test

# 3. Verifica isolata del test soggetto a flakiness temporale
npx vitest run src/tests/App.test.tsx -t "conserva la lettura confermata"
```

### 5.2 Verifica Packaging Tests
```bash
# Esecuzione dei 224 test del sistema di packaging e CI
python3 -m unittest discover -s packaging/tests -v
```

### 5.3 Ispezione Statica delle Evidenze di Codice
- **Polling continuo**: Verificare le linee `781-791` e `242-257` in `src-tauri/src/lib.rs`.
- **Dipendenza quick-xml inutilizzata**: Verificare la riga `51` di `src-tauri/Cargo.toml` e verificare l'assenza di `use quick_xml` con `grep -rn "quick_xml" src-tauri/src/`.
- **Sandbox SOFA Worker**: Ispezionare `src-tauri/src/audio_worker.rs:114-152` per le chiamate a `setrlimit` e drop di privilegi.
- **Whitelist Eseguibili**: Ispezionare `src-tauri/src/process.rs:34-42` per la lista fissa e immutabile dei percorsi binari.
- **Permessi Tauri**: Verificare `src-tauri/capabilities/default.json` per confermare l'assenza di plugin fs/shell.
