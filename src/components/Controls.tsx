import { useEffect, useId, useRef, useState } from "react";
import type { CSSProperties } from "react";
import { isDpiValueAllowed, parseDpiDraft } from "../api";
import { filterTargetReason } from "../audio";
import type { AudioNode, AudioSnapshot, AvailabilityCode, Battery, Device, DpiCapability, HubApi, ProviderKind, Snapshot } from "../types";

export type RunMutation = (key: string, operation: () => Promise<Snapshot>) => Promise<boolean>;
export interface MutationControls {
  api: HubApi;
  runMutation: RunMutation;
  pending: ReadonlySet<string>;
}

export const providerNames: Record<ProviderKind, string> = {
  openrazer: "OpenRazer",
  ratbag: "ratbagd",
  upower: "UPower",
  headsetcontrol: "HeadsetControl",
  pipewire: "PipeWire",
  inputremapper: "Input Remapper",
  usb: "USB",
  app: "Application",
};

export const availabilityNames: Record<AvailabilityCode, string> = {
  available: "Available",
  absent: "Not present",
  unsupported: "Unsupported",
  providerMissing: "Provider missing",
  permissionDenied: "Permission denied",
  unknown: "To verify",
  unsupportedPlatform: "Unsupported platform",
};

export function deviceStatus(device: Device): { label: string; tone: string } {
  if (device.availability !== "available") return { label: availabilityNames[device.availability], tone: "muted" };
  if (device.support === "partial") return { label: "Partial support", tone: "partial" };
  if (device.support === "unsupported") return { label: "Unsupported hardware", tone: "muted" };
  return { label: "Hardware available", tone: "available" };
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

export function DpiControl({ capability, draft, label = "Target DPI", manualLabel = "Manual DPI", disabled = false, reason, onChange }: {
  capability: DpiCapability | null; draft: string; label?: string; manualLabel?: string; disabled?: boolean;
  reason?: string | null; onChange: (draft: string) => void;
}) {
  const id = useId();
  const values = capability?.values;
  const known = values && values.kind !== "unknown" && (values.kind === "discrete" ? values.values.length > 0 : values.step > 0 && values.max >= values.min);
  const writable = !!(known && capability?.writable && !disabled);
  const explanation = reason || capability?.reason || (!known ? "DPI capabilities are not available. No range is assumed." : !capability?.writable ? "Read-only DPI for this provider." : null);
  const value = parseDpiDraft(draft);
  const valid = values !== undefined && isDpiValueAllowed(values, value);
  const valueLabel = writable && !valid ? draft === "" ? "Empty draft" : "Invalid draft" : value === null ? "Not read" : `${value} DPI`;
  const manual = <div className="dpi-manual-field"><label htmlFor={`${id}-manual`}>{manualLabel}</label>
    <input id={`${id}-manual`} type="text" inputMode="numeric" autoComplete="off" spellCheck={false}
      value={draft} readOnly={!writable} aria-invalid={writable && !valid}
      aria-describedby={writable && !valid ? `${id}-invalid` : explanation ? `${id}-reason` : `${id}-bounds`}
      placeholder="Not read" onChange={(event) => { if (writable) onChange(event.target.value); }} /></div>;

  if (!known || !values) {
    return <div className="dpi-control"><div className="field-heading"><span className="field-label">{label}</span><output>{valueLabel}</output></div>{manual}<p id={`${id}-reason`} className="control-reason">{explanation}</p></div>;
  }

  const min = values.kind === "discrete" ? 0 : values.min;
  const max = values.kind === "discrete" ? values.values.length - 1 : values.max;
  const position = valid && value !== null ? values.kind === "discrete" ? values.values.indexOf(value) : value : null;

  return (
    <div className="dpi-control">
      <div className="field-heading"><span className="field-label">{label}</span><output htmlFor={`${id} ${id}-manual`}>{valueLabel}</output></div>
      <div className="dpi-input-row"><div className="dpi-position-control">{position !== null ? (
        <>
          <input id={id} className="range-input" type="range" min={min} max={max} step={values.kind === "discrete" ? 1 : values.step}
            value={position} aria-label={label} aria-valuetext={`${value} DPI`} disabled={!writable}
            aria-describedby={explanation ? `${id}-reason` : `${id}-bounds`} style={rangeStyle(position, min, max)}
            onChange={(event) => {
              const next = Number(event.target.value);
              if (writable) onChange(String(values.kind === "discrete" ? values.values[next] : next));
            }} />
          <div className="range-bounds" id={`${id}-bounds`}><span>{values.kind === "discrete" ? values.values[0] : min} DPI</span><span>{values.kind === "discrete" ? values.values[values.values.length - 1] : max} DPI</span></div>
        </>
      ) : (
        <div className="invalid-dpi">
          {writable && values.kind === "discrete" ? (
            <select id={id} aria-label={`Choose ${label}`} aria-describedby={`${id}-invalid`} value="" onChange={(event) => onChange(event.target.value)}>
              <option value="" disabled>Choose a valid value</option>
              {values.values.map((dpi) => <option key={dpi} value={dpi}>{dpi} DPI</option>)}
            </select>
          ) : <p className="control-hint">{values.kind === "range" ? `${values.min} - ${values.max} DPI / step ${values.step}` : "No valid position selected."}</p>}
        </div>
      )}</div>{manual}</div>
      {writable && !valid && <p id={`${id}-invalid`} className="control-reason">{draft === "" ? "Empty DPI field." : "Invalid DPI for this mouse."} Enter an allowed integer before applying; values are not rounded.</p>}
      {values.kind === "discrete" && <p className="control-hint">Discrete steps: {values.values.join(" / ")} DPI</p>}
      {explanation && <p id={`${id}-reason`} className="control-reason">{explanation}</p>}
    </div>
  );
}

export function OutputSelector({ outputs, value, onChange, disabled = false, label = "Audio output" }: {
  outputs: AudioNode[]; value: string; onChange: (value: string) => void; disabled?: boolean; label?: string;
}) {
  const id = useId();
  return (
    <div className="select-field">
      <label htmlFor={id}>{label}</label>
      <select id={id} value={value} disabled={disabled || outputs.length === 0} aria-describedby={outputs.length === 0 ? `${id}-empty` : undefined} onChange={(event) => onChange(event.target.value)}>
        {!outputs.some((output) => output.nodeName === value) && <option value="">{outputs.length ? "Choose an output" : "No outputs detected"}</option>}
        {[false, true].map((virtual) => {
          const nodes = outputs.filter((output) => output.isVirtual === virtual);
          return nodes.length > 0 && <optgroup key={String(virtual)} label={virtual ? "Virtual outputs" : "Physical outputs"}>
            {nodes.map((output) => <option key={output.nodeName} value={output.nodeName}>{output.description || output.nodeName}{virtual ? " [virtual]" : " [physical]"}</option>)}
          </optgroup>;
        })}
      </select>
      {outputs.length === 0 && <small className="control-reason" id={`${id}-empty`}>The backend does not expose audio outputs.</small>}
    </div>
  );
}

export function NodeControls({ node, input = false, available, api, pending, runMutation }: MutationControls & {
  node: AudioNode; input?: boolean; available: boolean;
}) {
  const id = useId();
  const [draft, setDraft] = useState(node.volume);
  const prevNodeNameRef = useRef(node.nodeName);
  const prevVolumeRef = useRef(node.volume);

  useEffect(() => {
    const nodeChanged = node.nodeName !== prevNodeNameRef.current;
    if (nodeChanged) {
      prevNodeNameRef.current = node.nodeName;
      prevVolumeRef.current = node.volume;
      setDraft(node.volume);
      return;
    }

    const volumeChanged = node.volume !== prevVolumeRef.current;
    if (volumeChanged) {
      if (draft === prevVolumeRef.current) {
        setDraft(node.volume);
      }
      prevVolumeRef.current = node.volume;
    }
  }, [node.nodeName, node.volume, draft]);
  const busy = hasPending(pending, "audio/");
  const readable = node.volume !== null && Number.isFinite(node.volume) && node.volume >= 0;
  const percent = draft === null ? null : Math.round(draft * 100);
  const actualPercent = node.volume === null ? null : Math.round(node.volume * 100);
  const canApply = readable && draft !== null && draft >= 0 && draft <= 1 && draft !== node.volume;
  const reason = !available ? "Audio service is unavailable." : !readable ? "Volume not readable: node remains read-only." : busy ? "Audio operation in progress." : null;
  const label = input ? "Microphone gain" : "Output volume";
  return (
    <div className="node-controls">
      <div className="field-heading"><label htmlFor={id}>{label}</label><output htmlFor={id}>{percent === null ? "Not available" : `${percent}%`}</output></div>
      <input id={id} className="range-input" type="range" min={0} max={100} step={1}
        value={Math.max(0, Math.min(100, percent ?? 0))} disabled={!available || !readable || busy}
        aria-valuetext={percent === null ? "Volume not available" : `${percent}%`} aria-describedby={`${id}-readback`}
        style={rangeStyle(percent ?? 0, 0, 100)} onChange={(event) => setDraft(Number(event.target.value) / 100)} />
      <div className="range-bounds"><span>0%</span><span>100%</span></div>
      <p className="control-hint" id={`${id}-readback`}>Confirmed reading: {actualPercent === null ? "not available" : `${actualPercent}%`}.{reason ? ` ${reason}` : ""}{actualPercent !== null && actualPercent > 100 ? " Choose a value within 100% before applying." : ""}</p>
      <button className="button button-secondary button-small" disabled={!available || busy || !canApply} onClick={() => {
        if (draft !== null) void runMutation(`audio/volume/${node.nodeName}`, () => api.setVolume(node.nodeName, draft));
      }}>{busy ? "Please wait..." : input ? "Apply gain" : "Apply volume"}</button>
      <Toggle label={input ? "Mute microphone" : "Mute output"} checked={node.muted === true}
        disabled={!available || busy || node.muted === null}
        description={node.muted === null ? "Mute state not readable." : !available ? "Audio service unavailable." : null}
        onChange={(muted) => { void runMutation(`audio/mute/${node.nodeName}`, () => api.setMute(node.nodeName, muted)); }} />
    </div>
  );
}

export function SurroundControl({ audio, output, api, pending, runMutation }: MutationControls & { audio: AudioSnapshot; output: AudioNode | undefined }) {
  const busy = hasPending(pending, "audio/");
  const transitioning = audio.surroundState === "starting" || audio.surroundState === "stopping";
  const on = audio.surroundState === "on";
  const activationReason = filterTargetReason(audio, output) ?? (!audio.hrtf?.readable ? "Surround requires a readable SOFA file."
    : !audio.canEnableSurround ? audio.surroundReason ?? "The backend does not allow activation for this target." : null);
  const disabled = busy || transitioning || (!on && !!activationReason);
  const stateLabels = { off: "Disabled", starting: "Starting...", on: "Active", stopping: "Stopping...", error: "Not active: error" };
  const reason = transitioning || busy ? "Waiting for confirmed reading from backend." : (!on ? activationReason : null) ?? audio.surroundReason;
  return (
    <div className={`surround-control${on ? " surround-on" : ""}`}>
      <div className="surround-heading"><span className="spatial-number" aria-hidden="true">7<span>.1</span></span><span><strong>Binaural Surround</strong><small className={on ? "accent-text" : ""} role="status">{stateLabels[audio.surroundState]}</small></span></div>
      <Toggle label="Surround 7.1" checked={on} disabled={disabled} description={reason || "8-channel input, stereo output with HRTF."}
        onChange={(enabled) => {
          if (disabled) return;
          if (enabled && output) void runMutation("audio/surround", () => api.setSurround(true, output.nodeName));
          if (!enabled) void runMutation("audio/surround", () => api.setSurround(false));
        }} />
    </div>
  );
}

function batteryTime(timestamp: number | null): string {
  if (timestamp === null || !Number.isFinite(timestamp)) return "not available";
  return new Date(timestamp * 1000).toLocaleString("it-IT", { day: "2-digit", month: "2-digit", hour: "2-digit", minute: "2-digit" });
}

export function BatteryReadout({ battery, compact = false }: { battery: Battery | null; compact?: boolean }) {
  const Container = compact ? "span" : "div";
  const Heading = compact ? "span" : "div";

  if (battery === null) {
    return (
      <Container className={`battery-readout${compact ? " compact" : ""}`}>
        <span className="battery-empty">Battery not exposed</span>
        {!compact && <p className="control-hint">No measurement definitively associated with this device.</p>}
      </Container>
    );
  }
  const percentage = battery.percentage !== null && Number.isFinite(battery.percentage) && battery.percentage >= 0 && battery.percentage <= 100 ? battery.percentage : null;
  const levels = { unknown: "Not available", critical: "Critical", low: "Low", normal: "Normal", high: "High", full: "Full" };
  const level = battery.level !== null ? levels[battery.level] : "Not available";
  return (
    <Container className={`battery-readout${compact ? " compact" : ""}`}>
      <Heading className="battery-heading">
        <strong>{percentage !== null ? `${Math.round(percentage)}%` : level}</strong>
        {percentage === null && battery.level && battery.level !== "unknown" && <span className="tiny-label">Approximate level</span>}
        {battery.stale && <span className="status-pill partial">Previous reading</span>}
      </Heading>
      {percentage !== null && !compact && <meter className="battery-meter" min={0} max={100} value={percentage} aria-label="Battery percentage" />}
      {battery.charging === true && <small>Charging</small>}
      <small className="battery-source">Source: {providerNames[battery.source]}{compact ? ` / ${battery.stale ? "previous" : "read"} ${batteryTime(battery.observedAt)}` : ""}</small>
      {!compact && <dl className="metadata-list"><div><dt>Observed</dt><dd>{batteryTime(battery.observedAt)}</dd></div><div><dt>Source updated</dt><dd>{batteryTime(battery.sourceUpdatedAt)}</dd></div></dl>}
    </Container>
  );
}
