import { useEffect, useState } from "react";
import { isDpiValueAllowed, parseDpiDraft } from "../api";
import type { Device } from "../types";
import { BatteryReadout, DpiControl, Toggle, deviceStatus, hasPending, providerNames } from "../components/Controls";
import type { MutationControls } from "../components/Controls";
import { DeviceIllustration } from "../components/DeviceCard";
import MouseButtons from "./MouseButtons";

function dpiDraft(device: Device) { return { x: device.values.dpi?.x.toString() ?? "", y: device.values.dpi?.y.toString() ?? "" }; }

export function MouseControls({ device, compact = false, api, pending, runMutation }: MutationControls & { device: Device; compact?: boolean }) {
  const [draftDpi, setDraftDpi] = useState(() => dpiDraft(device));
  const [linked, setLinked] = useState(true);
  const [draftRate, setDraftRate] = useState(device.values.pollingRate);
  useEffect(() => { setDraftDpi(dpiDraft(device)); }, [device.id, device.values.dpi?.x, device.values.dpi?.y]);
  useEffect(() => { setDraftRate(device.values.pollingRate); }, [device.id, device.values.pollingRate]);
  useEffect(() => { setLinked(true); }, [device.id]);
  const dpi = device.capabilities.dpi;
  const polling = device.capabilities.polling;
  const busy = hasPending(pending, `mouse/${device.id}/`);
  const hardwareAvailable = device.availability === "available" && device.support !== "unsupported";
  const canWriteDpi = hardwareAvailable && dpi?.writable === true;
  const canWritePolling = hardwareAvailable && polling?.writable === true;
  const availabilityReason = !hardwareAvailable ? `${deviceStatus(device).label}. I controlli restano in sola lettura.` : null;
  const x = parseDpiDraft(draftDpi.x);
  const y = parseDpiDraft(draftDpi.y);
  const validDpi = dpi && isDpiValueAllowed(dpi.values, x) && isDpiValueAllowed(dpi.values, y) && (dpi.separateAxes || x === y);
  const dpiChanged = x !== device.values.dpi?.x || y !== device.values.dpi?.y;
  const dpiDirty = draftDpi.x !== (device.values.dpi?.x.toString() ?? "") || draftDpi.y !== (device.values.dpi?.y.toString() ?? "");
  const rateChanged = draftRate !== device.values.pollingRate;
  const validRate = draftRate !== null && polling?.values.includes(draftRate);
  const dpiReason = busy ? "Scrittura del mouse in corso. L'audio rimane indipendente." : availabilityReason;
  const confirmed = device.values.dpi;

  function changeDpi(axis: "x" | "y", value: string) {
    if (linked || !dpi?.separateAxes || compact) setDraftDpi({ x: value, y: value });
    else setDraftDpi((current) => ({ ...current, [axis]: value }));
  }

  const pollingControls = (
    <>
      {!compact && <div className="section-heading"><div><span className="eyebrow">02 / Comunicazione</span><h2>Polling</h2></div><span className="unit-tag">Hz</span></div>}
      <p className="readback-line">Lettura confermata: <strong>{device.values.pollingRate === null ? "non disponibile" : `${device.values.pollingRate} Hz`}</strong></p>
      {polling && polling.values.length > 0 ? (
        <div className="rate-options" role="group" aria-label="Frequenza polling desiderata">
          {polling.values.map((rate) => <button key={rate} type="button" aria-pressed={draftRate === rate} disabled={busy || !canWritePolling} onClick={() => setDraftRate(rate)}>{rate}<small>Hz</small></button>)}
        </div>
      ) : <p className="control-reason">Nessun elenco di frequenze supportate. Nessun intervallo universale viene presunto.</p>}
      {device.values.pollingRate !== null && polling && !polling.values.includes(device.values.pollingRate) && <p className="control-reason">La frequenza letta non e' nell'elenco del provider. Scegli una frequenza valida prima di applicare.</p>}
      {(availabilityReason || polling?.reason || !polling?.writable) && <p className="control-reason">{availabilityReason || polling?.reason || "Polling in sola lettura."}</p>}
      <button className="button button-secondary button-small" disabled={busy || !canWritePolling || !validRate || !rateChanged}
        onClick={() => { if (draftRate !== null) void runMutation(`mouse/${device.id}/polling`, () => api.setPollingRate(device.id, draftRate)); }}>
        {pending.has(`mouse/${device.id}/polling`) ? "Applicazione polling..." : "Applica polling"}
      </button>
    </>
  );

  return (
    <div className={`mouse-controls${compact ? " compact-controls" : ""}`}>
      {!compact && <div className="section-heading"><div><span className="eyebrow">01 / Sensore</span><h2>Sensibilita</h2></div><span className="unit-tag">DPI</span></div>}
      <p className="readback-line">Lettura confermata: <strong>{confirmed ? confirmed.x === confirmed.y ? `${confirmed.x} DPI` : `X ${confirmed.x} / Y ${confirmed.y} DPI` : "non disponibile"}</strong></p>
      {!compact && <p className="control-hint">DPI hardware, non velocita del puntatore di sistema. Le modifiche restano in bozza fino ad Applica.</p>}
      {dpi?.separateAxes && !compact ? (
        <Toggle label="Collega assi X/Y" checked={linked} disabled={busy || !canWriteDpi}
          description={linked ? "Una modifica imposta lo stesso DPI su entrambi gli assi." : "Il provider supporta impostazioni separate."}
          onChange={(checked) => {
            setLinked(checked);
            if (checked) setDraftDpi({ x: draftDpi.x, y: draftDpi.x });
          }} />
      ) : dpi && <p className="axis-note">{dpi.separateAxes ? "Controllo rapido con assi collegati." : "Assi collegati dal provider."}</p>}
      <DpiControl capability={dpi} label={!linked && !compact ? "DPI X desiderati" : "DPI desiderati"} manualLabel={!linked && !compact ? "DPI X manuali" : "DPI manuali"} draft={draftDpi.x}
        disabled={busy || !hardwareAvailable} reason={dpiReason} onChange={(value) => changeDpi("x", value)} />
      {!linked && dpi?.separateAxes && !compact && <DpiControl capability={dpi} label="DPI Y desiderati" manualLabel="DPI Y manuali" draft={draftDpi.y}
        disabled={busy || !hardwareAvailable} reason={dpiReason} onChange={(value) => changeDpi("y", value)} />}
      <div className="action-row">
        <button className={`button ${compact ? "button-secondary button-small" : "button-primary"}`} disabled={busy || !canWriteDpi || !validDpi || !dpiChanged}
          onClick={() => { if (validDpi && x !== null && y !== null) void runMutation(`mouse/${device.id}/dpi`, () => api.setDpi(device.id, x, y)); }}>
          {pending.has(`mouse/${device.id}/dpi`) ? "Applicazione DPI..." : "Applica DPI"}
        </button>
        <button className="text-button" disabled={busy || !dpiDirty} onClick={() => setDraftDpi(dpiDraft(device))}>Ripristina bozza</button>
      </div>
      {compact ? <details className="quick-details"><summary>Polling <span>{device.values.pollingRate === null ? "Non letto" : `${device.values.pollingRate} Hz`}</span></summary><div className="quick-details-content">{pollingControls}</div></details> : <><div className="control-divider" />{pollingControls}</>}
    </div>
  );
}

export default function MousePanel({ devices, selectedId, onSelect, ...controls }: MutationControls & {
  devices: Device[]; selectedId: string | null; onSelect: (id: string) => void;
}) {
  const mice = devices.filter((device) => device.kind === "mouse");
  const device = mice.find((mouse) => mouse.id === selectedId) ?? mice[0];
  if (!device) return <section className="panel empty-state large-empty"><DeviceIllustration kind="mouse" /><span className="eyebrow">Controllo hardware</span><h1>Nessun mouse rilevato</h1><p>Collega un mouse supportato e aggiorna i dispositivi. I controlli dipendono dalle capabilities del provider, non dal marchio.</p></section>;
  const status = deviceStatus(device);
  const vidPid = device.identity.vendorId !== null && device.identity.productId !== null ? `${device.identity.vendorId.toString(16).padStart(4, "0")}:${device.identity.productId.toString(16).padStart(4, "0")}` : "Non esposto";
  return (
    <>
      <div className="view-heading"><div><span className="eyebrow">Mouse / {providerNames[device.provider]}</span><h1>{device.name}</h1><p>Solo i controlli realmente esposti dal provider selezionato.</p></div>
        {mice.length > 1 && <div className="select-field device-picker"><label htmlFor="mouse-device">Mouse selezionato</label><select id="mouse-device" value={device.id} onChange={(event) => onSelect(event.target.value)}>{mice.map((mouse) => <option key={mouse.id} value={mouse.id}>{mouse.name}</option>)}</select></div>}
      </div>
      <div className="workspace-grid mouse-layout">
        <div className="panel-stack"><section className="panel settings-panel"><MouseControls key={device.id} device={device} {...controls} /></section><MouseButtons key={`${device.id}/${device.capabilities.remapping?.fingerprint ?? "unknown"}`} device={device} {...controls} /></div>
        <aside className="panel-stack">
          <section className="panel specimen-panel"><div className="panel-topline"><span className="eyebrow">Periferica selezionata</span><span className={`status-pill ${status.tone}`}>{status.label}</span></div><DeviceIllustration kind="mouse" className="large-illustration" /><dl className="metadata-list"><div><dt>Provider</dt><dd>{providerNames[device.provider]}</dd></div><div><dt>Trasporto</dt><dd>{device.identity.transport ?? "Non esposto"}</dd></div><div><dt>VID:PID</dt><dd className="mono">{vidPid}</dd></div><div><dt>Firmware</dt><dd>{device.identity.firmware ?? "Non esposto"}</dd></div><div><dt>Seriale</dt><dd className="mono">{device.identity.serial ?? "Non esposto"}</dd></div></dl></section>
          <section className="panel battery-panel"><div className="section-heading"><h2>Alimentazione</h2><span className="tiny-label">Sola lettura</span></div><BatteryReadout battery={device.values.battery} /></section>
          {device.limitations.length > 0 && <section className="panel limits-panel"><h2>Limiti del supporto</h2><ul className="plain-list">{device.limitations.map((limitation, index) => <li key={index}>{limitation}</li>)}</ul></section>}
        </aside>
      </div>
    </>
  );
}
