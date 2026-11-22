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
  const availabilityReason = !hardwareAvailable ? `${deviceStatus(device).label}. Controls remain read-only.` : null;
  const x = parseDpiDraft(draftDpi.x);
  const y = parseDpiDraft(draftDpi.y);
  const validDpi = dpi && isDpiValueAllowed(dpi.values, x) && isDpiValueAllowed(dpi.values, y) && (dpi.separateAxes || x === y);
  const dpiChanged = x !== device.values.dpi?.x || y !== device.values.dpi?.y;
  const dpiDirty = draftDpi.x !== (device.values.dpi?.x.toString() ?? "") || draftDpi.y !== (device.values.dpi?.y.toString() ?? "");
  const rateChanged = draftRate !== device.values.pollingRate;
  const validRate = draftRate !== null && polling?.values.includes(draftRate);
  const dpiReason = busy ? "Mouse write in progress. Audio remains independent." : availabilityReason;
  const confirmed = device.values.dpi;

  function changeDpi(axis: "x" | "y", value: string) {
    if (linked || !dpi?.separateAxes || compact) setDraftDpi({ x: value, y: value });
    else setDraftDpi((current) => ({ ...current, [axis]: value }));
  }

  const pollingControls = (
    <>
      {!compact && <div className="section-heading"><div><span className="eyebrow">02 / Communication</span><h2>Polling</h2></div><span className="unit-tag">Hz</span></div>}
      <p className="readback-line">Confirmed reading: <strong>{device.values.pollingRate === null ? "not available" : `${device.values.pollingRate} Hz`}</strong></p>
      {polling && polling.values.length > 0 ? (
        <div className="rate-options" role="group" aria-label="Target polling rate">
          {polling.values.map((rate) => <button key={rate} type="button" aria-pressed={draftRate === rate} disabled={busy || !canWritePolling} onClick={() => setDraftRate(rate)}>{rate}<small>Hz</small></button>)}
        </div>
      ) : <p className="control-reason">No list of supported frequencies. No universal range is assumed.</p>}
      {device.values.pollingRate !== null && polling && !polling.values.includes(device.values.pollingRate) && <p className="control-reason">Current frequency is not in provider list. Choose a valid frequency before applying.</p>}
      {(availabilityReason || polling?.reason || !polling?.writable) && <p className="control-reason">{availabilityReason || polling?.reason || "Read-only polling."}</p>}
      <button className="button button-secondary button-small" disabled={busy || !canWritePolling || !validRate || !rateChanged}
        onClick={() => { if (draftRate !== null) void runMutation(`mouse/${device.id}/polling`, () => api.setPollingRate(device.id, draftRate)); }}>
        {pending.has(`mouse/${device.id}/polling`) ? "Applying polling..." : "Apply polling"}
      </button>
    </>
  );

  return (
    <div className={`mouse-controls${compact ? " compact-controls" : ""}`}>
      {!compact && <div className="section-heading"><div><span className="eyebrow">01 / Sensor</span><h2>Sensitivity</h2></div><span className="unit-tag">DPI</span></div>}
      <p className="readback-line">Confirmed reading: <strong>{confirmed ? confirmed.x === confirmed.y ? `${confirmed.x} DPI` : `X ${confirmed.x} / Y ${confirmed.y} DPI` : "not available"}</strong></p>
      {!compact && <p className="control-hint">Hardware DPI, not system pointer speed. Changes remain in draft until Applied.</p>}
      {dpi?.separateAxes && !compact ? (
        <Toggle label="Link X/Y axes" checked={linked} disabled={busy || !canWriteDpi}
          description={linked ? "Applying changes sets the same DPI on both axes." : "Provider supports independent settings."}
          onChange={(checked) => {
            setLinked(checked);
            if (checked) setDraftDpi({ x: draftDpi.x, y: draftDpi.x });
          }} />
      ) : dpi && <p className="axis-note">{dpi.separateAxes ? "Quick control with linked axes." : "Axes linked by provider."}</p>}
      <DpiControl capability={dpi} label={!linked && !compact ? "Target X DPI" : "Target DPI"} manualLabel={!linked && !compact ? "Manual X DPI" : "Manual DPI"} draft={draftDpi.x}
        disabled={busy || !hardwareAvailable} reason={dpiReason} onChange={(value) => changeDpi("x", value)} />
      {!linked && dpi?.separateAxes && !compact && <DpiControl capability={dpi} label="Target Y DPI" manualLabel="Manual Y DPI" draft={draftDpi.y}
        disabled={busy || !hardwareAvailable} reason={dpiReason} onChange={(value) => changeDpi("y", value)} />}
      <div className="action-row">
        <button className={`button ${compact ? "button-secondary button-small" : "button-primary"}`} disabled={busy || !canWriteDpi || !validDpi || !dpiChanged}
          onClick={() => { if (validDpi && x !== null && y !== null) void runMutation(`mouse/${device.id}/dpi`, () => api.setDpi(device.id, x, y)); }}>
          {pending.has(`mouse/${device.id}/dpi`) ? "Applying DPI..." : "Apply DPI"}
        </button>
        <button className="text-button" disabled={busy || !dpiDirty} onClick={() => setDraftDpi(dpiDraft(device))}>Reset draft</button>
      </div>
      {compact ? <details className="quick-details"><summary>Polling <span>{device.values.pollingRate === null ? "Not read" : `${device.values.pollingRate} Hz`}</span></summary><div className="quick-details-content">{pollingControls}</div></details> : <><div className="control-divider" />{pollingControls}</>}
    </div>
  );
}

export default function MousePanel({ devices, selectedId, onSelect, ...controls }: MutationControls & {
  devices: Device[]; selectedId: string | null; onSelect: (id: string) => void;
}) {
  const mice = devices.filter((device) => device.kind === "mouse");
  const device = mice.find((mouse) => mouse.id === selectedId) ?? mice[0];
  if (!device) return <section className="panel empty-state large-empty"><DeviceIllustration kind="mouse" /><span className="eyebrow">Hardware control</span><h1>No mouse detected</h1><p>Connect a supported mouse and refresh devices. Controls depend on provider capabilities, not brand.</p></section>;
  const status = deviceStatus(device);
  const vidPid = device.identity.vendorId !== null && device.identity.productId !== null ? `${device.identity.vendorId.toString(16).padStart(4, "0")}:${device.identity.productId.toString(16).padStart(4, "0")}` : "Not exposed";
  return (
    <>
      <div className="view-heading"><div><span className="eyebrow">Mouse / {providerNames[device.provider]}</span><h1>{device.name}</h1><p>Only controls officially exposed by the selected provider.</p></div>
        {mice.length > 1 && <div className="select-field device-picker"><label htmlFor="mouse-device">Selected mouse</label><select id="mouse-device" value={device.id} onChange={(event) => onSelect(event.target.value)}>{mice.map((mouse) => <option key={mouse.id} value={mouse.id}>{mouse.name}</option>)}</select></div>}
      </div>
      <div className="workspace-grid mouse-layout">
        <div className="panel-stack"><section className="panel settings-panel"><MouseControls key={device.id} device={device} {...controls} /></section><MouseButtons key={`${device.id}/${device.capabilities.remapping?.fingerprint ?? "unknown"}`} device={device} {...controls} /></div>
        <aside className="panel-stack">
          <section className="panel specimen-panel"><div className="panel-topline"><span className="eyebrow">Selected device</span><span className={`status-pill ${status.tone}`}>{status.label}</span></div><DeviceIllustration kind="mouse" className="large-illustration" /><dl className="metadata-list"><div><dt>Provider</dt><dd>{providerNames[device.provider]}</dd></div><div><dt>Transport</dt><dd>{device.identity.transport ?? "Not exposed"}</dd></div><div><dt>VID:PID</dt><dd className="mono">{vidPid}</dd></div><div><dt>Firmware</dt><dd>{device.identity.firmware ?? "Not exposed"}</dd></div><div><dt>Serial</dt><dd className="mono">{device.identity.serial ?? "Not exposed"}</dd></div></dl></section>
          <section className="panel battery-panel"><div className="section-heading"><h2>Power</h2><span className="tiny-label">Read-only</span></div><BatteryReadout battery={device.values.battery} /></section>
          {device.limitations.length > 0 && <section className="panel limits-panel"><h2>Support limitations</h2><ul className="plain-list">{device.limitations.map((limitation, index) => <li key={`${device.id}-limit-${index}`}>{limitation}</li>)}</ul></section>}
        </aside>
      </div>
    </>
  );
}
