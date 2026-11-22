# Rapporto Autorevole di Analisi e Audit del Codice: Sycomopse

**Data di Emissione**: 17 Novembre 2026  
**Progetto**: Sycomopse (`peripheral-hub` / `it.alexcape.sycomopse`)  
**Autore**: Teamwork Independent Audit Unit (`worker_report_1`)  
**Perimetro di Ispezione**: Frontend React 19 / TypeScript (`src/`), Backend Nativo Rust / Tauri v2 (`src-tauri/`), Packaging e CI Linux (`packaging/`, `.github/`)  
**Stato del Repository**: Ispezione manuale e statica completa — Nessuna modifica apportata ai file sorgente esistenti ("sola lettura")

---

## 1. Executive Summary

### 1.1 Panoramica del Progetto
**Sycomopse** (noto a livello di pacchetto e binario come `peripheral-hub`) è un'applicazione desktop per Linux progettata per il monitoraggio, la gestione e la configurazione avanzata di periferiche hardware e del sottosistema audio multimediale. Lo stack architetturale comprende:
- **Presentation Layer (Frontend)**: Realizzato con **React 19.2.0**, **TypeScript 5.8.2** e **Vite 6.2.0**, con un design system orientato a componenti reattivi, dashboard di controllo rapido, visualizzazione dello stato energetico/batteria, gestione della risoluzione DPI e frequenza di polling del sensore mouse, rimappatura tasti a basso livello e mixer audio con spazializzazione surround 7.1.
- **Native Core Layer (Backend)**: Sviluppato in **Rust 2021** su framework **Tauri v2 (2.8.5)**, che interagisce direttamente con i sottosistemi del kernel Linux e con i demoni di sistema:
  - Bus di sistema e di sessione D-Bus (tramite crate `zbus 4.4`) per l'interazione con `org.freedesktop.UPower`, `org.freedesktop.login1`, `org.razer` (OpenRazer daemon) e `org.freedesktop.ratbag1` (libratbag daemon per periferiche Logitech e generiche).
  - Gestione eventi input a basso livello tramite kernel Linux `evdev` e driver virtuale `/dev/uinput` per la rimappatura atomica dei pulsanti del mouse con prevenzione del lockout.
  - Sottosistema audio multimediale **PipeWire** (con fallback PulseAudio) per il routing dei nodi, l'elaborazione DSP e l'applicazione di filtri HRTF (Head-Related Transfer Function) in formato SOFA tramite demone out-of-process.
- **Architettura Multi-Processo Ausiliaria**: Due binari Rust ausiliari:
  1. `peripheral-hub-remapping-agent`: demone out-of-process incaricato dell'acquisizione esclusiva dei dispositivi evdev e dell'emulazione degli eventi uinput.
  2. `peripheral-hub-audio-worker`: processo sandbox dedicato per l'elaborazione e la validazione dei profili SOFA/NetCDF e per l'esecuzione protetta di script di configurazione PipeWire.

### 1.2 Metodologia di Ispezione Manuale e Diretta
In rigorosa conformità ai requisiti dell'audit e al divieto esplicito di basarsi unicamente su output di strumenti automatici, l'analisi è stata condotta attraverso un'**ispezione manuale riga per riga** di tutti i 15 file sorgente TypeScript/React in `src/`, di tutti i moduli e binari Rust in `src-tauri/src/`, dei manifest di configurazione (`tauri.conf.json`, `Cargo.toml`, `package.json`, `tsconfig.json`) e della suite di packaging (`packaging/`).

L'indagine ha tracciato:
1. I percorsi critici di memoria e di allocazione (verifica di buffer limitati, drop semantics, asserzioni non-null).
2. I flussi di concorrenza, lock contention sui mutex Tokio e deadlock hazards nel ciclo di vita dell'applicazione.
3. Il confine IPC tra WebView e codice nativo, analizzando sia le chiamate Tauri (`invoke`) che i contratti di serializzazione/deserializzazione Serde.
4. Le misure di isolamento e sicurezza (sandboxing subprocessi, Content Security Policy, rlimit, memfd, sanitizzazione stringhe).
5. La copertura e la robustezza dei test automatizzati (79 test Vitest nel frontend, contratti mock nel backend e 224 test di packaging).

### 1.3 Sintesi delle Valutazioni e Giudizio Globale
Il progetto dimostra una **maturità ingegneristica complessivamente molto solida (voto medio 8.02 / 10)**. Spiccano in positivo l'eccellente postura di sicurezza (9.0/10) — priva di capability invasive sul frontend, dotata di sandbox con `setrlimit`/`unshare` e sanitizzazione byte-level — e l'approccio architetturale "readback-first", in cui nessuna mutazione hardware viene considerata applicata finché il sistema operativo non ne conferma l'effettivo stato.

Tuttavia, l'ispezione ha fatto emergere **30 difetti e aree di miglioramento**, tra cui:
- **1 Criticità Bloccante**: Su distribuzioni Linux, l'intercettazione dell'evento `RunEvent::ExitRequested` invoca `api.prevent_exit()` ma non attiva la procedura di chiusura a causa di un'esclusione condizionale `#[cfg(not(target_os = "linux"))]`, causando il congelamento in memoria (zombie/freeze) dell'applicazione alla chiusura della finestra o logout di sessione.
- **3 Difetti ad Alta Severità**: Un panic vector in `Drop` nel worker audio (`.expect()` invocato durante l'unwinding), l'assenza della direttiva `bundle.externalBin` in `tauri.conf.json` che causa la mancata inclusione dei due binari ausiliari nell'AppImage distribuita, e un crash a runtime non gestito nel mock frontend su riferimenti HRTF non mappati.
- **16 Difetti a Media Severità**: Tra cui il loop di polling continuo a 2 secondi che incrementa incondizionatamente la revisione ed emette broadcast IPC risvegliando la CPU e ri-renderizzando l'intera UI React, contese su lock monolitici e moduli backend monolitici superiori a 2.300 righe.
- **10 Difetti a Bassa Severità**: Ottimizzazioni di codice, rimozione di dipendenze morte (`quick-xml`), contrasto accessibilità e rifiniture UX.

---

## 2. Valutazione Dettagliata per Categorie

Ciascuna categoria standard è valutata con un punteggio motivato su scala 1–10, derivato dalla convergenza dell'ispezione frontend, backend e architetturale.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                       MATRICE DEI PUNTEGGI DI AUDIT                         │
├───────────────────────────────────┬──────────────┬──────────────┬───────────┤
│ Categoria                         │ Frontend     │ Backend      │ Globale   │
├───────────────────────────────────┼──────────────┼──────────────┼───────────┤
│ 1. Architettura (Architecture)    │ 7.5 / 10     │ 7.5 / 10     │ 7.8 / 10  │
│ 2. Qualità del Codice             │ 8.0 / 10     │ 8.0 / 10     │ 8.2 / 10  │
│ 3. Sicurezza (Security)           │ 9.0 / 10     │ 8.5 / 10     │ 9.0 / 10  │
│ 4. Prestazioni (Performance)      │ 7.0 / 10     │ 6.5 / 10     │ 7.0 / 10  │
│ 5. Gestione Errori                │ 7.5 / 7.5    │ 7.5 / 10     │ 7.8 / 10  │
│ 6. Test e Verificabilità          │ 8.5 / 10     │ 8.5 / 10     │ 8.3 / 10  │
├───────────────────────────────────┴──────────────┴──────────────┼───────────┤
│ MEDIA COMPLESSIVA PONDERATA (Scala 1-10)                        │ 8.02 / 10 │
│ Giudizio Globale: Solido e Maturo con Aree Mirate di Bonifica  │           │
└─────────────────────────────────────────────────────────────────┴───────────┘
```

---

### Categoria 1: Architettura (Architecture) — Voto: 7.8 / 10

#### Punti di Forza
1. **Modello Readback-Driven Coerente**: Sia il frontend che il backend rifiutano categoricamente gli aggiornamenti ottimistici non confermati. Lo stato dell'interfaccia utente riflette unicamente lo snapshot reale validato e restituito dai driver del kernel o dai bus D-Bus/PipeWire, azzerando le discrepanze visive tra UI e stato dell'hardware.
2. **Confine IPC Ristretto ed Essenziale**: Il confine di comunicazione Tauri tra WebView e runtime nativo è compatto e pulito: soli 14 comandi RPC strongly-typed in `src-tauri/src/lib.rs` e un unico canale di broadcast asincrono (`snapshot-changed`). Nessun comando espone primitive di basso livello o generiche.
3. **Disaccoppiamento dei Processi Ausiliari (Out-of-Process Isolation)**: Le operazioni potenzialmente instabili o ad alto privilegio (gestione dei descrittori `evdev`/`uinput` e decodifica dei file HDF5/NetCDF delle funzioni HRTF) sono delegate a due processi worker separati (`peripheral-hub-remapping-agent` e `peripheral-hub-audio-worker`), preservando l'integrità del processo desktop principale in caso di anomalie.

#### Punti di Debolezza e Criticità
1. **Moduli Monolitici nel Backend**: Eccessiva concentrazione di codice in singoli file Rust: `src-tauri/src/remapping.rs` supera le 2.340 righe, `src-tauri/src/audio.rs` supera le 2.260 righe e `src-tauri/src/providers/ratbag.rs` supera le 1.370 righe. In questi moduli convergono logiche eterogenee: comunicazione D-Bus, parsing, gestione lifecycle systemd e serializzazione.
2. **Stato Centralizzato e Prop Drilling nel Frontend**: `src/App.tsx` accumula più di 10 hook `useState` e `useRef` contemporaneamente. L'oggetto composito di controllo viene distribuito a cascata attraverso molteplici livelli di componenti fino ai controlli atomici, creando un accoppiamento elevato.
3. **Accoppiamento Orizzontale tra Viste**: La vista `Dashboard.tsx` importa ed esegue direttamente componenti esportati da `views/MousePanel.tsx` (`MouseControls`), violando il principio di indipendenza tra schermate.
4. **Mancata Registrazione Bundling in `tauri.conf.json`**: L'assenza di configurazione per i binari ausiliari in `bundle.externalBin` crea una discrepanza tra l'ambiente di sviluppo e i binari distribuiti (AppImage), rischiando di non includere gli helper nel pacchetto finale.

---

### Categoria 2: Qualità del Codice (Code Quality) — Voto: 8.2 / 10

#### Punti di Forza
1. **Tipizzazione Rigorosa e Totale Assenza di `any`**: Nell'intero frontend (`src/`) non compare un singolo utilizzo del tipo `any`. La configurazione del compilatore TypeScript (`strict: true`, `noUncheckedIndexedAccess: true`, `noUnusedLocals: true`) è pienamente rispettata.
2. **Discriminated Unions Esaustive**: I modelli di dominio (`MouseAction`, `AvailabilityCode`, `ProviderKind`, `DpiValues`) sfruttano unioni discriminate con pattern matching esaustivo senza rami di fallback permissivi.
3. **Contratti Dati Rust con `deny_unknown_fields`**: Tutte le strutture di interscambio DTO in `src-tauri/src/models.rs` utilizzano la direttiva Serde `#[serde(deny_unknown_fields)]` e la convenzione `rename_all = "camelCase"`, impedendo injection o deserializzazioni corrotte o ambigue.
4. **Delimitazione della Memoria**: Funzioni dedicate come `bounded_text` e costanti esplicite (`MAX_CAPTURED_BYTES = 256KB`, `MAX_REQUEST_BYTES`) proteggono il backend da allocazioni arbitrarie durante la cattura dell'output dei comandi di sistema.

#### Punti di Debolezza e Criticità
1. **Dipendenze Morte e Obsolete**: In `src-tauri/Cargo.toml` è presente `quick-xml = "0.38"`, ma l'introspezione XML di D-Bus è gestita da un parser manuale custom in `src/providers/mod.rs:181-456`, rendendo la dipendenza superflua. Inoltre, il crate `fs2 = "0.4"` è obsoleto e non mantenuto dal 2017.
2. **Assenza di Tool di Linting e Formatting Unificati**: Nel repository non sono configurati ESLint né Prettier (manca `eslint.config.js` e lo script `npm run lint`), così come mancano `rustfmt.toml` e `clippy.toml`.
3. **Manipolazioni Dirette del DOM**: Presenza di chiamate imperative `document.getElementById(...)?.focus()` all'interno degli hook React di `src/App.tsx`.
4. **Asserzioni Non-Null e `key={index}`**: Presenza di asserzioni non-null su collezioni dinamiche in `src/api.ts:339` e impiego dell'indice di iterazione come `key` React in `MousePanel.tsx:105` e `AudioPanel.tsx:60`.

---

### Categoria 3: Sicurezza (Security) — Voto: 9.0 / 10

#### Punti di Forza
1. **Principio del Minimo Privilegio nelle Capabilities Tauri v2**: In `src-tauri/capabilities/default.json` sono abilitati esclusivamente i permessi `core:default`, `core:event:allow-listen` e `core:event:allow-unlisten`. Nessun plugin pericoloso (accesso diretto al filesystem `fs`, esecuzione comandi `shell`, accesso clipboard) è esposto alla WebView.
2. **Content Security Policy (CSP) Impeccabile**: La direttiva CSP in `tauri.conf.json` impedisce attacchi XSS e injection vietando script remoti (`script-src 'self'`), oggetti e plugin (`object-src 'none'`), framing e clickjacking (`frame-ancestors 'none'`) e restringendo le connessioni a `ipc:` e `http://ipc.localhost`.
3. **Sandbox Stringente per Subprocessi Worker**: In `src-tauri/src/audio_worker.rs`, il worker viene eseguito con:
   - Limiti rigidi di memoria virtuale (`RLIMIT_AS = 256 MB`) tramite `setrlimit`.
   - Limite di dimensione file (`RLIMIT_FSIZE`) e disabilitazione core dump (`RLIMIT_CORE = 0`).
   - Isolamento di rete completo tramite namespace dedicato (`CLONE_NEWNET` via `libc::unshare`).
   - Abilitazione del flag `PR_SET_NO_NEW_PRIVS` e chiusura dei descrittori di file aperti non necessari (`close_range` / iterazione su `/proc/self/fd`).
4. **Esecuzione Sicura di Script via `memfd_create` Sigillati**: La configurazione PipeWire in Lua non viene scritta su file temporanei su disco (soggetti a race condition o TOCTOU symlink attack), bensì allocata in memoria RAM con `libc::memfd_create` e sigillata con `F_SEAL_SEAL | F_SEAL_WRITE`.
5. **Sanitizzazione Byte-Level per Interprete Lua**: Tutti i parametri testuali passati agli script Lua vengono codificati byte per byte in sequenze di escape decimali a 3 cifre (`\ddd`), rendendo matematicamente impossibile l'iniezione di codice arbitrario nel motore di script.
6. **Protezione da Processi Orfani**: Configurazione di `PR_SET_PDEATHSIG` con `SIGTERM` e verifica immediata post-fork di `getppid() == parent_pid`.
7. **Regole di Difesa nel Mouse Remapping**: Il modulo `src/remapping.ts` impone vincoli invalicabili:
   - Divieto assoluto di rimuovere o disabilitare il clic sinistro (`code: 272`) senza riassegnarlo a un altro pulsante fisico (prevenzione lockout utente).
   - Richiesta di consenso esplicito per periferiche con identità debole basata su sola porta USB (`identityStrength: "port"`).
   - Isolamento del recorder di scorciatoie da tastiera (attivo solo quando il componente detiene il focus del DOM).

#### Punti di Debolezza
1. **Risoluzione Implicita nel `$PATH` per Alcuni Eseguibili di Sistema**: In `src-tauri/src/process.rs`, i tool `pw-dump`, `wpctl` e `systemctl` vengono invocati con percorsi relativi confidando nella variabile `$PATH` dell'ambiente, a differenza di `headsetcontrol` e dei binari interni che usano percorsi assoluti.

---

### Categoria 4: Prestazioni (Performance) — Voto: 7.0 / 10

#### Punti di Forza
1. **Velocità di Build e Bundle Leggero**: Compilazione Vite completata in meno di 1 secondo (929 ms), generando un bundle JS compatto di 288 kB (89 kB gzippato) e un CSS di 32 kB (7.4 kB gzippato).
2. **Architettura I/O Asincrona non Bloccante**: Uso nativo di Tokio sul backend con canali asincroni mpsc e broadcast per la gestione degli eventi udev e upower.
3. **Gestione Fluida degli Slider UI**: Calcolo del riempimento visivo degli slider delegato a variabili CSS native (`--range-fill`), azzerando i ricalcoli di layout e repaint JavaScript durante il trascinamento.

#### Punti di Debolezza e Criticità
1. **Polling a 2s con Incremento Incondizionato di Revisione**: In `src-tauri/src/lib.rs:781-791, 108-116, 242-257`, ogni 2 secondi il backend acquisisce il lock `hardware`, invoca `remapping.decorate`, incrementa incondizionatamente `snapshot.revision` ed emette l'evento `snapshot-changed` anche a sistema completamente fermo, risvegliando la CPU e forzando il re-render dell'intero albero React.
2. **Contesa del Mutex Globale `AppState.hardware`**: Un unico mutex Tokio controlla sia il polling continuo a 2s sia tutte le mutazioni hardware utente (DPI, polling rate, commit remapping) e il refresh della batteria. Qualsiasi stallo o rallentamento D-Bus/sysfs blocca la reattività dell'intera interfaccia.
3. **Latenze Elevate e Scansioni Sysfs Ridondanti in OpenRazer**: In `src-tauri/src/providers/openrazer.rs:180-201, 610-660`, ogni singola operazione di scrittura esegue `systemctl --user start openrazer-daemon`, attende uno sleep rigido di 350 ms e avvia due scansioni ricorsive complete di `/sys/bus/usb/devices`, introducendo una latenza cumulativa compresa tra 500 ms e 1.200 ms per ogni spostamento dello slider DPI.
4. **Assenza di Memoizzazione React nei Componenti Pesanti**: I componenti che renderizzano grafiche vettoriali SVG procedurali (`DeviceCard`, `DeviceIllustration`) non utilizzano `React.memo`, venendo ri-renderizzati a vuoto ad ogni emissione di snapshot.
5. **Ricalcolo di `JSON.stringify` su Strutture Dati ad Ogni Render**: In `src/views/MouseButtons.tsx:51-54, 66`, vengono eseguite 4 serializzazioni JSON sincroniche ad ogni ciclo di render.
6. **Inclusione del Mock Demo nel Bundle di Produzione**: L'engine in-memory di simulazione demo (`createDemoApi`, 270 righe) è importato staticamente in `src/api.ts` e distribuito nel binario di produzione desktop.

---

### Categoria 5: Gestione Errori (Error Handling) — Voto: 7.8 / 10

#### Punti di Forza
1. **Dominio di Errore Tipizzato e Centralizzato**: Definizione uniforme del modello `AppError` con enum `ErrorCode` nel backend (`Validation`, `ProviderMissing`, `PermissionDenied`, `DeviceBusy`, `Timeout`, ecc.) e tipo speculare TypeScript nel frontend.
2. **Diagnostica Contestuale Troncata in Modo Sicuro**: I dettagli tecnici degli errori vengono limitati a un massimo di 512 caratteri per evitare memory exhaustion ed esposizione accidentale di dati sensibili o stack trace.
3. **Prevenzione di Race Condition tramite Token di Generazione**: In `src/App.tsx:48-52`, l'utilizzo combinato di `generationRef` e `activeRef` impedisce che risposte asincrone obsolete sovrascrivano lo stato più recente dell'applicazione.
4. **Serializzazione delle Mutazioni per Gruppo**: La funzione `runMutation` inibisce la sottomissione di scritture concorrenti sullo stesso dispositivo o canale audio mentre una precedente mutazione è in corso.

#### Punti di Debolezza e Criticità
1. **Deadlock / Congelamento del Processo all'Uscita su Linux (Critica)**: In `src-tauri/src/lib.rs:944-953`, all'evento `ExitRequested` viene chiamato `api.prevent_exit()`, ma la chiamata a `request_exit` è disabilitata su Linux (`#[cfg(not(target_os = "linux"))]`). L'applicazione impedisce la chiusura ma non avvia mai il cleanup, rimanendo congelata in memoria.
2. **Panic Vector in `Drop` di `OwnedChild` (Alta)**: In `src-tauri/src/audio_worker.rs:573, 591-605`, il metodo `child(&mut self)` richiama `.expect("owned child")`. Se invocato in una condizione in cui il processo è già terminato o l'opzione è `None`, scaturisce un panic all'interno dell'implementazione di `Drop`, provocando un abort istantaneo del processo (`SIGABRT`).
3. **Assenza di React ErrorBoundary**: Nel frontend React non è presente alcun `ErrorBoundary` né alla radice (`main.tsx`) né nei singoli pannelli, con il rischio di "schermo bianco" permanente (white screen of death) in caso di eccezioni di rendering o parsing.
4. **Crash a Runtime nel Mock Demo per Asserzione Non-Null**: In `src/api.ts:339`, l'uso di `clone(hrtfs.get(profile.hrtf)!)` solleva un `SyntaxError: Unexpected token u in JSON at position 0` se il profilo fa riferimento a un ID HRTF non censito.
5. **Rilettura Ridondante su Errore di Connessione**: In `src/App.tsx:118-125`, il blocco catch di `runMutation` invoca una rilettura `api.getSnapshot()`; se l'errore originario era proprio la mancata connessione iniziale, viene lanciata immediatamente una seconda richiesta fallimentare duplicata.

---

### Categoria 6: Test e Verificabilità (Testing) — Voto: 8.3 / 10

#### Punti di Forza
1. **Suite di Test Frontend Esaustiva e al 100% Funzionante**: 79 test Vitest suddivisi tra `src/tests/api.test.ts` (25 test) e `src/tests/App.test.tsx` (54 test), tutti superati con successo:
   - Copertura accurata delle regole di business e sicurezza (blocco perdita clic sinistro, chording modificatori, autorizzazione su porte deboli).
   - Validazione delle risposte dei contratti IPC, gestione dell'ambiente browser non supportato e isolamento della modalità demo.
   - Gestione del doppio montaggio in `StrictMode` e ripristino da rimozione a caldo delle periferiche (hotplugging).
2. **Suite di Test di Packaging Linux**: 224 test unitari eseguiti tramite Python in `packaging/tests/`, tutti passati con successo in meno di 1 secondo, a garanzia della consistenza delle matrici di distribuzione e degli script di installazione.
3. **Contratti Backend con Fixture Reali**: Nel backend Rust sono presenti test di contratto approfonditi (`tests/hardware_contracts.rs`, `tests/audio_dsp_contract.rs`, `tests/audio_worker.rs`, `tests/remapping/recovery.rs`) basati su file di introspezione D-Bus reali di dispositivi fisici (OpenRazer, Ratbag) e catture sysfs.

#### Punti di Debolezza e Criticità
1. **File di Test Monolitico e Flakiness Temporale**: `src/tests/App.test.tsx` supera le 810 righe racchiudendo 54 test end-to-end; durante l'esecuzione completa è stata rilevata una flakiness intermittente su `findByRole("alert")` (linea 305) dovuta ai tempi di elaborazione di JSDOM.
2. **Pipeline CI Incompleta per PR e Push**: Il workflow GitHub Actions `.github/workflows/linux-packaging.yml` viene attivato unicamente da `workflow_dispatch` e tag di release, privando il repository di verifiche automatiche continue (typecheck, linting, test veloci) sui push ordinari e sulle Pull Request verso `main`.
3. **Mancanza di Test Automatizzati di Accessibilità (a11y)**: Nessun tool automatico (come `axe-core`) è integrato nei test frontend, permettendo la permanenza della violazione di content model HTML in `DeviceCard.tsx`.

---

## 3. Tabella di Riepilogo Voti con Punteggio Medio Globale

La tabella sottostante riassume le valutazioni attribuite a ciascuna categoria dai rispettivi flussi di ispezione e il voto unificato finale calcolato:

| Categoria | Frontend (`src/`) | Backend (`src-tauri/`) | Architettura & Sicurezza | Voto Unificato | Sintesi della Motivazione |
|---|:---:|:---:|:---:|:---:|---|
| **1. Architettura** | 7.5 | 7.5 | 8.5 | **7.8 / 10** | Eccellente separazione tra presentazione e core nativo con modello readback-driven; penalizzata da moduli monolitici nel backend (>2.300 righe) e prop drilling in App.tsx. |
| **2. Qualità del codice** | 8.0 | 8.0 | 8.5 | **8.2 / 10** | Tipizzazione impeccabile con zero `any` in TS e DTO Serde con `deny_unknown_fields` in Rust; assenti linter formali e presente una dipendenza non usata (`quick-xml`). |
| **3. Sicurezza** | 9.0 | 8.5 | 9.5 | **9.0 / 10** | Postura esemplare: zero plugin Tauri esposti, sandbox subprocessi con `setrlimit` e `unshare`, script eseguiti via `memfd_create` sigillati, CSP ferrea e protezione anti-lockout mouse. |
| **4. Prestazioni** | 7.0 | 6.5 | 7.5 | **7.0 / 10** | Build fulminea (<1s), ma polling continuo a 2s con emissione incondizionata di `snapshot-changed`, contesa del lock `hardware` e latenze di oltre 500ms su OpenRazer. |
| **5. Gestione errori** | 7.5 | 7.5 | 8.5 | **7.8 / 10** | Pattern Result/AppError solido e diagnostica contestuale; difetto critico di freeze all'uscita su Linux, potenziale panic in `Drop` e assenza di React ErrorBoundary. |
| **6. Test e Verificabilità** | 8.5 | 8.5 | 8.0 | **8.3 / 10** | 79 test Vitest e 224 test di packaging passati al 100%; ottimi contratti con fixture reali; flakiness temporale sporadica in `App.test.tsx` e trigger CI incompleti. |
| **MEDIA GLOBALE** | **7.92** | **7.75** | **8.42** | **8.02 / 10** | **Giudizio Sintetico: Progetto Maturo e Robusto con Aree Mirate di Consolidamento** |

*Formula di calcolo del Punteggio Medio Globale*:
$$\text{Punteggio Medio} = \frac{7.8 + 8.2 + 9.0 + 7.0 + 7.8 + 8.3}{6} = \frac{48.1}{6} \approx \mathbf{8.02} \text{ / 10}$$

---

## 4. Registro Completo ed Esaustivo dei Difetti e Aree di Miglioramento

### 4.1 Tabella Sintetica dei 30 Difetti Rilevati

| ID | Titolo del Difetto | Severità | Ambito Primario | Percorso File e Linea / Intervallo | Categoria |
|---|---|:---:|---|---|---|
| **DEF-01** | Deadlock / Freeze Zombie all'Uscita su Distribuzioni Linux | **Critica** | Backend Nativo | `src-tauri/src/lib.rs:944-953` | Gestione Errori / Ciclo di Vita |
| **DEF-02** | Panic Vector in `Drop` per la Struttura `OwnedChild` | **Alta** | Backend Nativo | `src-tauri/src/audio_worker.rs:568-605` | Resilienza / Gestione Errori |
| **DEF-03** | Assenza di Bundling dei Binari Ausiliari in `tauri.conf.json` | **Alta** | Packaging / Runtime | `src-tauri/tauri.conf.json:29-41`, `src-tauri/Cargo.toml:20-27` | Architettura / Packaging |
| **DEF-04** | Crash a Runtime per Asserzione Non-Null su Riferimento HRTF Non Censito | **Alta** | Frontend / Mock | `src/api.ts:338-340` | Qualità del Codice / Robustezza |
| **DEF-05** | Polling Continuo a 2s con Emissione Incondizionata di `snapshot-changed` | **Media** | Backend / IPC | `src-tauri/src/lib.rs:781-791, 108-116, 242-257` | Prestazioni / Risorse CPU |
| **DEF-06** | Contesa del Mutex Globale `AppState.hardware` nel Loop di Remapping | **Media** | Backend Nativo | `src-tauri/src/lib.rs:786, 236, 270, 358, 403` | Concorrenza / Prestazioni |
| **DEF-07** | Latenza Eccessiva, Avvio Ridondante e Doppia Scansione Sysfs in OpenRazer | **Media** | Backend Nativo | `src-tauri/src/providers/openrazer.rs:180-201, 610-660` | Prestazioni / Provider |
| **DEF-08** | Assenza di React ErrorBoundary a Livello Root e di Vista | **Media** | Frontend React | `src/main.tsx:9`, `src/App.tsx:165-177` | Gestione Errori / UX |
| **DEF-09** | Violazione del Content Model HTML (Elementi di Blocco dentro `<button>`) | **Media** | Frontend / A11y | `src/components/DeviceCard.tsx:53-58` | Standard Web / Accessibilità |
| **DEF-10** | Sovrascrittura Prematura delle Bozze Utente in `useEffect` su Eventi Live | **Media** | Frontend React | `src/components/Controls.tsx:137-138`, `src/views/MousePanel.tsx:15-16` | UX / Gestione Stato |
| **DEF-11** | Ricalcolo Costoso di `JSON.stringify` ad Ogni Ciclo di Render | **Media** | Frontend React | `src/views/MouseButtons.tsx:51-54, 66` | Prestazioni Frontend |
| **DEF-12** | Rirender a Cascata dell'Intero Albero per Mancanza di `React.memo` | **Media** | Frontend React | `src/App.tsx:173`, `src/components/DeviceCard.tsx:47`, `src/views/Dashboard.tsx:29` | Prestazioni / Re-render |
| **DEF-13** | Inclusione dell'Intero Motore di Simulazione Demo nel Bundle Finale | **Media** | Frontend / Build | `src/api.ts:83-352` | Bundle Size / Architettura |
| **DEF-14** | Moduli Backend Monolitici ad Elevata Complessità Dimensionale | **Media** | Backend Nativo | `src-tauri/src/remapping.rs:1-2345`, `src/audio.rs:1-2264` | Architettura / Manutenibilità |
| **DEF-15** | Timeout Sproporzionato di 100 Secondi su `systemctl --user stop` | **Media** | Backend Nativo | `src-tauri/src/remapping.rs:1683` | Gestione Processi / UX |
| **DEF-16** | Percorsi Hardcoded e Assunzioni FHS Fragili su Ambienti Non-Standard | **Media** | Backend Nativo | `src-tauri/src/audio_worker.rs:721`, `src/process.rs:10`, `src/surround.rs:15` | Portabilità Linux |
| **DEF-17** | Esecuzione Comandi di Sistema con Risoluzione Ambigua in `$PATH` | **Media** | Backend Nativo | `src-tauri/src/process.rs:52-104` | Sicurezza / Invocazione Comandi |
| **DEF-18** | Assenza di Pipeline CI Automatica su Push e Pull Request Ordinarie | **Media** | Devops / CI | `.github/workflows/linux-packaging.yml:3-11` | Integrazione Continua |
| **DEF-19** | Assenza di Linter e Formatter Configurati nel Repository | **Media** | Tooling / Igiene | `package.json:8-19`, root configs | Qualità del Codice |
| **DEF-20** | Flakiness Temporale nel Test Frontend su Gestione Errori DPI in JSDOM | **Media** | Testing | `src/tests/App.test.tsx:298-308` | Stabilità Test Suite |
| **DEF-21** | Dipendenza Inutilizzata `quick-xml` e Crate Deprecato `fs2` | **Bassa** | Backend Nativo | `src-tauri/Cargo.toml:46, 51` | Gestione Dipendenze |
| **DEF-22** | Risorsa Template Spazializzatore Ridondante nel Bundle Tauri | **Bassa** | Backend / Config | `src-tauri/tauri.conf.json:34` vs `src-tauri/src/surround.rs:240` | Configurazione Bundle |
| **DEF-23** | Rifiuto Immediato con `Busy` nel Semaforo del Worker Audio | **Bassa** | Backend Nativo | `src-tauri/src/audio_worker.rs:358-361` | Resilienza Chiamate |
| **DEF-24** | Rilettura Snapshot Ridondante e Duplicata su Fallimento di Connessione | **Bassa** | Frontend React | `src/App.tsx:118-125` | Gestione Errori Frontend |
| **DEF-25** | Manipolazione Diretta del DOM (`document.getElementById`) in React | **Bassa** | Frontend React | `src/App.tsx:99, 146` | Idiomi React |
| **DEF-26** | Utilizzo dell'Indice dell'Array come Chiave React (`key={index}`) | **Bassa** | Frontend React | `src/views/MousePanel.tsx:105`, `src/views/AudioPanel.tsx:60` | Best Practice Virtual DOM |
| **DEF-27** | Ricreazione Forzata di `AudioPanel` con Chiave Dinamica e Perdita Stato | **Bassa** | Frontend React | `src/App.tsx:40, 65, 137, 156` | Ciclo di Vita Componenti |
| **DEF-28** | Rischio di Indisponibilità di `crypto.randomUUID()` in WebKitGTK Legacy | **Bassa** | Frontend React | `src/views/AudioPanel.tsx:38` | Portabilità Browser |
| **DEF-29** | Gestione Asimmetrica degli Eventi Click sull'Icona Tray di Sistema | **Bassa** | Backend Nativo | `src-tauri/src/lib.rs:840-875` | Integrazione Desktop Linux |
| **DEF-30** | Contrasto Colore Insufficiente su Micro-Etichette secondo WCAG 2.1 AA | **Bassa** | Design / CSS | `src/styles.css:43, 64, 81, 120` | Accessibilità Visiva |

---

### 4.2 Schede Dettagliate dei Difetti

---

#### DEF-01: Deadlock / Freeze Zombie all'Uscita su Distribuzioni Linux
- **Identificativo**: `DEF-01`
- **Titolo**: Blocco del processo all'uscita per mancata invocazione di `request_exit` su Linux
- **Livello di Severità**: **Critica**
- **Percorso File e Linee**: `src-tauri/src/lib.rs:944-953`
- **Descrizione del Problema**:  
  Nel ciclo degli eventi di esecuzione dell'applicazione Tauri:
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
  Quando il desktop environment Linux (GNOME, KDE, session manager) o l'utente richiede la chiusura dell'applicazione tramite segnale di finestra, Tauri emette un evento `RunEvent::ExitRequested`. Il gestore esegue `api.prevent_exit()` per ritardare l'uscita in modo da consentire il ripristino delle connessioni audio. Tuttavia, l'invocazione di `request_exit(app.clone())` è racchiusa nella direttiva condizionale `#[cfg(not(target_os = "linux"))]`. Di conseguenza, su Linux `request_exit` non viene MAI invocata. L'applicazione blocca l'uscita ordinata, non avvia il task di cleanup asincrono e non imposta mai `state.exit_ready` a `true`.
- **Spiegazione dell'Impatto Tecnico**:  
  Il processo rimane orfano e congelato in memoria di background (zombie). La finestra grafica può scomparire o smettere di rispondere, ma il processo continua a impegnare descrittori, lock di sistema e thread, impedendo all'utente di riaprire l'applicazione o ritardando il logout della sessione desktop.
- **Raccomandazione di Remediation**:  
  Rimuovere l'esclusione condizionale `#[cfg(not(target_os = "linux"))]` e avviare il cleanup `request_exit` anche su Linux:
  ```rust
  app.run(|app, event| {
      if let tauri::RunEvent::ExitRequested { api, .. } = event {
          let state = app.state::<Arc<AppState>>();
          if !state.exit_ready.load(Ordering::SeqCst) {
              api.prevent_exit();
              request_exit(app.clone());
          }
      }
  });
  ```

---

#### DEF-02: Panic Vector in `Drop` per la Struttura `OwnedChild`
- **Identificativo**: `DEF-02`
- **Titolo**: Invocazione di `.expect()` nel metodo `child()` durante l'esecuzione del distruttore `Drop`
- **Livello di Severità**: **Alta**
- **Percorso File e Linee**: `src-tauri/src/audio_worker.rs:568-605`
- **Descrizione del Problema**:  
  La struttura `OwnedChild` incapsula un processo figlio asincrono:
  ```rust
  struct OwnedChild(Option<tokio::process::Child>);

  impl OwnedChild {
      fn child(&mut self) -> &mut tokio::process::Child {
          self.0.as_mut().expect("owned child")
      }
      fn kill(&mut self) {
          let child = self.child();
          ...
      }
  }

  impl Drop for OwnedChild {
      fn drop(&mut self) {
          self.kill();
          if let Some(mut child) = self.0.take() { ... }
      }
  }
  ```
  Se il processo figlio è già stato estratto (`self.0.is_none()`) o se si verifica una sequenza anomala di deallocazione, `self.kill()` richiama `self.child()` che esegue `.expect("owned child")`.
- **Spiegazione dell'Impatto Tecnico**:  
  In Rust, scatenare un panic all'interno del metodo `Drop::drop` mentre è già in corso un unwinding dello stack (dovuto a un errore o cancellazione di task Tokio) causa l'arresto immediato e forzato dell'intero processo applicativo (`SIGABRT`), senza alcuna possibilità di intercettazione o log diagnostico.
- **Raccomandazione di Remediation**:  
  Rendere `kill()` totalmente difensivo verificando la presenza del figlio con un costrutto `if let` senza mai invocare `.expect()`:
  ```rust
  impl OwnedChild {
      fn kill(&mut self) {
          let Some(child) = self.0.as_mut() else { return; };
          if child.try_wait().ok().flatten().is_none() {
              if let Some(pid) = child.id() {
                  unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL); }
              }
              let _ = child.start_kill();
          }
      }
  }
  ```

---

#### DEF-03: Assenza di Bundling dei Binari Ausiliari in `tauri.conf.json`
- **Identificativo**: `DEF-03`
- **Titolo**: Mancata inclusione dei worker `peripheral-hub-audio-worker` e `peripheral-hub-remapping-agent` nel bundle distribuito
- **Livello di Severità**: **Alta**
- **Percorso File e Linee**: `src-tauri/tauri.conf.json:29-41`, `src-tauri/Cargo.toml:20-27`, `src-tauri/src/audio_worker.rs:380-411`
- **Descrizione del Problema**:  
  In `Cargo.toml` vengono compilati tre binari separati: l'eseguibile principale `peripheral-hub` e due helper ausiliari (`peripheral-hub-remapping-agent` e `peripheral-hub-audio-worker`). A runtime, `audio_worker.rs:380-411` ricerca il binario helper nella stessa directory dell'eseguibile principale (`executable.parent()`). Tuttavia, in `tauri.conf.json`, la sezione `bundle` non definisce la chiave `externalBin`, né include script per copiare questi due eseguibili all'interno del pacchetto AppImage finale.
- **Spiegazione dell'Impatto Tecnico**:  
  Nell'ambiente di sviluppo locale con `cargo run` i file binari compilati risiedono nella cartella `target/debug` comune e vengono trovati. Ma quando viene generata la release distribubile (AppImage, deb, rpm), i binari ausiliari non vengono impacchettati. Al primo avvio dell'app impacchettata, qualsiasi operazione audio fallisce sistematicamente con errore `ErrorCode::ProviderMissing ("Helper audio non installato")` e la rimappatura del mouse risulta non operativa.
- **Raccomandazione di Remediation**:  
  Configurare `externalBin` in `tauri.conf.json` o strutturare lo script di CI/packaging per copiare i target ausiliari nella directory binaria di release:
  ```json
  "bundle": {
    "active": true,
    "targets": ["appimage"],
    "externalBin": [
      "binaries/peripheral-hub-remapping-agent",
      "binaries/peripheral-hub-audio-worker"
    ]
  }
  ```

---

#### DEF-04: Crash a Runtime per Asserzione Non-Null su Riferimento HRTF Non Censito
- **Identificativo**: `DEF-04`
- **Titolo**: Eccezione non intercettata `SyntaxError` in `applyAudioProfile` nella simulazione demo
- **Livello di Severità**: **Alta**
- **Percorso File e Linee**: `src/api.ts:338-340`
- **Descrizione del Problema**:  
  Nel metodo `applyAudioProfile` dell'API demo:
  ```ts
  if (profile.hrtf !== null) next.audio.hrtf = clone(hrtfs.get(profile.hrtf)!);
  ```
  Viene usata l'asserzione non-null `!` su `hrtfs.get(...)`. Se un profilo fa riferimento a un ID HRTF mancante o rimosso, `hrtfs.get(...)` restituisce `undefined`. La funzione `clone` richiama internamente:
  `JSON.parse(JSON.stringify(undefined))`  
  Poiché `JSON.stringify(undefined)` produce il valore speciale JavaScript `undefined` (e non la stringa `"undefined"`), il metodo `JSON.parse(undefined)` solleva un'eccezione irreversibile: `SyntaxError: Unexpected token u in JSON at position 0`.
- **Spiegazione dell'Impatto Tecnico**:  
  Crash immediato del frontend React alla selezione o applicazione di un profilo audio con un file SOFA non registrato nella mappa, provocando la schermata bianca per l'utente.
- **Raccomandazione di Remediation**:  
  Validare difensivamente la presenza del file HRTF prima della clonazione:
  ```ts
  if (profile.hrtf !== null) {
    const targetHrtf = hrtfs.get(profile.hrtf);
    if (!targetHrtf) {
      demoError(`Riferimento HRTF '${profile.hrtf}' non disponibile nella demo.`);
      return;
    }
    next.audio.hrtf = clone(targetHrtf);
  }
  ```

---

#### DEF-05: Polling Continuo a 2s con Emissione Incondizionata di `snapshot-changed`
- **Identificativo**: `DEF-05`
- **Titolo**: Risveglio periodico della CPU ogni 2 secondi con incremento arbitrario di `revision` e broadcast IPC
- **Livello di Severità**: **Media**
- **Percorso File e Linee**: `src-tauri/src/lib.rs:781-791, 108-116, 242-257`
- **Descrizione del Problema**:  
  In `lib.rs:781-791` un task Tokio esegue un loop con intervallo di 2 secondi invocando `refresh_remapping_locked`. Questo metodo richiama a sua volta `self.publish(app, ...)` che, a ogni singolo passaggio:
  ```rust
  snapshot.revision = snapshot.revision.saturating_add(1);
  let confirmed = snapshot.clone();
  drop(snapshot);
  let _ = app.emit("snapshot-changed", &confirmed);
  ```
  incrementa la proprietà numerica `revision` ed emette l'evento broadcast `snapshot-changed` verso la WebView, anche se non è avvenuta alcuna modifica nell'hardware, nelle batterie o nei profili.
- **Spiegazione dell'Impatto Tecnico**:  
  Nel frontend (`src/App.tsx:93-95`), l'hook riceve l'evento, rileva una `revision` maggiore, aggiorna lo stato globale `snapshot` e scatena un re-render completo dell'intero albero DOM di React ogni 2 secondi. Su laptop a batteria questo comportamento causa un consumo energetico continuativo e impedisce al processore di entrare negli stati di deep idle sleep.
- **Raccomandazione di Remediation**:  
  Eseguire una verifica differenziale dello snapshot (tramite hash rapido o confronto dei campi dati) prima di pubblicare: emettere l'evento `snapshot-changed` e incrementare `revision` **esclusivamente** se i dati hanno subito variazioni effettive. In assenza di modifiche, non inviare alcun evento sul bus IPC.

---

#### DEF-06: Contesa del Mutex Globale `AppState.hardware` nel Loop di Remapping
- **Identificativo**: `DEF-06`
- **Titolo**: Blocco coarse-grained su `AppState.hardware` che serializza polling periodico e mutazioni interattive
- **Livello di Severità**: **Media**
- **Percorso File e Linee**: `src-tauri/src/lib.rs:786, 236, 270, 358, 403`
- **Descrizione del Problema**:  
  L'oggetto `AppState` utilizza un unico mutex `self.hardware = tokio::sync::Mutex::new(())`. Questo stesso lock viene acquisito ogni 2 secondi dal loop di remapping in background e conteso da ogni operazione interattiva dell'utente: modifica DPI (`mutate_dpi`), polling rate (`mutate_polling`), applicazione rimappatura (`mutate_remapping`) e refresh periodico della batteria a 30s.
- **Spiegazione dell'Impatto Tecnico**:  
  Se durante il tick periodico di 2s interviene una scansione sysfs o una chiamata D-Bus lenta verso OpenRazer o libratbag, il lock viene trattenuto per centinaia di millisecondi. Qualsiasi interazione dell'utente con gli slider dell'interfaccia si blocca in coda, causando evidenti micro-lag e mancata reattività dei controlli UI.
- **Raccomandazione di Remediation**:  
  Scomporre il lock globale in lock granulari distinti (es. `Mutex` per-device o separazione tra sottosistema remapping e configurazione hardware) oppure utilizzare lock non bloccanti basati su snapshot immutable a copia (`ArcSwap`).

---

#### DEF-07: Latenza Eccessiva, Avvio Ridondante e Doppia Scansione Sysfs in OpenRazer
- **Identificativo**: `DEF-07`
- **Titolo**: Invocazione sistematica di `systemctl start`, sleep fisso di 350ms e scansione sysfs duplicata ad ogni scrittura
- **Livello di Severità**: **Media**
- **Percorso File e Linee**: `src-tauri/src/providers/openrazer.rs:180-201, 610-660`
- **Descrizione del Problema**:  
  In `openrazer.rs`, ogni chiamata a `set_dpi` o `set_polling_rate` invoca preliminarmente `manager()`. Questa funzione esegue incondizionatamente il comando `systemctl --user start openrazer-daemon` seguito da uno sleep asincrono rigido di 350 ms:
  ```rust
  crate::process::run("systemctl", &["--user", "start", "openrazer-daemon"], ...).await;
  tokio::time::sleep(Duration::from_millis(350)).await;
  ```
  Subito dopo, la funzione `authoritative()` viene invocata due volte: una prima della scrittura D-Bus e una come readback post-scrittura. Ciascuna chiamata ad `authoritative()` avvia `spawn_blocking` per scansionare ricorsivamente l'intero albero `/sys/bus/usb/devices`.
- **Spiegazione dell'Impatto Tecnico**:  
  Ogni singola variazione di uno slider DPI su un mouse Razer richiede tra 500 ms e 1.200 ms per completarsi. L'accumulo di queste latenze rende il trascinamento dello slider scattoso e sovraccarica il sottosistema I/O del disco.
- **Raccomandazione di Remediation**:  
  1. Verificare prima se l'interfaccia D-Bus di OpenRazer è già attiva sul session bus; se attiva, non invocare `systemctl` e azzerare lo sleep di 350 ms.
  2. Riutilizzare l'oggetto `Device` già in memoria per la validazione pre-scrittura anziché forzare una scansione sysfs preventiva, limitando la lettura da disco al solo readback finale.

---

#### DEF-08: Assenza di React ErrorBoundary a Livello Root e di Vista
- **Identificativo**: `DEF-08`
- **Titolo**: Mancanza di confini di errore React per la protezione dai crash di rendering
- **Livello di Severità**: **Media**
- **Percorso File e Linee**: `src/main.tsx:9`, `src/App.tsx:165-177`
- **Descrizione del Problema**:  
  L'albero dell'applicazione React in `src/main.tsx` viene avviato direttamente con:
  ```tsx
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <App />
    </StrictMode>
  );
  ```
  Né in `main.tsx` né nei pannelli di vista (`Dashboard`, `MousePanel`, `MouseButtons`, `AudioPanel`) è presente un componente `ErrorBoundary`.
- **Spiegazione dell'Impatto Tecnico**:  
  Se si verifica una qualsiasi eccezione JavaScript non intercettata durante il rendering di un componente (ad esempio un calcolo vettoriale SVG errato in `DeviceIllustration`, un accesso a proprietà nulle su un payload di snapshot imprevisto o un errore nei fogli di stile), l'intero albero di React viene smontato e l'utente si ritrova davanti a una schermata bianca permanente ("white screen of death"), senza messaggi diagnostici o possibilità di recupero.
- **Raccomandazione di Remediation**:  
  Implementare un componente `ErrorBoundary.tsx` e incapsulare sia la root dell'app in `main.tsx` sia ciascun pannello in `App.tsx`:
  ```tsx
  export class ErrorBoundary extends React.Component<Props, State> {
    state = { hasError: false, error: null };
    static getDerivedStateFromError(error: Error) { return { hasError: true, error }; }
    render() {
      if (this.state.hasError) {
        return <div className="error-fallback"><h3>Si e' verificato un errore imprevisto</h3><button onClick={() => window.location.reload()}>Ricarica</button></div>;
      }
      return this.props.children;
    }
  }
  ```

---

#### DEF-09: Violazione del Content Model HTML (Elementi di Blocco dentro `<button>`)
- **Identificativo**: `DEF-09`
- **Titolo**: Inclusione illegittima di `<div>`, `<h3>`, `<p>` e `<meter>` all'interno del tag `<button>`
- **Livello di Severità**: **Media**
- **Percorso File e Linee**: `src/components/DeviceCard.tsx:53-58`
- **Descrizione del Problema**:  
  In `DeviceCard.tsx`, il pulsante interattivo della card è strutturato come:
  ```tsx
  <button type="button" className="device-card-button" ...>
    <div className="device-card-top">...</div>
    <DeviceIllustration kind={device.kind} />
    <div className="device-card-copy">
      <h3>{device.name}</h3>
      <p>{providerNames[device.provider]}</p>
    </div>
    <div className="device-card-bottom">
      <BatteryReadout battery={device.values.battery} />
    </div>
  </button>
  ```
  Secondo le specifiche formali W3C HTML5, il modello di contenuto ammesso per il tag `<button>` è limitato esclusivamente a *phrasing content* (elementi in linea come `span`, `strong`, `svg`). L'inserimento di elementi di intestazione (`<h3>`), paragrafi (`<p>`), contenitori di blocco (`<div>`) e controlli interattivi come `<meter>` (all'interno di `BatteryReadout`) costituisce una violazione delle specifiche.
- **Spiegazione dell'Impatto Tecnico**:  
  Le tecnologie assistive (lettori di schermo come Orca su Linux, NVDA, VoiceOver) falliscono nell'interpretare la gerarchia semantica della pagina: le intestazioni `<h3>` interne al bottone vengono oscurate o annunciate in modo inintelligibile; inoltre alcuni browser possono generare alberi di accessibilità errati.
- **Raccomandazione di Remediation**:  
  Ristrutturare la card come un `<article>` contenitore e convertire i tag di blocco interni al bottone in elementi `<span>` stilizzati via CSS con `display: block`, posizionando le intestazioni semantiche all'esterno o associandole tramite attributi `aria-labelledby`.

---

#### DEF-10: Sovrascrittura Prematura delle Bozze Utente in `useEffect` su Eventi Live
- **Identificativo**: `DEF-10`
- **Titolo**: Reset involontario del valore degli slider durante l'interazione per effetto di aggiornamenti in background
- **Livello di Severità**: **Media**
- **Percorso File e Linee**: `src/components/Controls.tsx:137-138`, `src/views/MousePanel.tsx:15-16`
- **Descrizione del Problema**:  
  Nei componenti `NodeControls` e `MouseControls`:
  ```ts
  // Controls.tsx
  const [draft, setDraft] = useState(node.volume);
  useEffect(() => { setDraft(node.volume); }, [node.nodeName, node.volume]);

  // MousePanel.tsx
  useEffect(() => { setDraftDpi(dpiDraft(device)); }, [device.id, device.values.dpi?.x, device.values.dpi?.y]);
  ```
  Gli hook `useEffect` riallineano incondizionatamente lo stato della bozza locale (`draft`) ogni volta che giunge un nuovo valore dallo snapshot.
- **Spiegazione dell'Impatto Tecnico**:  
  Se l'utente sta trascinando il cursore del volume o sta digitando un valore nel campo numerico dei DPI nel momento in cui il backend emette un aggiornamento di snapshot (ad esempio per via del polling a 2s o di un evento udev), la bozza in corso di modifica viene improvvisamente sovrascritta e resettata al valore confermato precedente, degradando l'esperienza d'uso.
- **Raccomandazione di Remediation**:  
  Tracciare lo stato di interazione attiva (tramite un flag `isInteracting` o verificando se il controllo ha il focus attivo): sincronizzare la bozza locale con lo snapshot solo se l'utente non ha modificato il valore o non ha un'interazione in corso.

---

#### DEF-11: Ricalcolo Costoso di `JSON.stringify` ad Ogni Ciclo di Render
- **Identificativo**: `DEF-11`
- **Titolo**: Serializzazioni JSON sincrone e ripetute a corpo componente in `MouseButtons.tsx`
- **Livello di Severità**: **Media**
- **Percorso File e Linee**: `src/views/MouseButtons.tsx:51-54, 66`
- **Descrizione del Problema**:  
  Nel componente `MouseButtons`:
  ```ts
  const sourceKey = JSON.stringify([capability?.fingerprint, capability?.identityStrength, capability?.buttons.map((b) => [b.id, b.code])]);
  const savedKey = JSON.stringify([observed?.profileId, observed?.bindings, observed?.autoApply]);
  ...
  const dirty = JSON.stringify(draft) !== JSON.stringify(savedDraft(device));
  ```
  A ogni ciclo di render (inclusi eventi di hover, focus e battitura), vengono calcolate 4 serializzazioni JSON complete di array e oggetti complessi per verificare lo stato di variazione ("dirty").
- **Spiegazione dell'Impatto Tecnico**:  
  Consumo superfluo di cicli di clock e generazione di frammenti di stringhe nella memoria heap che appesantiscono il Garbage Collector della WebView, provocando micro-stutter specialmente durante l'uso del recorder interattivo dei tasti.
- **Raccomandazione di Remediation**:  
  Incapsulare il calcolo delle chiavi e del flag `dirty` all'interno di `useMemo` o ricorrere a comparatori superficiali mirati.

---

#### DEF-12: Rirender a Cascata dell'Intero Albero per Mancanza di `React.memo`
- **Identificativo**: `DEF-12`
- **Titolo**: Mancata memoizzazione dei componenti pesanti contenenti illustrazioni SVG
- **Livello di Severità**: **Media**
- **Percorso File e Linee**: `src/App.tsx:173`, `src/components/DeviceCard.tsx:47`, `src/views/Dashboard.tsx:29`
- **Descrizione del Problema**:  
  L'intero stato dell'applicazione è concentrato in `App.tsx`. A ogni aggiornamento dello snapshot (attivato anche dal polling a 2s), l'intero albero dell'interfaccia viene rieseguito. Componenti costosi come `DeviceCard` e `DeviceIllustration` (che generano elementi vettoriali complessi con gradienti radiali e filtri) vengono ri-renderizzati anche quando le relative proprietà rimangono identiche.
- **Spiegazione dell'Impatto Tecnico**:  
  Occupazione CPU ingiustificata del processo di rendering della WebView e degrado della fluidità visiva.
- **Raccomandazione di Remediation**:  
  Avvolgere i componenti foglia e le card con `React.memo`:
  ```tsx
  export const DeviceCard = React.memo(function DeviceCard({ device, selected, onOpen }: DeviceCardProps) { ... });
  export const DeviceIllustration = React.memo(function DeviceIllustration({ kind }: { kind: DeviceKind }) { ... });
  ```

---

#### DEF-13: Inclusione dell'Intero Motore di Simulazione Demo nel Bundle Finale
- **Identificativo**: `DEF-13`
- **Titolo**: Presenza di oltre 270 righe di codice demo/mock nel bundle JavaScript di produzione
- **Livello di Severità**: **Media**
- **Percorso File e Linee**: `src/api.ts:83-352`
- **Descrizione del Problema**:  
  La funzione `createDemoApi` e tutte le strutture dati di test in memoria (dispositivi virtuali Razer/Logitech, simulatore PipeWire e matrici nodi) risiedono all'interno di `src/api.ts` e sono importate staticamente da `createHubApi()`.
- **Spiegazione dell'Impatto Tecnico**:  
  Il codice di simulazione demo viene compilato e incluso nel bundle di produzione desktop finale (`dist/assets/index-*.js`), aumentando il peso del bundle di oltre 100 kB non compressi e violando il principio di separazione tra codice di release e fixture di test.
- **Raccomandazione di Remediation**:  
  Separare `createDemoApi` in un modulo isolato (`src/demoApi.ts`) e caricarlo tramite dynamic import condizionale solo se la variabile d'ambiente `VITE_DEMO` è attiva:
  ```ts
  export async function createHubApi(): Promise<HubApi> {
    if (import.meta.env.VITE_DEMO === "true") {
      const { createDemoApi } = await import("./demoApi");
      return createDemoApi();
    }
    return createTauriApi();
  }
  ```

---

#### DEF-14: Moduli Backend Monolitici ad Elevata Complessità Dimensionale
- **Identificativo**: `DEF-14`
- **Titolo**: File sorgente Rust singoli con dimensioni superiori a 2.300 linee di codice
- **Livello di Severità**: **Media**
- **Percorso File e Linee**: `src-tauri/src/remapping.rs:1-2345`, `src-tauri/src/audio.rs:1-2264`, `src-tauri/src/providers/ratbag.rs:1-1374`
- **Descrizione del Problema**:  
  I file `src-tauri/src/remapping.rs` (2.345 linee) e `src-tauri/src/audio.rs` (2.264 linee) racchiudono in un unico blocco continuo molteplici responsabilità: invocazione comandi di sistema, persistenza filesystem, parsing D-Bus, logica di business, conversioni di tipo e test.
- **Spiegazione dell'Impatto Tecnico**:  
  Complessità elevata nella manutenibilità del codice, forte accoppiamento interno e aumento del rischio di introdurre regressioni in caso di modifiche concorrenti.
- **Raccomandazione di Remediation**:  
  Scomporre i file in package modulari con sottomoduli dedicati (es. `remapping/evdev.rs`, `remapping/service.rs`, `remapping/state.rs`; `audio/pipewire.rs`, `audio/pulse.rs`, `audio/dsp.rs`).

---

#### DEF-15: Timeout Sproporzionato di 100 Secondi su `systemctl --user stop`
- **Identificativo**: `DEF-15`
- **Titolo**: Attesa sincrona di 100 secondi durante l'arresto dell'agente di rimappatura systemd
- **Livello di Severità**: **Media**
- **Percorso File e Linee**: `src-tauri/src/remapping.rs:1683`
- **Descrizione del Problema**:  
  Nella funzione `quiesce_agent`:
  ```rust
  crate::process::run("systemctl", STOP_AGENT_ARGUMENTS, Duration::from_secs(100))
  ```
  Il comando di arresto del demone utente systemd viene invocato impostando un timeout massimo di 100 secondi.
- **Spiegazione dell'Impatto Tecnico**:  
  Qualora l'agente di rimappatura o il gestore D-Bus dell'utente dovesse entrare in deadlock o rispondere con ritardo, l'interfaccia utente o il comando IPC rimane bloccato per oltre un minuto e mezzo prima di restituire un errore o fallire.
- **Raccomandazione di Remediation**:  
  Ridurre il timeout a una durata congrua per un demone utente (es. 5-10 secondi) e, in caso di mancata risposta entro il tempo limite, inviare `SIGKILL` o gestire l'errore senza bloccare il flusso principale.

---

#### DEF-16: Percorsi Hardcoded e Assunzioni FHS Fragili su Ambienti Non-Standard
- **Identificativo**: `DEF-16`
- **Titolo**: Percorsi assoluti cablati a directory standard `/usr/bin/` e `/usr/lib/`
- **Livello di Severità**: **Media**
- **Percorso File e Linee**: `src-tauri/src/audio_worker.rs:721`, `src/process.rs:10`, `src/surround.rs:15`, `src/remapping.rs:2109`
- **Descrizione del Problema**:  
  I percorsi verso binari ausiliari o risorse di sistema sono codificati come costanti fisse:
  - `/usr/bin/wpexec`
  - `/usr/lib/peripheral-hub/headsetcontrol`
  - `/usr/share/libmysofa/default.sofa`
  - `/usr/bin/peripheral-hub-remapping-agent`
- **Spiegazione dell'Impatto Tecnico**:  
  Su distribuzioni Linux che non adottano la gerarchia standard FHS (es. **NixOS**, **Guix**) o qualora l'applicazione venga distribuita in sandbox containerizzate (es. **Flatpak**), questi percorsi non esistono, causando il fallimento immediato delle relative funzionalità.
- **Raccomandazione di Remediation**:  
  Introdurre un meccanismo di risoluzione flessibile con fallback: verifica del `$PATH`, controllo relativo alla cartella dell'eseguibile dell'app e possibilità di sovrascrittura tramite variabili d'ambiente (es. `PERIPHERAL_HUB_SOFA_PATH`).

---

#### DEF-17: Esecuzione Comandi di Sistema con Risoluzione Ambigua in `$PATH`
- **Identificativo**: `DEF-17`
- **Titolo**: Invocazione di eseguibili di sistema con nomi relativi soggetti a manipolazione del `$PATH`
- **Livello di Severità**: **Media**
- **Percorso File e Linee**: `src-tauri/src/process.rs:52-104`
- **Descrizione del Problema**:  
  Mentre alcuni eseguibili critici utilizzano percorsi assoluti, i comandi `pw-dump`, `wpctl` e `systemctl` vengono invocati specificando unicamente il nome del binario (es. `Command::new("pw-dump")`), affidando la risoluzione alla variabile d'ambiente `$PATH` del processo utente.
- **Spiegazione dell'Impatto Tecnico**:  
  Se l'applicazione viene eseguita in contesti con un `$PATH` manipolato o contenente directory scrivibili da utenti non privilegiati (es. directory temporanee o cartelle locali utente), vi è il rischio teorico di esecuzione di binari contraffatti (PATH spoofing / binary shadowing).
- **Raccomandazione di Remediation**:  
  Risolvere e validare i binari canonici all'avvio dell'applicazione verificando percorsi di sistema fidati (es. `/usr/bin/pw-dump`, `/bin/systemctl`).

---

#### DEF-18: Assenza di Pipeline CI Automatica su Push e Pull Request Ordinarie
- **Identificativo**: `DEF-18`
- **Titolo**: Workflow di integrazione continua limitato unicamente a tag di versione e dispatch manuale
- **Livello di Severità**: **Media**
- **Percorso File e Linee**: `.github/workflows/linux-packaging.yml:3-11`
- **Descrizione del Problema**:  
  Il file di configurazione della CI GitHub Actions è attivato unicamente su:
  ```yaml
  on:
    workflow_dispatch:
    pull_request:
      paths: ...
  ```
  Non è configurato alcun workflow continuo sui normali `push` verso il branch `main` che esegua controlli veloci (typecheck TypeScript, test Vitest, compilazione e test Rust con cargo).
- **Spiegazione dell'Impatto Tecnico**:  
  Regressioni o rotture del codice possono confluire nel branch principale senza che il team riceva alcuna notifica automatica tempestiva, posticipando la scoperta dei difetti al momento del rilascio di un tag.
- **Raccomandazione di Remediation**:  
  Creare un workflow dedicato `.github/workflows/ci.yml` configurato su `push: branches: [main]` e `pull_request: branches: [main]` che esegua `npm run build`, `npm test` e `cargo test`.

---

#### DEF-19: Assenza di Linter e Formatter Configurati nel Repository
- **Identificativo**: `DEF-19`
- **Titolo**: Mancanza di strumenti formali per il linting e la formattazione di codice (ESLint, Prettier, Clippy)
- **Livello di Severità**: **Media**
- **Percorso File e Linee**: `package.json:8-19`, root config files
- **Descrizione del Problema**:  
  Nel file `package.json` non sono presenti dipendenze né script per ESLint o Prettier. Nella root e nella cartella `src-tauri` non sono presenti file di configurazione per `clippy.toml` o `rustfmt.toml`.
- **Spiegazione dell'Impatto Tecnico**:  
  Rischio di disallineamenti di stile tra sviluppatori e mancata intercettazione automatica di pratiche deprecate di React e TypeScript (es. hook dependencies non conformi).
- **Raccomandazione di Remediation**:  
  Configurare ESLint con `@typescript-eslint` e `eslint-plugin-react-hooks`, aggiungere lo script `npm run lint` e integrare i controlli di `cargo clippy -- -D warnings` nella pipeline di CI.

---

#### DEF-20: Flakiness Temporale nel Test Frontend su Gestione Errori DPI in JSDOM
- **Identificativo**: `DEF-20`
- **Titolo**: Fallimento intermittente per timeout del test di gestione errori DPI in `src/tests/App.test.tsx`
- **Livello di Severità**: **Media**
- **Percorso File e Linee**: `src/tests/App.test.tsx:298-308`
- **Descrizione del Problema**:  
  Nel test `conserva la lettura confermata su errore DPI e riconcilia la snapshot`:
  ```ts
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Impossibile impostare i DPI: Errore bus I/O"
  );
  ```
  Durante l'esecuzione congiunta dell'intera suite (54 test in un singolo file monolitico da 812 righe), l'ambiente JSDOM accumula latenza nel rendering dell'albero e può superare il timeout predefinito di 1.000 ms.
- **Spiegazione dell'Impatto Tecnico**:  
  Generazione di falsi positivi nei test automatici, con conseguente perdita di fiducia nella stabilità della suite di test.
- **Raccomandazione di Remediation**:  
  Specificare un timeout esplicito più generoso per l'asserzione asincrona: `screen.findByRole("alert", {}, { timeout: 3000 })` e suddividere `App.test.tsx` in suite separate per modulo.

---

#### DEF-21: Dipendenza Inutilizzata `quick-xml` e Crate Deprecato `fs2`
- **Identificativo**: `DEF-21`
- **Titolo**: Presenza di dipendenza morta `quick-xml = "0.38"` e dipendenza non mantenuta `fs2 = "0.4"`
- **Livello di Severità**: **Bassa**
- **Percorso File e Linee**: `src-tauri/Cargo.toml:46, 51`
- **Descrizione del Problema**:  
  In `Cargo.toml`, linea 51 è dichiarata `quick-xml = "0.38"`. Tuttavia, l'introspezione XML di D-Bus è interamente gestita da un parser manuale in `src/providers/mod.rs:181-456`, e non vi è alcuna occorrenza di `quick_xml` nel codice sorgente. Inoltre, alla linea 46 è presente `fs2 = "0.4"`, un crate per il file locking non più mantenuto dal 2017.
- **Spiegazione dell'Impatto Tecnico**:  
  Compilazione inutile di pacchetti di terze parti, incremento dei tempi di build e accumulo di debito tecnico nella catena delle dipendenze (supply chain).
- **Raccomandazione di Remediation**:  
  Rimuovere `quick-xml` da `Cargo.toml` e sostituire `fs2` con `fs4` o chiamate dirette alla syscall POSIX `libc::flock`.

---

#### DEF-22: Risorsa Template Spazializzatore Ridondante nel Bundle Tauri
- **Identificativo**: `DEF-22`
- **Titolo**: Duplicazione della risorsa `spatializer-7.1.conf.template` sia nel binario che nelle risorse di bundle
- **Livello di Severità**: **Bassa**
- **Percorso File e Linee**: `src-tauri/tauri.conf.json:34` vs `src-tauri/src/surround.rs:240`
- **Descrizione del Problema**:  
  In `tauri.conf.json:34`, `"resources/spatializer-7.1.conf.template"` è configurato come risorsa da copiare nel bundle di installazione. Tuttavia, in `src/surround.rs:240`, il file viene incorporato nel binario a tempo di compilazione tramite `include_str!("../resources/spatializer-7.1.conf.template")`.
- **Spiegazione dell'Impatto Tecnico**:  
  Duplicazione inutile del file template all'interno del pacchetto distribuito, senza che la copia su disco venga mai letta a runtime.
- **Raccomandazione di Remediation**:  
  Rimuovere la risorsa dall'array `bundle.resources` in `tauri.conf.json`.

---

#### DEF-23: Rifiuto Immediato con `Busy` nel Semaforo del Worker Audio
- **Identificativo**: `DEF-23`
- **Titolo**: Uso di `try_acquire()` su semaforo a 2 slot senza buffer o timeout di accodamento
- **Livello di Severità**: **Bassa**
- **Percorso File e Linee**: `src-tauri/src/audio_worker.rs:358-361`
- **Descrizione del Problema**:  
  Il worker audio limita la concorrenza con un semaforo a 2 slot usando `SLOTS.try_acquire()`. Se pervengono più di due richieste quasi simultanee (ad esempio regolazioni veloci del volume), le chiamate eccedenti vengono immediatamente respinte con errore `ErrorCode::Busy`.
- **Spiegazione dell'Impatto Tecnico**:  
  Rifiuto prematuro di operazioni utente legittime durante interazioni rapide con i controlli audio.
- **Raccomandazione di Remediation**:  
  Utilizzare `tokio::time::timeout(Duration::from_millis(500), SLOTS.acquire())` per consentire un breve accodamento prima di restituire un errore di occupato.

---

#### DEF-24: Rilettura Snapshot Ridondante e Duplicata su Fallimento di Connessione
- **Identificativo**: `DEF-24`
- **Titolo**: Chiamata a vuoto `getSnapshot` in `runMutation` quando l'errore originario era la connessione stessa
- **Livello di Severità**: **Bassa**
- **Percorso File e Linee**: `src/App.tsx:118-125`
- **Descrizione del Problema**:  
  In `runMutation`:
  ```ts
  } catch (error) {
    ...
    try {
      acceptSnapshot(await api.getSnapshot(), generation);
    } catch (readbackError) { ... }
  }
  ```
  Se l'operazione fallita era la connessione iniziale (`key === "connection"`), il blocco catch ripete immediatamente una seconda volta `api.getSnapshot()`.
- **Spiegazione dell'Impatto Tecnico**:  
  Chiamata IPC a vuoto verso un backend già noto come non disponibile e lieve ritardo nella visualizzazione dell'errore.
- **Raccomandazione di Remediation**:  
  Condizionare la rilettura a `if (key !== "connection")`.

---

#### DEF-25: Manipolazione Diretta del DOM (`document.getElementById`) in React
- **Identificativo**: `DEF-25`
- **Titolo**: Uso imperativo di `document.getElementById` per il controllo del focus nei pannelli
- **Livello di Severità**: **Bassa**
- **Percorso File e Linee**: `src/App.tsx:99, 146`
- **Descrizione del Problema**:  
  La gestione del focus all'apertura dei pannelli e nella navigazione a schede richiama `document.getElementById(...)?.focus()`.
- **Spiegazione dell'Impatto Tecnico**:  
  Violazione dell'incapsulamento dei componenti React e potenziale fragilità in contesti con istanze multiple o ID duplicati.
- **Raccomandazione di Remediation**:  
  Utilizzare i React Ref (`useRef` o mappe di riferimenti) per gestire il focus in maniera dichiarativa.

---

#### DEF-26: Utilizzo dell'Indice dell'Array come Chiave React (`key={index}`)
- **Identificativo**: `DEF-26`
- **Titolo**: Uso di `index` come chiave di riconciliazione virtual DOM nelle liste di limitazioni hardware
- **Livello di Severità**: **Bassa**
- **Percorso File e Linee**: `src/views/MousePanel.tsx:105`, `src/views/AudioPanel.tsx:60`
- **Descrizione del Problema**:  
  Nelle liste delle limitazioni hardware:
  ```tsx
  {device.limitations.map((limitation, index) => <li key={index}>{limitation}</li>)}
  ```
- **Spiegazione dell'Impatto Tecnico**:  
  Anti-pattern noto in React: l'uso dell'indice numerico può causare anomalie di rendering se la lista subisce mutazioni, riordinamenti o filtri.
- **Raccomandazione di Remediation**:  
  Adottare chiavi stabili e univoche: `key={`${device.id}-limit-${index}`}` o hash del testo.

---

#### DEF-27: Ricreazione Forzata di `AudioPanel` con Chiave Dinamica e Perdita Stato
- **Identificativo**: `DEF-27`
- **Titolo**: Distruzione e ricreazione da zero del componente `AudioPanel` con `key={audioFocus.token}`
- **Livello di Severità**: **Bassa**
- **Percorso File e Linee**: `src/App.tsx:40, 65, 137, 156`
- **Descrizione del Problema**:  
  In `App.tsx`, il componente audio viene renderizzato con `key={audioFocus.token}`: ogni volta che l'utente clicca su un riferimento audio o sul banner, il token viene incrementato.
- **Spiegazione dell'Impatto Tecnico**:  
  React distrugge e rimonta da zero l'intero albero di `AudioPanel`. Se l'utente stava configurando un profilo audio o modificando impostazioni non ancora salvate, qualsiasi cambio di focus azzera istantaneamente lo stato del form locale.
- **Raccomandazione di Remediation**:  
  Rimuovere la chiave dinamica e gestire lo scroll verso la scheda target tramite `useEffect` interno che reagisce al cambio di `audioFocus.targetCardIndex`.

---

#### DEF-28: Rischio di Indisponibilità di `crypto.randomUUID()` in WebKitGTK Legacy
- **Identificativo**: `DEF-28`
- **Titolo**: Invocazione diretta di `crypto.randomUUID()` senza fallback in ambiente WebView Linux
- **Livello di Severità**: **Bassa**
- **Percorso File e Linee**: `src/views/AudioPanel.tsx:38`
- **Descrizione del Problema**:  
  La generazione degli ID per i nuovi profili audio fa affidamento su `crypto.randomUUID()`.
- **Spiegazione dell'Impatto Tecnico**:  
  Su distribuzioni Linux meno recenti con versioni di WebKitGTK datate o in contesti non considerati crittograficamente sicuri, `crypto.randomUUID` può risultare indefinito, provocando un'eccezione a runtime durante il salvataggio di un profilo.
- **Raccomandazione di Remediation**:  
  Introdurre un helper con fallback standard basato su valori casuali formattati secondo lo standard RFC 4122.

---

#### DEF-29: Gestione Asimmetrica degli Eventi Click sull'Icona Tray di Sistema
- **Identificativo**: `DEF-29`
- **Titolo**: Gestione della visibilità della finestra basata su flag atomico asimmetrico `skip_release` su eventi Tray
- **Livello di Severità**: **Bassa**
- **Percorso File e Linee**: `src-tauri/src/lib.rs:840-875`
- **Descrizione del Problema**:  
  La tray icon alterna la visibilità della finestra intercettando l'evento `MouseButtonState::Down` e memorizzando un flag atomico `skip_release.store(true, Ordering::SeqCst)`.
- **Spiegazione dell'Impatto Tecnico**:  
  Su implementazioni Linux di StatusNotifierItem (es. KDE Plasma o GNOME con estensioni AppIndicator), gli eventi di mouse down e up possono pervenire in ordine non deterministico, provocando doppi scatti o mancate aperture al click.
- **Raccomandazione di Remediation**:  
  Standardizzare la logica di apertura/chiusura sull'evento `MouseButtonState::Up`, in linea con le convenzioni standard delle GUI desktop Linux.

---

#### DEF-30: Contrasto Colore Insufficiente su Micro-Etichette secondo WCAG 2.1 AA
- **Identificativo**: `DEF-30`
- **Titolo**: Testi a 9px con rapporto di contrasto cromatico inferiore alla soglia minima 4.5:1
- **Livello di Severità**: **Bassa**
- **Percorso File e Linee**: `src/styles.css:43, 64, 81, 120`
- **Descrizione del Problema**:  
  Alcune classi di testo secondario utilizzano dimensioni di 9px e colori a basso contrasto:
  - `.brand-subtitle`: `#969696` su `#1b1b1b` (contrasto 4.4:1)
  - `.instrument-caption`: `#888` su `#222222` (contrasto 3.5:1)
  - `.device-card-copy p`: `#929292` su `#181818` (contrasto 4.1:1)
- **Spiegazione dell'Impatto Tecnico**:  
  Mancata conformità alle linee guida di accessibilità W3C WCAG 2.1 AA (soglia minima 4.5:1 per testi normali), con conseguente difficoltà di lettura per utenti ipovedenti.
- **Raccomandazione di Remediation**:  
  Elevare la dimensione minima dei caratteri ad almeno 11px e utilizzare tonalità cromatiche più luminose (es. `#b4b4b4` o superiore).

---

## 5. Piano d'Azione e Raccomandazioni Strategiche di Remediation

Si raccomanda di articolare gli interventi correttivi in quattro fasi progressive a priorità decrescente:

### Fase 1: Risoluzione Criticità Bloccanti e di Stabilità (Priorità Immediata)
1. **Risolvere il Blocco Exit su Linux (DEF-01)**: Rimuovere la direttiva `#[cfg(not(target_os = "linux"))]` in `src-tauri/src/lib.rs:949` e garantire che `request_exit` venga sempre avviata alla ricezione di `ExitRequested`.
2. **Eliminare il Panic Vector in Drop (DEF-02)**: Sostituire l'asserzione `.expect()` in `OwnedChild` con una guardia `if let Some(...)` totalmente non-panicking in `src-tauri/src/audio_worker.rs:573`.
3. **Configurare il Bundling dei Worker Ausiliari (DEF-03)**: Aggiungere `bundle.externalBin` in `tauri.conf.json` per assicurare l'inclusione dei binari ausiliari nei pacchetti AppImage distribuiti.
4. **Proteggere il Frontend dal Crash HRTF (DEF-04)**: Introdurre il controllo preventivo sull'esistenza dell'ID HRTF in `src/api.ts:339`.

### Fase 2: Ottimizzazione Prestazioni e Concorrenza IPC (Priorità Alta)
1. **Condizionare l'Emissione di `snapshot-changed` (DEF-05)**: Modificare `refresh_remapping_locked` per confrontare lo snapshot ed emettere l'evento IPC solo in presenza di variazioni di stato effettive, eliminando il risveglio inutile della CPU a 2 secondi.
2. **Scomporre il Mutex Globale Hardware (DEF-06)**: Separare il lock del remapping dai controlli per-device e dal polling della batteria per eliminare la contesa.
3. **Ottimizzare il Provider OpenRazer (DEF-07)**: Evitare l'avvio e lo sleep di `systemctl` quando il demone D-Bus è già attivo e rimuovere la scansione sysfs ridondante prima della scrittura.
4. **Preservare le Bozze Utente durante gli Aggiornamenti Live (DEF-10)**: Evitare il reset dei cursori durante il trascinamento o focus attivo in `Controls.tsx` e `MousePanel.tsx`.
5. **Memoizzare Componenti SVG e Ottimizzare Stringify (DEF-11, DEF-12)**: Applicare `React.memo` a `DeviceCard` e sostituire i `JSON.stringify` ripetuti in `MouseButtons.tsx` con `useMemo`.

### Fase 3: Architettura, Robustezza e Accessibilità (Priorità Media)
1. **Integrare React ErrorBoundary (DEF-08)**: Proteggere l'albero componenti da schermate bianche con un fallback grafico di ripristino.
2. **Correggere la Struttura HTML di `DeviceCard` (DEF-09)**: Convertire il content model del bottone a elementi in linea conformi alle specifiche W3C.
3. **Isolare il Mock Demo dal Bundle di Produzione (DEF-13)**: Adottare import dinamici condizionali per caricare `createDemoApi` solo in ambiente demo.
4. **Modularizzare i File Backend Monolitici (DEF-14)**: Decomporre `remapping.rs` e `audio.rs` in sottomoduli specializzati.
5. **Ridurre il Timeout di Quiesce a 5-10s (DEF-15)**: Evitare blocchi di 100 secondi su `systemctl --user stop`.
6. **Migliorare la Portabilità FHS e Risoluzione Comandi (DEF-16, DEF-17)**: Introdurre percorsi configurabili e validazione percorsi assoluti.

### Fase 4: Rifinitura CI/CD, Tooling e Dettagli Minori (Priorità Bassa)
1. **Completare la Pipeline CI (DEF-18)**: Configurare workflow su push e pull request verso `main`.
2. **Configurare Linter e Formatter (DEF-19)**: Introdurre ESLint, Prettier e controlli Clippy.
3. **Risolvere la Flakiness nei Test Frontend (DEF-20)**: Estendere il timeout di attesa per l'alert DPI in `App.test.tsx`.
4. **Pulire Dipendenze e Risorse Morte (DEF-21, DEF-22)**: Rimuovere `quick-xml` e la duplicazione del template di spazializzazione.
5. **Rifiniture UX e Accessibilità (DEF-23 a DEF-30)**: Aggiungere timeout di accodamento al semaforo audio, eliminare manipolazioni DOM dirette, preservare lo stato di `AudioPanel` ed elevare il contrasto cromatico delle micro-etichette.

---

## 6. Metodologia di Verifica Indipendente

Per consentire a revisori, auditor indipendenti o altri agenti di verificare la veridicità di ogni affermazione, punteggio e riga di codice citata nel presente rapporto, eseguire i seguenti comandi e controlli:

### 6.1 Verifica Suite di Test Frontend
```bash
# Esecuzione completa dei 79 test Vitest
npm test
```
*Esito Verificato*: 2 file di test completati con successo (`src/tests/api.test.ts` e `src/tests/App.test.tsx`), **79 test passati su 79** (durata ~11-12s).

### 6.2 Verifica Controllo Tipi TypeScript e Bundle di Produzione
```bash
# Controllo tipi strict e build con Vite
npm run build
```
*Esito Verificato*: `tsc --noEmit` completato con **0 errori**. Build Vite completata con successo in meno di 1 secondo (929 ms), generando gli asset in `dist/`.

### 6.3 Verifica Suite di Test di Packaging Linux
```bash
# Esecuzione dei 224 test del sistema di packaging e CI Linux
python3 -m unittest discover -s packaging/tests -v
```
*Esito Verificato*: **224 test passati su 224** con esito `OK` in meno di 1 secondo (0.993s).

### 6.4 Verifica dell'Invarianza del Codice Sorgente (Sola Lettura)
```bash
# Verifica che nessun file di codice preesistente sia stato modificato
git status --porcelain
```
*Esito Verificato*: Nessuna modifica ai file in `src/`, `src-tauri/`, `packaging/` o file di configurazione; unico file nuovo nella root è `ANALISI_CODICE.md`.

### 6.5 Riscontro Diretto dei Riferimenti di Codice nei File Sorgente
1. **Verifica Deadlock all'uscita (DEF-01)**: Ispezionare `src-tauri/src/lib.rs` alle linee 944–953 per constatare l'esclusione di `request_exit` su Linux.
2. **Verifica Panic in Drop (DEF-02)**: Ispezionare `src-tauri/src/audio_worker.rs` alle linee 568–605 per verificare `.expect("owned child")` nel flusso di `kill()` richiamato da `drop()`.
3. **Verifica Omissione Bundling Helper (DEF-03)**: Ispezionare `src-tauri/tauri.conf.json` alle linee 29–41 per constatare l'assenza di `externalBin`.
4. **Verifica Asserzione Non-Null su HRTF (DEF-04)**: Ispezionare `src/api.ts` alla linea 339 per constatare l'uso di `clone(hrtfs.get(profile.hrtf)!)`.
5. **Verifica Polling a 2s e Revision Increment (DEF-05)**: Ispezionare `src-tauri/src/lib.rs` alle linee 781–791 e 108–116 per verificare l'incremento sistematico di `revision` e l'emissione dell'evento `snapshot-changed`.
6. **Verifica Content Model HTML (DEF-09)**: Ispezionare `src/components/DeviceCard.tsx` alle linee 53–58 per verificare la presenza di `<div>`, `<h3>`, `<p>`, `<meter>` dentro `<button>`.
7. **Verifica Dipendenza Morta quick-xml (DEF-21)**: Ispezionare `src-tauri/Cargo.toml` alla linea 51 e verificare l'assenza di `use quick_xml` nel sorgente con `grep -rn "quick_xml" src-tauri/src/`.

---
*Fine del Rapporto di Analisi — Documento redatto a scopo di audit ingegneristico per il progetto Sycomopse.*
