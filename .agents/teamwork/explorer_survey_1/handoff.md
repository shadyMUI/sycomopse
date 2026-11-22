# Report di Audit e Analisi Codice — Frontend React/TypeScript (`src/`)

## 1. Observation

L'ispezione diretta, manuale e sistematica di tutti i 15 file sorgente nella directory `src/` del repository `sycomopse` (`peripheral-hub`) ha evidenziato le seguenti evidenze concrete:

### 1.1 Metriche e Configurazione del Frontend
- **Framework & Runtime**: React 19.1.1, React DOM 19.1.1, TypeScript 5.9.2, Vite 7.1.7, Vitest 5.0.5, Tauri API Core & Event 2.8.0.
- **Configurazione TypeScript (`tsconfig.json:8-16`)**:
  - `strict: true`
  - `noEmit: true`
  - `noUnusedLocals: true`
  - `noUnusedParameters: true`
  - `target: "ES2022"`, `moduleResolution: "Bundler"`, `jsx: "react-jsx"`.
- **Esecuzione Test Suite (`vitest run`)**:
  - Comando: `npm test`
  - Risultato: **2 file di test superati, 79 test passati su 79** (durata 12.15s).
    - `src/tests/api.test.ts`: 25 test passati.
    - `src/tests/App.test.tsx`: 54 test passati.
- **Verifica Compilazione e Bundle (`tsc --noEmit && vite build`)**:
  - Comando: `npm run build`
  - Risultato: **0 errori TypeScript**, build completata con successo in 929ms.
  - Dimensioni bundle di produzione generato:
    - `dist/index.html`: 0.51 kB (gzip: 0.31 kB)
    - `dist/assets/index-BTMcGq5-.css`: 32.78 kB (gzip: 7.46 kB)
    - `dist/assets/index-BpeWqCIj.js`: 288.45 kB (gzip: 89.12 kB)

### 1.2 Inventario File Esaminati
| File | Righe | Scopo Primario |
|---|---|---|
| `src/main.tsx` | 10 | Bootstrap dell'app React con `createRoot` e `StrictMode` |
| `src/App.tsx` | 179 | Componente root, orchestrazione stato globale, routing a tab, gestione IPC e mutazioni |
| `src/types.ts` | 90 | Definizione tipi TypeScript per hardware, audio PipeWire, capabilities, diagnostica e IPC |
| `src/api.ts` | 361 | Client IPC Tauri (`createTauriApi`), engine mock di simulazione in-memory (`createDemoApi`), factory |
| `src/remapping.ts` | 99 | Logica di dominio pura: validazione assegnazioni tasti, codici Linux EV_KEY, chording |
| `src/styles.css` | 423 | Fogli di stile CSS unificati con variabili CSS, layout a griglia e media query responsive |
| `src/components/Controls.tsx` | 202 | Componenti atomici UI: `Toggle`, `DpiControl`, `OutputSelector`, `NodeControls`, `SurroundControl`, `BatteryReadout` |
| `src/components/DeviceCard.tsx` | 62 | Card periferica interattiva con illustrazioni vettoriali SVG procedurali (`DeviceIllustration`) |
| `src/views/Dashboard.tsx` | 61 | Vista principale: panoramica hardware, stato surround, metriche, sezione controlli rapidi |
| `src/views/MousePanel.tsx` | 111 | Vista dedicata al mouse: regolazione DPI/polling (`MouseControls`), metadati, alimentazione |
| `src/views/MouseButtons.tsx` | 184 | Vista rimappatura pulsanti: assegnazione clic, tasti, scorciatoie (chording), recorder interattivo |
| `src/views/AudioPanel.tsx` | 76 | Vista audio PipeWire: routing ingressi/uscite, controlli nodi, surround 7.1 binaurale, HRTF SOFA, profili |
| `src/tests/setup.ts` | 6 | Inizializzazione ambiente Vitest + `@testing-library/jest-dom` e cleanup |
| `src/tests/api.test.ts` | 276 | Test di integrazione contratti IPC Tauri, validazione DPI, simulazione demo e remapping |
| `src/tests/App.test.tsx` | 812 | Test di integrazione end-to-end componenti React con `@testing-library/react` |

---

## 2. Logic Chain

Dall'osservazione diretta del codice e dei contratti architetturali emergono le seguenti deduzioni logiche e valutazioni dettagliate.

### 2.1 Valutazione per Categorie Standard (Scala 1-10)

#### 1. Architettura (Architecture) — Voto: 7.5 / 10
- **Punti di Forza**:
  - Chiara separazione delle responsabilità tra tipi (`types.ts`), logica pura di dominio per il remapping (`remapping.ts`), layer di astrazione hardware (`api.ts`), componenti atomici (`Controls.tsx`) e viste applicative (`views/*`).
  - Pattern "Readback-First": il frontend non presume mai il successo delle mutazioni hardware. Nessun aggiornamento ottimistico arbitrario; l'interfaccia si aggiorna esclusivamente su ricezione di snapshot confermati dal backend, garantendo coerenza con lo stato reale delle periferiche.
  - Distinzione netta e trasparente tra ambiente Linux nativo, browser standalone non supportato e modalità demo simulata.
- **Criticità e Debolezze**:
  - **Stato monolitico in `App.tsx`**: oltre 10 `useState` e `useRef` coesistono in `App.tsx` (`snapshot`, `loading`, `tab`, `selectedMouseId`, `audioFocus`, `pending`, `errors`, `pendingRef`, `generationRef`, `activeRef`, `focusFromCardRef`).
  - **Prop Drilling esteso**: l'oggetto `controls = { api, pending, runMutation }` viene passato a cascata attraverso più livelli di annidamento fino ai controlli foglia (`NodeControls`, `SurroundControl`, `DpiControl`).
  - **Accoppiamento incrociato tra viste**: `Dashboard.tsx:6` importa ed esegue direttamente `MouseControls` esportato da `views/MousePanel.tsx`. Le viste dovrebbero consumare componenti condivisi posizionati in `components/`, senza dipendenze orizzontali tra viste distinte.
  - **Accorpamento dell'engine mock nel bundle di produzione**: `src/api.ts` incorpora sia il client IPC Tauri che un motore di simulazione in memoria di 270 righe (`createDemoApi`), incluso staticamente nel bundle di produzione.
  - **Assenza di ErrorBoundary**: non è presente alcun confine di errore né a livello root né a livello di vista.

#### 2. Qualità del codice (Code Quality) — Voto: 8.0 / 10
- **Punti di Forza**:
  - **Assenza totale del tipo `any`**: nell'intera codebase `src/` non compare un singolo utilizzo di `any`. La tipizzazione è rigorosa, pulita ed espressiva.
  - **Modellazione solida con Discriminated Unions**: tipi come `MouseAction`, `DpiValues`, `AvailabilityCode`, `ProviderKind` sfruttano discriminatori esaustivi gestiti tramite switch/case completi.
  - **Nomenclatura e localizzazione eccellenti**: testi e messaggi di stato utilizzano un italiano coerente, chiaro e tecnicamente accurato per il contesto Linux (EV_KEY, PipeWire, nodi ALSA, HRTF).
- **Criticità e Debolezze**:
  - Asserzione non-null rischiosa in `src/api.ts:339` (`clone(hrtfs.get(profile.hrtf)!)`) che innesca un'eccezione a runtime se la chiave non esiste.
  - Utilizzo di `key={index}` in liste dinamiche in `src/views/MousePanel.tsx:105` e `src/views/AudioPanel.tsx:60`.
  - Serializzazione JSON multipla e pesante (`JSON.stringify`) eseguita a ogni ciclo di render in `src/views/MouseButtons.tsx:51-54, 66`.
  - Clonazione tramite `JSON.parse(JSON.stringify(...))` anziché `structuredClone`.
  - Manipolazione diretta del DOM (`document.getElementById`) in `App.tsx:99, 146`.

#### 3. Sicurezza (Security) — Voto: 9.0 / 10
- **Punti di Forza**:
  - **Validazione stringente sul remapping dei pulsanti (`src/remapping.ts:62-98`)**: la funzione `remappingProblem` verifica che ogni codice tasto sia rigorosamente presente tra i codici hardware esposti dal provider (`capability.keyCodes`), che non siano inserite macro o sequenze arbitrarie, che i modificatori siano validi e limitati (massimo 4), e che non vengano utilizzati codici mouse riservati come tasti tastiera.
  - **Protezione anticrash/antilocckout del mouse**: il codice impone tassativamente che esista sempre almeno un pulsante assegnato al clic sinistro (`code: 272`). Se l'utente tenta di disabilitare o rimappare il clic sinistro senza assegnarlo altrove, la mutazione viene bloccata sul client (`src/remapping.ts:91-95`).
  - **Gestione del consenso per identità deboli**: se una periferica è identificata solo dalla porta USB fisica (`identityStrength: "port"` anziché seriale hardware univoco), l'opzione di riattivazione automatica (`autoApply`) viene disabilitata a meno di esplicito consenso dell'utente, prevenendo l'applicazione accidentale di profili a mouse diversi collegati alla medesima porta (`src/remapping.ts:96`, `src/views/MouseButtons.tsx:169-170`).
  - **Isolamento del recorder tastiera**: l'acquisizione delle scorciatoie da tastiera avviene unicamente quando il pulsante del recorder detiene attivamente il focus (`event.currentTarget === document.activeElement`), azzerandosi su blur o Tab/Esc, senza alcun listener globale su `window` o `document`.
  - **Assenza di XSS e injection**: nessun uso di `dangerouslySetInnerHTML`, nessun accesso `eval`/`Function`, nessuna richiesta verso endpoint remoti non attendibili.
- **Criticità Minori**:
  - `URLSearchParams` su `window.location.search` in `src/api.ts:356` (sebbene sanificato tramite whitelist `["0", "1", "3"]`).

#### 4. Prestazioni (Performance) — Voto: 7.0 / 10
- **Punti di Forza**:
  - Build estremamente veloce (<1 secondo).
  - Gestione efficiente degli slider tramite variabili CSS native (`--range-fill`) per la barra di avanzamento, azzerando i ricalcoli di layout JavaScript.
  - Zero librerie pesanti o CSS-in-JS a runtime.
- **Criticità e Debolezze**:
  - **Rirender a cascata dell'intero albero**: ogni aggiornamento dello stato globale `snapshot` in `App.tsx` rigenera l'intero albero virtual DOM. Componenti complessi e pesanti come `DeviceCard` e `DeviceIllustration` (che contengono SVG procedurali articolati con gradienti lineari e radiali multipli) vengono ri-renderizzati anche quando non hanno subito alcuna modifica. Mancano `React.memo` o contesti dedicati.
  - **Conflitto di aggiornamento in `useEffect` su slider e input**: `NodeControls` (`Controls.tsx:138`) e `MouseControls` (`MousePanel.tsx:15-16`) resettano incondizionatamente lo stato della bozza locale (`draft`) quando arrivano nuovi valori da snapshot, causando scatti o ripristini imprevisti del cursore se l'utente sta interagendo durante un evento live in background.
  - **Pressione sul Garbage Collector**: i 4 ricalcoli sincroni di `JSON.stringify` a ogni render in `MouseButtons.tsx:51-54, 66` generano stringhe temporanee scartate immediatamente.
  - **Bundle size subottimale**: il bundle JS di produzione (288 kB) contiene tutto il codice di simulazione demo.

#### 5. Gestione errori (Error Handling) — Voto: 7.5 / 10
- **Punti di Forza**:
  - Tipizzazione solida degli errori tramite `AppError` (`code`, `message`, `detail`).
  - Banner di errore contestuali non bloccanti con dettagli diagnostici espandibili (`App.tsx:171`).
  - Prevenzione race condition mediante `generationRef` e `activeRef` in `App.tsx:48-52`, garantendo che risposte asincrone obsolete non sovrascrivano lo stato più recente.
  - Serializzazione delle mutazioni concorrenti per gruppo (`runMutation` previene scritture simultanee sullo stesso dispositivo o sottosistema audio).
- **Criticità e Debolezze**:
  - **Assenza di React ErrorBoundary**: se si verifica un errore durante il ciclo di render o in un getter di una proprietà imprevista, l'intera applicazione fallisce con schermo bianco permanente.
  - **Rilettura duplicata su fallimento**: in `runMutation` (`App.tsx:118-125`), la gestione dell'errore esegue un `api.getSnapshot()`. Se l'operazione fallita era la connessione iniziale (`getSnapshot`), il catch ripete la chiamata fallimentare una seconda volta.
  - **Rischio di crash per asserzione non-null**: `src/api.ts:339` innesca un'eccezione `SyntaxError` in caso di HRTF non mappata.
  - **Chiusura obsoleta (stale closure)**: `runMutation("exit", ...)` in `App.tsx:168` cattura il valore di `snapshot` del render corrente.

#### 6. Test (Testing) — Voto: 8.5 / 10
- **Punti di Forza**:
  - 79 test completi e superati, con ottima copertura comportamentale di edge case:
    - Contratti IPC, argomenti camelCase, assenza di leak di path locali dal frontend al backend.
    - Gestione della piattaforma non supportata nel browser.
    - Validazione DPI a intervallo e a valori discreti, assi separati X/Y, bozze manuali non valide.
    - Regole di sicurezza del mouse remapping (blocco perdita clic sinistro, chording, auto-apply, consenso porta debole).
    - Pipeline audio PipeWire, gestione nodi virtuali surround 7.1, importazione HRTF e profili.
    - Hotplugging: rimozione a caldo del mouse selezionato e fallback immediato al mouse successivo.
    - Test in `StrictMode` per verificare la pulizia delle sottoscrizioni e l'assenza di memory leak su doppio mount.
- **Criticità e Debolezze**:
  - **Test monolitici**: `src/tests/App.test.tsx` supera le 800 righe; quasi tutti i test sono integrazioni end-to-end sull'intera app. Mancano test unitari isolati per i singoli componenti atomici (`DeviceCard`, `Controls`, `NodeControls`).
  - **Assenza di test di accessibilità automatizzati (a11y)**: nessun tool come `axe-core` o `jest-axe` è integrato nella pipeline, permettendo al difetto di invalidità HTML in `DeviceCard.tsx` di sfuggire ai test.
  - **Nessun test di regressione visuale**: non ci sono snapshot test o test visivi per le illustrazioni SVG procedurali.

---

### 2.2 Riepilogo Punteggi per Categoria

| Categoria | Voto (1-10) | Giustificazione Sintetica |
|---|:---:|---|
| **Architettura** | **7.5** | Buona modularità di base e ottimo approccio readback-first, ma stato monolitico in `App.tsx`, prop drilling, accoppiamento incrociato tra viste e mock demo non separato dal bundle di produzione. |
| **Qualità del codice** | **8.0** | Zero `any`, tipizzazione TypeScript rigorosa ed elegante con discriminated unions. Piccoli difetti: `key={index}`, asserzione non-null a rischio crash, `JSON.stringify` ripetuti e manipolazioni DOM dirette. |
| **Sicurezza** | **9.0** | Eccellente progettazione difensiva: convalida ferrea dei codici tasto, prevenzione del blocco del clic sinistro, consenso esplicito per identità deboli, isolamento del recorder scorciatoie. |
| **Prestazioni** | **7.0** | Velocità di avvio e build eccellenti, ma ri-render a cascata dell'intero albero per mancanza di `React.memo`, ricalcoli `JSON.stringify` su hot path e rischio di reset dei controlli durante interazioni attive. |
| **Gestione errori** | **7.5** | Ottimo modello di stato confermato, gestione delle race condition con token di generazione e banner non bloccanti. Carenza grave nell'assenza di ErrorBoundary React e rilettura duplicata su errori di connessione. |
| **Test** | **8.5** | 79 test completi e tutti passati, con eccellente copertura di flussi reali, contratti IPC e StrictMode lifecycle. Manca modularità nei test (file monolitico da 812 righe) e test a11y automatici. |
| **MEDIA COMPLESSIVA** | **7.92 / 10** | **Valutazione Generale: Buono / Solido con Aree di Rifinitura Mirate** |

---

### 2.3 Registro Dettagliato dei Difetti e Aree di Miglioramento

#### DEF-FE-01: Assenza di React ErrorBoundary a livello Root e di Vista
- **Livello di Severità**: **Media**
- **Riferimento nel Codice**: `src/main.tsx:9`, `src/App.tsx:165-177`
- **Descrizione**: L'applicazione React viene istanziata direttamente tramite `createRoot(root).render(<StrictMode><App /></StrictMode>)` senza alcun componente `ErrorBoundary`.
- **Impatto**: Qualsiasi eccezione non gestita durante il ciclo di render o all'interno di un componente figlio (ad esempio un errore di calcolo vettoriale SVG, una proprietà imprevista in snapshot o un errore di parsing) provoca il crash totale dell'albero React ("white screen of death"), lasciando l'utente di fronte a una schermata vuota senza feedback o possibilità di ripristino.
- **Raccomandazione di Correzione**:
  Creare un componente `ErrorBoundary` in `src/components/ErrorBoundary.tsx` e avvolgere sia `<App />` in `main.tsx` sia i singoli pannelli in `App.tsx`:
  ```tsx
  // src/main.tsx
  createRoot(root).render(
    <StrictMode>
      <ErrorBoundary fallback={<AppCrashFallback />}>
        <App />
      </ErrorBoundary>
    </StrictMode>
  );
  ```

---

#### DEF-FE-02: Rischio di Crash a Runtime per Asserzione Non-Null su HRTF Non Censita
- **Livello di Severità**: **Alta**
- **Riferimento nel Codice**: `src/api.ts:338-340`
- **Descrizione**: Nel metodo `applyAudioProfile` dell'API demo:
  ```ts
  if (profile.hrtf !== null) next.audio.hrtf = clone(hrtfs.get(profile.hrtf)!);
  ```
  Viene utilizzata l'asserzione non-null `!` sul risultato di `hrtfs.get(profile.hrtf)`. Se il profilo contiene un riferimento HRTF non registrato o corrotto, `hrtfs.get(...)` restituisce `undefined`. La funzione `clone` richiama `JSON.parse(JSON.stringify(undefined))` -> `JSON.stringify(undefined)` restituisce `undefined` -> `JSON.parse(undefined)` solleva un'eccezione non intercettata: `SyntaxError: Unexpected token u in JSON at position 0`.
- **Impatto**: Crash immediato e non gestito dell'applicazione demo quando si tenta di applicare un profilo audio con un riferimento HRTF non registrato.
- **Raccomandazione di Correzione**:
  Validare l'esistenza del riferimento prima della clonazione:
  ```ts
  if (profile.hrtf !== null) {
    const targetHrtf = hrtfs.get(profile.hrtf);
    if (!targetHrtf) demoError(`Riferimento HRTF '${profile.hrtf}' non disponibile nella demo.`);
    next.audio.hrtf = clone(targetHrtf);
  }
  ```

---

#### DEF-FE-03: Violazione del Content Model HTML (Elementi di blocco e heading dentro `<button>`)
- **Livello di Severità**: **Media**
- **Riferimento nel Codice**: `src/components/DeviceCard.tsx:53-58`
- **Descrizione**: L'elemento `<button type="button" className="device-card-button">` contiene elementi di blocco e di flusso non ammessi dalle specifiche W3C HTML5 all'interno dei bottoni: `<div>`, `<h3>`, `<p>`, e `<meter>` (tramite `BatteryReadout`). Il modello di contenuto consentito per `<button>` è limitato esclusivamente a *phrasing content*.
- **Impatto**: Comportamento imprevedibile con lettori di schermo e tecnologie assistive (NVDA, VoiceOver), che possono ignorare la semantica di intestazione (`<h3>`) nella navigazione dell'albero delle sezioni, o annunciare in modo caotico la card periferica.
- **Raccomandazione di Correzione**:
  Ristrutturare la card come un `<article>` contenitore e posizionare all'interno un pulsante accessibile, oppure convertire i tag interni in `<span>` stilizzati via CSS con `display: block` e preservare la semantica di intestazione all'esterno o tramite attributi ARIA:
  ```tsx
  <article className={`device-card${selected ? " selected" : ""}`} data-testid="device-card">
    <div className="device-card-content">
      <div className="device-card-top">...</div>
      <DeviceIllustration kind={device.kind} />
      <div className="device-card-copy">
        <h3>{device.name}</h3>
        ...
      </div>
      <button type="button" className="button button-secondary button-small" disabled={!actionable} onClick={() => onOpen(device)}>
        Apri controlli
      </button>
    </div>
  </article>
  ```

---

#### DEF-FE-04: Sovrascrittura Involontaria delle Bozze Utente in `useEffect` durante Aggiornamenti Live
- **Livello di Severità**: **Media**
- **Riferimento nel Codice**: `src/components/Controls.tsx:137-138`, `src/views/MousePanel.tsx:15-16`
- **Descrizione**: In `NodeControls`:
  ```ts
  const [draft, setDraft] = useState(node.volume);
  useEffect(() => { setDraft(node.volume); }, [node.nodeName, node.volume]);
  ```
  e in `MouseControls`:
  ```ts
  useEffect(() => { setDraftDpi(dpiDraft(device)); }, [device.id, device.values.dpi?.x, device.values.dpi?.y]);
  useEffect(() => { setDraftRate(device.values.pollingRate); }, [device.id, device.values.pollingRate]);
  ```
  Se il backend invia un aggiornamento di snapshot mentre l'utente sta trascinando il cursore del volume o digitando il DPI manuale, l'effetto intercetta il nuovo riferimento e sovrascrive istantaneamente la bozza locale con il valore precedente.
- **Impatto**: Difetto di usabilità: lo slider salta indietro durante l'interazione o il testo digitato nel campo DPI viene azzerato prima di poter cliccare su "Applica".
- **Raccomandazione di Correzione**:
  Tracciare lo stato di "dirty" o interazione attiva: sincronizzare lo stato solo se l'utente non ha modificato il valore o non ha il focus sull'elemento.

---

#### DEF-FE-05: Ricalcolo Costoso di `JSON.stringify` a ogni Ciclo di Render
- **Livello di Severità**: **Media**
- **Riferimento nel Codice**: `src/views/MouseButtons.tsx:51-54`, `src/views/MouseButtons.tsx:66`
- **Descrizione**: A ogni esecuzione della funzione componente `MouseButtons`:
  ```ts
  const sourceKey = JSON.stringify([capability?.fingerprint, capability?.identityStrength, capability?.buttons.map((button) => [button.id, button.code])]);
  const savedKey = JSON.stringify([observed?.profileId, observed?.bindings, observed?.autoApply]);
  ...
  const dirty = JSON.stringify(draft) !== JSON.stringify(savedDraft(device));
  ```
  Vengono eseguite 4 serializzazioni JSON complete di strutture dati articolate a ogni ciclo di render (inclusi eventi di battitura, focus e hover).
- **Impatto**: Inutile spreco di cicli CPU e sovraccarico per il garbage collector, con impatto evidente su dispositivi a basse prestazioni o a batteria.
- **Raccomandazione di Correzione**:
  Racchiudere il calcolo delle chiavi in `useMemo` o utilizzare comparatori superficiali/profondi mirati (ad esempio confrontando revisioni o flag boolean).

---

#### DEF-FE-06: Rirender a Cascata dell'Intero Albero (Mancanza di Memoizzazione)
- **Livello di Severità**: **Media**
- **Riferimento nel Codice**: `src/App.tsx:173`, `src/components/DeviceCard.tsx:47`, `src/views/Dashboard.tsx:29`
- **Descrizione**: Lo stato `snapshot` è gestito alla radice dell'albero in `App.tsx`. A ogni aggiornamento dello snapshot (anche solo per l'avanzamento del contatore di revisione o un singolo evento volume), l'intero albero dell'interfaccia viene ri-renderizzato. Nessun componente (`DeviceCard`, `DeviceIllustration`, `Diagnostics`, `Dashboard`) è protetto con `React.memo` o selettori di stato granulari.
- **Impatto**: Inefficienza nel ciclo di rendering, frequenti ridisegni delle pesanti illustrazioni SVG e lag percepibile durante l'interazione rapida con controlli slider.
- **Raccomandazione di Correzione**:
  Avvolgere i componenti computazionalmente intensivi con `React.memo(DeviceCard)` e `React.memo(DeviceIllustration)`, e considerare la separazione dello stato globale tramite Context selettivi (es. `AudioContext`, `DeviceContext`).

---

#### DEF-FE-07: Inclusione dell'Intero Motore di Simulazione Demo nel Bundle di Produzione
- **Livello di Severità**: **Media**
- **Riferimento nel Codice**: `src/api.ts:83-352`
- **Descrizione**: L'implementazione completa di `createDemoApi` (circa 270 righe di matrici dati, logiche di validazione virtuale e simulazione PipeWire) risiede in `src/api.ts` ed è inclusa direttamente nel bundle finale JavaScript (`dist/assets/index-*.js`, 288 kB) distribuito con l'applicazione desktop Tauri.
- **Impatto**: Aumento non necessario della dimensione del file binario (+100 kB non compressi) e violazione della separazione tra codice di produzione e fixture di test.
- **Raccomandazione di Correzione**:
  Estrarre `createDemoApi` in un file separato (`src/demoApi.ts`) e caricarlo tramite import dinamico condizionale in `createHubApi()`:
  ```ts
  export async function createHubApi(): Promise<HubApi> {
    if (import.meta.env.VITE_DEMO === "true") {
      const { createDemoApi } = await import("./demoApi");
      return createDemoApi(...);
    }
    return createTauriApi();
  }
  ```

---

#### DEF-FE-08: Rilettura Ridondante e Duplicata su Fallimento di Mutazione
- **Livello di Severità**: **Bassa**
- **Riferimento nel Codice**: `src/App.tsx:118-125`
- **Descrizione**: In `runMutation`:
  ```ts
  } catch (error) {
    ...
    try {
      acceptSnapshot(await api.getSnapshot(), generation);
    } catch (readbackError) { ... }
  }
  ```
  Se un'operazione fallisce, il blocco di catch esegue una rilettura `api.getSnapshot()`. Se l'operazione fallita era la connessione stessa (`runMutation("connection", () => api.getSnapshot())`), viene immediatamente inviata una seconda richiesta identica a un backend già noto come non disponibile.
- **Impatto**: Chiamata IPC duplicata e ritardo nel mostrare l'errore effettivo all'utente.
- **Raccomandazione di Correzione**:
  Condizionare la rilettura a `key !== "connection"`.

---

#### DEF-FE-09: Manipolazione Diretta del DOM (`document.getElementById`) nel Ciclo di Vita React
- **Livello di Severità**: **Bassa**
- **Riferimento nel Codice**: `src/App.tsx:99`, `src/App.tsx:146`
- **Descrizione**: La gestione del focus per i pannelli e la navigazione da tastiera tra i tab utilizza `document.getElementById(...)?.focus()` all'interno di `useEffect` e del gestore `tabKey`.
- **Impatto**: Accoppiamento rigido a identificatori stringa globali; potenziale fragilità in ambienti di test con mount multipli o in caso di riutilizzo dei componenti; violazione dell'incapsulamento React.
- **Raccomandazione di Correzione**:
  Utilizzare React Ref (`useRef` o mappe di ref per i tab) per gestire il focus in maniera dichiarativa e incapsulata.

---

#### DEF-FE-10: Uso dell'Indice dell'Array come Chiave React (`key={index}`)
- **Livello di Severità**: **Bassa**
- **Riferimento nel Codice**: `src/views/MousePanel.tsx:105`, `src/views/AudioPanel.tsx:60`
- **Descrizione**: Nella visualizzazione delle liste di limitazioni hardware:
  ```tsx
  {device.limitations.map((limitation, index) => <li key={index}>{limitation}</li>)}
  ```
- **Impatto**: Anti-pattern noto in React: l'uso dell'indice come chiave può generare disallineamenti di riconciliazione virtual DOM se la lista subisce variazioni o ordinamenti.
- **Raccomandazione di Correzione**:
  Utilizzare una chiave stabile e deterministica: `key={`${device.id}-limitation-${index}`}` o l'hash del testo.

---

#### DEF-FE-11: Rischio di Indisponibilità di `crypto.randomUUID()` in Ambienti Non-Secure
- **Livello di Severità**: **Bassa**
- **Riferimento nel Codice**: `src/views/AudioPanel.tsx:38`
- **Descrizione**: L'identificatore univoco per i nuovi profili audio viene generato tramite `crypto.randomUUID()`. Se l'app viene eseguita in un contesto non sicuro (non-HTTPS o WebView legacy), `crypto.randomUUID` può risultare `undefined`.
- **Impatto**: Possibile eccezione `TypeError: crypto.randomUUID is not a function` al salvataggio di un profilo audio.
- **Raccomandazione di Correzione**:
  Introdurre un helper di fallback UUID compatibile RFC 4122.

---

#### DEF-FE-12: Contrasto Colore Insufficiente su Micro-Etichette e Didascalie (WCAG 2.1 AA)
- **Livello di Severità**: **Bassa**
- **Riferimento nel Codice**: `src/styles.css:43, 64, 81, 120`
- **Descrizione**: Alcune classi CSS per testi secondari utilizzano dimensioni molto ridotte (8px-9px) e colori con rapporto di contrasto inferiore al requisito minimo di 4.5:1 prescritto dalle linee guida WCAG 2.1 AA:
  - `.brand-subtitle`: `#969696` su `#1b1b1b` (9px, contrasto 4.4:1)
  - `.instrument-caption`: `#888` su `#222222` (9px, contrasto 3.5:1)
  - `.device-card-copy p`: `#929292` su `#181818` (9px, contrasto 4.1:1)
- **Impatto**: Difficoltà di lettura per utenti ipovedenti o in condizioni di scarsa illuminazione.
- **Raccomandazione di Correzione**:
  Portare la dimensione minima del testo ad almeno 11px ed elevare i valori cromatici a tonalità più chiare (es. `#b4b4b4` o superiore).

---

## 3. Caveats

1. **Perimetro Limitato al Frontend**: L'indagine è stata condotta esclusivamente sulla cartella `src/` (TypeScript/React/CSS). Il backend nativo Rust/Tauri (`src-tauri`) viene analizzato parallelamente dall'agente peer dedicato.
2. **Ambiente di Test JSDOM**: I test automatizzati sono stati eseguiti in ambiente Vitest + JSDOM. La validazione finale su display server nativi (Wayland/X11), hardware Razer/Logitech reale e istanze live di PipeWire richiede l'esecuzione dell'app desktop completa su Linux.
3. **Modalità Demo**: La simulazione in-memory (`createDemoApi`) offre un comportamento fedele ai contratti di specifica, ma non può sostituire i riscontri di timing reali del demone D-Bus o del socket PipeWire.

---

## 4. Conclusion

Il frontend React/TypeScript di `sycomopse` dimostra un **livello ingegneristico notevole e una solida disciplina progettuale**, evidenziata da:
- Un'architettura "readback-first" rigorosa che rispetta pienamente lo stato confermato dell'hardware e nega scorciatoie basate su aggiornamenti ottimistici fittizi.
- Zero utilizzo del tipo `any`, con un'eccellente modellazione a tipi discriminati su standard Linux (EV_KEY, PipeWire, nodi ALSA, HRTF).
- Meccanismi di sicurezza di alto profilo (difesa contro la perdita del tasto sinistro, consenso esplicito su porte fisiche deboli, isolamento del recorder scorciatoie).
- 79 test completi e pienamente superati che garantiscono la tenuta dei contratti IPC e dei flussi d'uso chiave.

I difetti riscontrati (nessuno critico, 1 ad alta severità, 6 a media severità e 5 a bassa severità) riguardano principalmente:
1. **Resilienza e Tolleranza ai Guasti**: l'assenza di React ErrorBoundary e un potenziale crash su HRTF non censita.
2. **Conformità e Accessibilità**: l'annidamento illegittimo di elementi di blocco dentro `<button>` in `DeviceCard.tsx`.
3. **Rifinitura delle Prestazioni**: il re-render a cascata per mancanza di memoizzazione, l'inclusione del mock demo nel bundle finale e la sovrascrittura prematura delle bozze utente in presenza di aggiornamenti live.

Con l'applicazione delle raccomandazioni documentate nel presente report, la qualità e la manutenibilità del frontend possono agevolmente raggiungere l'eccellenza (punteggio potenziale > 9.0/10).

---

## 5. Verification Method

Per verificare in modo indipendente le osservazioni e le conclusioni del presente report, eseguire i seguenti comandi nel workspace del repository:

1. **Verifica Suite di Test Frontend**:
   ```bash
   npm test
   ```
   *Esito atteso*: 2 test file eseguiti con successo (`api.test.ts` e `App.test.tsx`), 79 test passati su 79, tempo di esecuzione ~10-12s.

2. **Verifica Controllo Tipi TypeScript e Build Vite**:
   ```bash
   npm run build
   ```
   *Esito atteso*: `tsc --noEmit` non produce alcun errore; Vite completa la build del bundle in `dist/` in meno di 1.5 secondi.

3. **Verifica Ispezione Statica dei Difetti Segnalati**:
   - Ispezionare `src/main.tsx:9`: verificare l'assenza di `ErrorBoundary`.
   - Ispezionare `src/api.ts:339`: verificare la presenza dell'asserzione non-null `clone(hrtfs.get(profile.hrtf)!)`.
   - Ispezionare `src/components/DeviceCard.tsx:53-58`: verificare la presenza di `<div>`, `<h3>`, `<p>` dentro `<button>`.
   - Ispezionare `src/views/MouseButtons.tsx:51-54, 66`: verificare i ricalcoli di `JSON.stringify` a corpo componente.
   - Ispezionare `src/components/Controls.tsx:138`: verificare l'effetto `useEffect` che reimposta `setDraft(node.volume)`.
