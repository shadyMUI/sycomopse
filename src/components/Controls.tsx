import { useEffect, useId, useState } from "react";
import type { CSSProperties } from "react";
import { isDpiValueAllowed } from "../api";
import type { AudioNode, AudioSnapshot, AvailabilityCode, Battery, Device, DpiCapability, HubApi, ProviderKind, Snapshot } from "../types";

export type RunMutation = (key: string, operation: () => Promise<Snapshot>) => Promise<boolean>;
export interface MutationControls {
  api: HubApi;
  runMutation: RunMutation;
  pending: ReadonlySet<string>;
}

export const providerNames: Record<ProviderKind, string> = {
  openrazer: "OpenRazer", ratbag: "ratbagd", upower: "UPower",
  headsetcontrol: "HeadsetControl", pipewire: "PipeWire", usb: "USB", app: "Applicazione",
};

export const availabilityNames: Record<AvailabilityCode, string> = {
  available: "Disponibile", absent: "Non presente", unsupported: "Non supportato",
  providerMissing: "Provider assente", permissionDenied: "Permesso negato", unknown: "Da verificare",
  unsupportedPlatform: "Piattaforma non supportata",
};

export function deviceStatus(device: Device): { label: string; tone: string } {
  if (device.availability !== "available") return { label: availabilityNames[device.availability], tone: "muted" };
  if (device.support === "partial") return { label: "Supporto parziale", tone: "partial" };
  if (device.support === "unsupported") return { label: "Hardware non supportato", tone: "muted" };
  return { label: "Hardware disponibile", tone: "available" };
}

export function hasPending(pending: ReadonlySet<string>, prefix: string): boolean {
  return [...pending].some((key) => key.startsWith(prefix));
}

export function Toggle({ label, checked, disabled = false, description, onChange }: {
  label: string; checked: boolean; disabled?: boolean; description?: string | null; onChange: (checked: boolean) => void;
}) {
  const id = useId();
  return (
    <label className={`toggle-row${disabled ? " is-disabled" : ""}`}>
      <span className="toggle-copy"><span>{label}</span>{description && <small id={`${id}-reason`}>{description}</small>}</span>
      <span className="toggle-switch">
        <input type="checkbox" role="switch" aria-label={label} checked={checked} disabled={disabled}
          aria-describedby={description ? `${id}-reason` : undefined} onChange={(event) => onChange(event.target.checked)} />
        <span className="toggle-track" aria-hidden="true" />
      </span>
    </label>
  );
}

function rangeStyle(value: number, min: number, max: number): CSSProperties {
  return { "--range-fill": `${max > min ? Math.max(0, Math.min(100, (value - min) / (max - min) * 100)) : 0}%` } as CSSProperties;
}

export function DpiControl({ capability, value, label = "DPI desiderati", disabled = false, reason, onChange }: {
  capability: DpiCapability | null; value: number | null; label?: string; disabled?: boolean;
  reason?: string | null; onChange: (value: number) => void;
}) {
  const id = useId();
  const values = capability?.values;
  const known = values && values.kind !== "unknown" && (values.kind === "discrete" ? values.values.length > 0 : values.step > 0 && values.max >= values.min);
  const writable = known && capability?.writable && !disabled;
  const explanation = reason || capability?.reason || (!known ? "Le capabilities DPI non sono disponibili. Nessun intervallo viene presunto." : !capability?.writable ? "DPI in sola lettura per questo provider." : null);
  const valid = values !== undefined && isDpiValueAllowed(values, value);
  const valueLabel = value === null ? "Non letto" : `${value} DPI`;

  if (!known || !values) {
    return <div className="dpi-control"><span className="field-label">{label}</span><strong className="read-only-value">{valueLabel}</strong><p className="control-reason">{explanation}</p></div>;
  }

  const min = values.kind === "discrete" ? 0 : values.min;
  const max = values.kind === "discrete" ? values.values.length - 1 : values.max;
  const position = valid && value !== null ? values.kind === "discrete" ? values.values.indexOf(value) : value : null;

  return (
    <div className="dpi-control">
      <div className="field-heading"><label htmlFor={id}>{label}</label><output htmlFor={id}>{valueLabel}</output></div>
      {position !== null ? (
        <>
          <input id={id} className="range-input" type="range" min={min} max={max} step={values.kind === "discrete" ? 1 : values.step}
            value={position} aria-valuetext={`${value} DPI`} disabled={!writable}
            aria-describedby={explanation ? `${id}-reason` : `${id}-bounds`} style={rangeStyle(position, min, max)}
            onChange={(event) => {
              const next = Number(event.target.value);
              onChange(values.kind === "discrete" ? values.values[next] : next);
            }} />
          <div className="range-bounds" id={`${id}-bounds`}><span>{values.kind === "discrete" ? values.values[0] : min} DPI</span><span>{values.kind === "discrete" ? values.values[values.values.length - 1] : max} DPI</span></div>
          {values.kind === "discrete" && <p className="control-hint">Posizioni discrete: {values.values.join(" / ")} DPI</p>}
        </>
      ) : (
        <div className="invalid-dpi">
          <p id={`${id}-invalid`} className="control-reason">{value === null ? "Nessuna lettura confermata." : "Valore letto fuori dai valori ammessi."} Scegli un DPI valido prima di applicare.</p>
          {writable && (values.kind === "discrete" ? (
            <select id={id} aria-label={`Scegli ${label}`} aria-describedby={`${id}-invalid`} value="" onChange={(event) => onChange(Number(event.target.value))}>
              <option value="" disabled>Scegli un valore valido</option>
              {values.values.map((dpi) => <option key={dpi} value={dpi}>{dpi} DPI</option>)}
            </select>
          ) : (
            <input id={id} type="number" aria-label={`Scegli ${label}`} aria-describedby={`${id}-invalid`} min={values.min} max={values.max} step={values.step}
              placeholder={`${values.min} - ${values.max}`} onChange={(event) => {
                const next = event.target.value === "" ? null : Number(event.target.value);
                if (isDpiValueAllowed(values, next) && next !== null) onChange(next);
              }} />
          ))}
        </div>
      )}
      {explanation && <p id={`${id}-reason`} className="control-reason">{explanation}</p>}
    </div>
  );
}

export function OutputSelector({ outputs, value, onChange, disabled = false, label = "Uscita audio" }: {
  outputs: AudioNode[]; value: string; onChange: (value: string) => void; disabled?: boolean; label?: string;
}) {
  const id = useId();
  return (
    <div className="select-field">
      <label htmlFor={id}>{label}</label>
      <select id={id} value={value} disabled={disabled || outputs.length === 0} aria-describedby={outputs.length === 0 ? `${id}-empty` : undefined} onChange={(event) => onChange(event.target.value)}>
        {!outputs.some((output) => output.nodeName === value) && <option value="">{outputs.length ? "Scegli un'uscita" : "Nessuna uscita rilevata"}</option>}
        {[false, true].map((virtual) => {
          const nodes = outputs.filter((output) => output.isVirtual === virtual);
          return nodes.length > 0 && <optgroup key={String(virtual)} label={virtual ? "Uscite virtuali" : "Uscite fisiche"}>
            {nodes.map((output) => <option key={output.nodeName} value={output.nodeName}>{output.description || output.nodeName}{virtual ? " [virtuale]" : " [fisica]"}</option>)}
          </optgroup>;
        })}
      </select>
      {outputs.length === 0 && <small className="control-reason" id={`${id}-empty`}>Il backend non espone uscite audio.</small>}
    </div>
  );
}

export function NodeControls({ node, input = false, available, api, pending, runMutation }: MutationControls & {
  node: AudioNode; input?: boolean; available: boolean;
}) {
  const id = useId();
  const [draft, setDraft] = useState(node.volume);
  useEffect(() => { setDraft(node.volume); }, [node.nodeName, node.volume]);
  const busy = hasPending(pending, "audio/");
  const readable = node.volume !== null && Number.isFinite(node.volume) && node.volume >= 0;
  const percent = draft === null ? null : Math.round(draft * 100);
  const actualPercent = node.volume === null ? null : Math.round(node.volume * 100);
  const canApply = readable && draft !== null && draft >= 0 && draft <= 1 && draft !== node.volume;
  const reason = !available ? "Il servizio audio non e' disponibile." : !readable ? "Volume non leggibile: il nodo rimane in sola lettura." : busy ? "Operazione audio in corso." : null;
  const label = input ? "Guadagno microfono" : "Volume uscita";
  return (
    <div className="node-controls">
      <div className="field-heading"><label htmlFor={id}>{label}</label><output htmlFor={id}>{percent === null ? "Non disponibile" : `${percent}%`}</output></div>
      <input id={id} className="range-input" type="range" min={0} max={100} step={1}
        value={Math.max(0, Math.min(100, percent ?? 0))} disabled={!available || !readable || busy}
        aria-valuetext={percent === null ? "Volume non disponibile" : `${percent}%`} aria-describedby={`${id}-readback`}
        style={rangeStyle(percent ?? 0, 0, 100)} onChange={(event) => setDraft(Number(event.target.value) / 100)} />
      <div className="range-bounds"><span>0%</span><span>100%</span></div>
      <p className="control-hint" id={`${id}-readback`}>Lettura confermata: {actualPercent === null ? "non disponibile" : `${actualPercent}%`}.{reason ? ` ${reason}` : ""}{actualPercent !== null && actualPercent > 100 ? " Scegli un valore entro 100% prima di applicare." : ""}</p>
      <button className="button button-secondary button-small" disabled={!available || busy || !canApply} onClick={() => {
        if (draft !== null) void runMutation(`audio/volume/${node.nodeName}`, () => api.setVolume(node.nodeName, draft));
      }}>{busy ? "Attendi..." : input ? "Applica guadagno" : "Applica volume"}</button>
      <Toggle label={input ? "Silenzia microfono" : "Silenzia uscita"} checked={node.muted === true}
        disabled={!available || busy || node.muted === null}
        description={node.muted === null ? "Stato mute non leggibile." : !available ? "Servizio audio non disponibile." : null}
        onChange={(muted) => { void runMutation(`audio/mute/${node.nodeName}`, () => api.setMute(node.nodeName, muted)); }} />
    </div>
  );
}

export function SurroundControl({ audio, api, pending, runMutation }: MutationControls & { audio: AudioSnapshot }) {
  const busy = hasPending(pending, "audio/");
  const transitioning = audio.surroundState === "starting" || audio.surroundState === "stopping";
  const on = audio.surroundState === "on";
  const disabled = busy || transitioning || (!on && (!audio.available || !audio.canEnableSurround));
  const stateLabels = { off: "Disattivato", starting: "Attivazione in corso", on: "Attivo", stopping: "Disattivazione in corso", error: "Non attivo: errore" };
  const reason = transitioning || busy ? "Attendi la lettura confermata dal backend." : audio.surroundReason || (!audio.available ? "PipeWire non disponibile." : !audio.canEnableSurround && !on ? "Il backend non consente l'attivazione per questo target." : null);
  return (
    <div className={`surround-control${on ? " surround-on" : ""}`}>
      <div className="surround-heading"><span className="spatial-number" aria-hidden="true">7<span>.1</span></span><span><strong>Surround binaurale</strong><small className={on ? "accent-text" : ""} role="status">{stateLabels[audio.surroundState]}</small></span></div>
      <Toggle label="Surround 7.1" checked={on} disabled={disabled} description={reason || "Ingresso a 8 canali, uscita stereo con HRTF."}
        onChange={(enabled) => { void runMutation("audio/surround", () => api.setSurround(enabled)); }} />
    </div>
  );
}

function batteryTime(timestamp: number | null): string {
  if (timestamp === null || !Number.isFinite(timestamp)) return "non disponibile";
  return new Date(timestamp * 1000).toLocaleString("it-IT", { day: "2-digit", month: "2-digit", hour: "2-digit", minute: "2-digit" });
}

export function BatteryReadout({ battery, compact = false }: { battery: Battery | null; compact?: boolean }) {
  if (battery === null) return <div className={`battery-readout${compact ? " compact" : ""}`}><span className="battery-empty">Batteria non esposta</span>{!compact && <p className="control-hint">Nessuna misura associata con certezza a questo dispositivo.</p>}</div>;
  const percentage = battery.percentage !== null && Number.isFinite(battery.percentage) && battery.percentage >= 0 && battery.percentage <= 100 ? battery.percentage : null;
  const levels = { unknown: "Non disponibile", critical: "Critico", low: "Basso", normal: "Normale", high: "Alto", full: "Pieno" };
  const level = battery.level !== null ? levels[battery.level] : "Non disponibile";
  return (
    <div className={`battery-readout${compact ? " compact" : ""}`}>
      <div className="battery-heading"><strong>{percentage !== null ? `${Math.round(percentage)}%` : level}</strong>{percentage === null && battery.level && battery.level !== "unknown" && <span className="tiny-label">Livello approssimativo</span>}{battery.stale && <span className="status-pill partial">Dato precedente</span>}</div>
      {percentage !== null && !compact && <meter className="battery-meter" min={0} max={100} value={percentage} aria-label="Percentuale batteria" />}
      {battery.charging === true && <small>In carica</small>}
      <small className="battery-source">Fonte: {providerNames[battery.source]}{compact ? ` / ${battery.stale ? "precedente" : "letto"} ${batteryTime(battery.observedAt)}` : ""}</small>
      {!compact && <dl className="metadata-list"><div><dt>Osservato</dt><dd>{batteryTime(battery.observedAt)}</dd></div><div><dt>Aggiornato dalla fonte</dt><dd>{batteryTime(battery.sourceUpdatedAt)}</dd></div></dl>}
    </div>
  );
}
