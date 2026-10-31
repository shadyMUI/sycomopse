import { useEffect, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { asAppError, createHubApi } from "./api";
import type { AppError, Device, Diagnostic, HubApi, ProviderKind, Snapshot } from "./types";
import { availabilityNames, providerNames } from "./components/Controls";
import Dashboard from "./views/Dashboard";
import MousePanel from "./views/MousePanel";
import AudioPanel from "./views/AudioPanel";

type Tab = "dashboard" | "mouse" | "audio";
const tabs: { id: Tab; label: string; number: string }[] = [
  { id: "dashboard", label: "Panoramica", number: "01" },
  { id: "mouse", label: "Mouse", number: "02" },
  { id: "audio", label: "Audio", number: "03" },
];

function Diagnostics({ diagnostics }: { diagnostics: Diagnostic[] }) {
  const providers = new Map<ProviderKind, Diagnostic>();
  const priority = { info: 0, warning: 1, error: 2 };
  for (const diagnostic of diagnostics) {
    const previous = providers.get(diagnostic.provider);
    if (!previous || priority[diagnostic.severity] > priority[previous.severity]) providers.set(diagnostic.provider, diagnostic);
  }
  return (
    <details className="diagnostics-panel">
      <summary aria-label="Servizi e diagnostica"><span className="diagnostics-title"><span className="diagnostic-symbol" aria-hidden="true">[+]</span>Servizi e diagnostica</span><span className="provider-summary">{[...providers.values()].map((diagnostic) => <span key={diagnostic.provider} className="provider-chip"><i className={`status-dot ${diagnostic.code === "available" ? "available" : diagnostic.severity !== "info" ? "partial" : "muted"}`} aria-hidden="true" />{providerNames[diagnostic.provider]}<span className="sr-only">: {availabilityNames[diagnostic.code]}</span></span>)}</span><span className="details-chevron" aria-hidden="true">+</span></summary>
      <div className="diagnostic-list">{diagnostics.length === 0 ? <p className="control-hint">Nessuna diagnostica disponibile.</p> : diagnostics.map((diagnostic) => <article key={diagnostic.id} className={`diagnostic-item severity-${diagnostic.severity}`}><div><strong>{providerNames[diagnostic.provider]}</strong><span className={`status-pill ${diagnostic.code === "available" ? "available" : "muted"}`}>{availabilityNames[diagnostic.code]}</span></div><p>{diagnostic.message}</p>{diagnostic.detail && <pre>{diagnostic.detail}</pre>}</article>)}</div>
    </details>
  );
}

export default function App({ api: injectedApi }: { api?: HubApi }) {
  const [defaultApi] = useState(createHubApi);
  const api = injectedApi ?? defaultApi;
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [loading, setLoading] = useState(true);
  const [tab, setTab] = useState<Tab>("dashboard");
  const [selectedMouseId, setSelectedMouseId] = useState<string | null>(null);
  const [audioFocus, setAudioFocus] = useState<{ output: string | null; deviceId: string | null; token: number }>({ output: null, deviceId: null, token: 0 });
  const [pending, setPending] = useState<ReadonlySet<string>>(new Set());
  const [errors, setErrors] = useState<Record<string, AppError>>({});
  const pendingRef = useRef(new Set<string>());
  const generationRef = useRef(0);
  const activeRef = useRef(false);
  const focusFromCardRef = useRef(false);

  function acceptSnapshot(incoming: Snapshot, generation: number) {
    if (!activeRef.current || generationRef.current !== generation) return;
    // Commands and events may arrive in a different order than their readbacks.
    setSnapshot((current) => current && incoming.revision < current.revision ? current : incoming);
  }

  useEffect(() => {
    const generation = ++generationRef.current;
    activeRef.current = true;
    let alive = true;
    let unsubscribe: (() => void) | undefined;
    setSnapshot(null);
    setLoading(true);
    setErrors({});
    setPending(new Set());
    pendingRef.current.clear();
    setSelectedMouseId(null);
    setAudioFocus((current) => ({ output: null, deviceId: null, token: current.token + 1 }));
    void (async () => {
      try {
        const stop = await api.subscribe((incoming) => { if (alive) acceptSnapshot(incoming, generation); });
        if (alive) unsubscribe = stop;
        else stop();
      } catch (error) {
        if (alive) setErrors((current) => ({ ...current, subscription: asAppError(error) }));
      }
    })();
    void (async () => {
      try {
        acceptSnapshot(await api.getSnapshot(), generation);
      } catch (error) {
        if (alive) setErrors((current) => ({ ...current, connection: asAppError(error) }));
      } finally {
        if (alive) setLoading(false);
      }
    })();
    return () => {
      alive = false;
      activeRef.current = false;
      generationRef.current += 1;
      unsubscribe?.();
    };
  }, [api]);

  useEffect(() => {
    if (!snapshot) return;
    setSelectedMouseId((current) => snapshot.devices.some((device) => device.id === current && device.kind === "mouse") ? current : snapshot.devices.find((device) => device.kind === "mouse")?.id ?? null);
  }, [snapshot]);

  useEffect(() => {
    if (snapshot && focusFromCardRef.current) {
      document.getElementById(`panel-${tab}`)?.focus();
      focusFromCardRef.current = false;
    }
  }, [tab, snapshot]);

  async function runMutation(key: string, operation: () => Promise<Snapshot>): Promise<boolean> {
    const generation = generationRef.current;
    const group = key.startsWith("mouse/") ? key.slice(0, key.lastIndexOf("/") + 1) : key.startsWith("audio/") ? "audio/" : key;
    if (!activeRef.current || [...pendingRef.current].some((entry) => group === key ? entry === key : entry.startsWith(group))) return false;
    pendingRef.current.add(key);
    setPending(new Set(pendingRef.current));
    setErrors((current) => { const next = { ...current }; delete next[key]; return next; });
    try {
      acceptSnapshot(await operation(), generation);
      return activeRef.current && generationRef.current === generation;
    } catch (error) {
      if (!activeRef.current || generationRef.current !== generation) return false;
      const failure = asAppError(error);
      setErrors((current) => ({ ...current, [key]: failure }));
      try {
        acceptSnapshot(await api.getSnapshot(), generation);
      } catch (readbackError) {
        if (activeRef.current && generationRef.current === generation) {
          const readback = asAppError(readbackError);
          setErrors((current) => ({ ...current, [key]: { ...failure, detail: [failure.detail, `Rilettura non riuscita: ${readback.message}`].filter(Boolean).join("\n") } }));
        }
      }
      return false;
    } finally {
      if (activeRef.current && generationRef.current === generation) {
        pendingRef.current.delete(key);
        setPending(new Set(pendingRef.current));
      }
    }
  }

  function chooseTab(next: Tab) {
    setTab(next);
    if (next === "audio") setAudioFocus((current) => ({ output: null, deviceId: null, token: current.token + 1 }));
  }

  function tabKey(event: KeyboardEvent<HTMLButtonElement>, current: Tab) {
    const index = tabs.findIndex((item) => item.id === current);
    const nextIndex = event.key === "ArrowRight" ? (index + 1) % tabs.length : event.key === "ArrowLeft" ? (index + tabs.length - 1) % tabs.length : event.key === "Home" ? 0 : event.key === "End" ? tabs.length - 1 : null;
    if (nextIndex === null) return;
    event.preventDefault();
    chooseTab(tabs[nextIndex].id);
    document.getElementById(`tab-${tabs[nextIndex].id}`)?.focus();
  }

  function openDevice(device: Device) {
    focusFromCardRef.current = true;
    if (device.kind === "mouse") {
      setSelectedMouseId(device.id);
      setTab("mouse");
    } else if (device.kind === "headset") {
      const output = snapshot?.audio.outputs.find((node) => device.outputNodeNames.includes(node.nodeName));
      setAudioFocus((current) => ({ output: output?.nodeName ?? null, deviceId: device.id, token: current.token + 1 }));
      setTab("audio");
    }
  }

  const demo = api.demo || snapshot?.demo === true;
  const controls = { api, pending, runMutation };
  const connectionLabel = demo ? "Demo / nessun hardware collegato" : snapshot?.platform === "unsupportedPlatform" ? "Backend non disponibile" : snapshot ? "Sessione locale / Linux" : loading ? "Connessione al backend" : "Connessione non riuscita";

  return (
    <div className="app-shell">
      <a className="skip-link" href="#main-content">Vai ai controlli</a>
      <header className="app-header"><div className="header-inner"><div className="brand-lockup"><svg className="hub-logo" viewBox="0 0 44 44" fill="none" aria-hidden="true"><path d="M12 7H32L40 22L32 37H12L4 22L12 7Z" stroke="currentColor" strokeWidth="1.5" /><path d="M15 15H25L30 22L25 29H15L11 22L15 15Z" stroke="currentColor" /><path d="M25 15L32 7M30 22H40M25 29L32 37M15 15L12 7M15 29L12 37M11 22H4" stroke="currentColor" strokeOpacity=".55" /></svg><div><span className="brand-name">Peripheral <strong>Hub</strong></span><span className="brand-subtitle">Client non ufficiale / Razer e Logitech</span></div></div><div className="header-actions"><div className={`connection-badge${demo ? " demo-badge" : ""}`} role="status"><span className={`status-dot ${demo || snapshot?.platform === "linux" ? "available" : "muted"}`} aria-hidden="true" />{connectionLabel}</div>{!demo && isTauri() && snapshot && <button className="text-button exit-button" aria-label="Esci e ripristina l'audio" disabled={pending.has("exit")} onClick={() => { void runMutation("exit", async () => { await api.exitApp(); return snapshot; }); }}>Esci</button>}</div></div></header>
      <div className="navigation-wrap"><nav className="main-tabs" role="tablist" aria-label="Sezioni Peripheral Hub">{tabs.map((item) => <button key={item.id} type="button" role="tab" id={`tab-${item.id}`} aria-selected={tab === item.id} aria-controls={`panel-${item.id}`} tabIndex={tab === item.id ? 0 : -1} onClick={() => chooseTab(item.id)} onKeyDown={(event) => tabKey(event, item.id)}><span className="tab-number">{item.number}</span>{item.label}<span className="tab-active-mark" aria-hidden="true" /></button>)}</nav><span className="revision-label mono">{snapshot ? `LETTURA / ${String(snapshot.revision).padStart(4, "0")}` : "LETTURA / ----"}</span></div>
      <main id="main-content" className="main-content">
        {Object.entries(errors).map(([key, error]) => <div className="error-banner" role="alert" key={key}><div><strong>{key === "subscription" ? "Aggiornamenti live non disponibili" : key === "connection" ? "Connessione al backend non riuscita" : "Operazione non confermata"}</strong><p>{error.message}</p><small>I controlli mostrano soltanto l'ultima lettura confermata.</small>{error.detail && <details><summary>Dettaglio diagnostico</summary><pre>{error.detail}</pre></details>}</div><button className="text-button" aria-label={`Chiudi errore ${error.code}`} onClick={() => setErrors((current) => { const next = { ...current }; delete next[key]; return next; })}>Chiudi</button></div>)}
        {snapshot?.platform === "unsupportedPlatform" && !demo && <section className="platform-banner"><span className="note-mark" aria-hidden="true">[!]</span><div><h2>Piattaforma non supportata</h2><p>{snapshot.diagnostics.find((diagnostic) => diagnostic.code === "unsupportedPlatform")?.message ?? "I controlli hardware e PipeWire richiedono l'app desktop su Linux."}</p><small>Nessun passaggio automatico alla demo.</small></div></section>}
        {snapshot ? <><div role="tabpanel" id={`panel-${tab}`} aria-labelledby={`tab-${tab}`} tabIndex={0} className="view-panel">{tab === "dashboard" ? <Dashboard snapshot={snapshot} selectedMouseId={selectedMouseId} onSelectMouse={setSelectedMouseId} onOpenDevice={openDevice} {...controls} /> : tab === "mouse" ? <MousePanel devices={snapshot.devices} selectedId={selectedMouseId} onSelect={setSelectedMouseId} {...controls} /> : <AudioPanel key={audioFocus.token} snapshot={snapshot} initialOutput={audioFocus.output} initialDevice={snapshot.devices.find((device) => device.id === audioFocus.deviceId) ?? null} {...controls} />}</div><Diagnostics diagnostics={snapshot.diagnostics} /></> : <section className="panel startup-panel"><div className="startup-mark" aria-hidden="true">PH</div><span className="eyebrow">Peripheral Hub</span><h1>{loading ? "Connessione al tuo setup." : "Backend non raggiungibile."}</h1><p role="status">{loading ? "Caricamento dei dispositivi e dello stato audio..." : "Nessun dato simulato e nessuna operazione hardware. Riprova la lettura del backend."}</p>{!loading && <button className="button button-primary" disabled={pending.has("connection")} onClick={() => { void runMutation("connection", () => api.getSnapshot()); }}>Riprova connessione</button>}</section>}
      </main>
      <footer className="app-footer"><span>Peripheral Hub <span className="footer-divider">/</span> Non ufficiale</span><span>Controllo locale <span className="footer-dot" aria-hidden="true" /> Nessun account richiesto</span></footer>
    </div>
  );
}
