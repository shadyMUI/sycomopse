import { memo, useEffect, useState } from "react";
import type { Device, Snapshot } from "../types";
import { DeviceCard, DeviceIllustration } from "../components/DeviceCard";
import { NodeControls, OutputSelector, SurroundControl, hasPending } from "../components/Controls";
import type { MutationControls } from "../components/Controls";
import { MouseControls } from "./MousePanel";

function QuickAudio({ snapshot, ...controls }: MutationControls & { snapshot: Snapshot }) {
  const audio = snapshot.audio;
  const initial = audio.selectedOutput ?? audio.defaultOutput ?? audio.outputs[0]?.nodeName ?? "";
  const [outputName, setOutputName] = useState(initial);
  useEffect(() => {
    setOutputName((current) => audio.outputs.some((node) => node.nodeName === current) ? current : audio.selectedOutput ?? audio.defaultOutput ?? audio.outputs[0]?.nodeName ?? "");
  }, [audio.outputs, audio.selectedOutput, audio.defaultOutput]);
  const output = audio.outputs.find((node) => node.nodeName === outputName);
  const busy = hasPending(controls.pending, "audio/");
  const isCurrent = output && output.nodeName === (output.isVirtual ? audio.defaultOutput : audio.selectedOutput);
  return (
    <section className="panel quick-panel">
      <div className="section-heading"><div><span className="eyebrow">Quick controls</span><h2>Audio</h2></div><span className="small-glyph" aria-hidden="true">))</span></div>
      <SurroundControl audio={audio} {...controls} />
      <div className="control-divider" />
      <OutputSelector outputs={audio.outputs} value={outputName} onChange={setOutputName} disabled={busy || !audio.available} />
      {output && <><p className="node-name mono">{output.nodeName}</p><button className="button button-secondary button-small" disabled={busy || !audio.available || !!isCurrent} onClick={() => { void controls.runMutation("audio/output", () => controls.api.setDefaultOutput(output.nodeName)); }}>Apply output</button><details className="quick-details"><summary>Volume & mute <span>{output.volume === null ? "Not read" : `${Math.round(output.volume * 100)}%`} / {output.muted === null ? "Mute not read" : output.muted ? "Muted" : "Unmuted"}</span></summary><NodeControls key={output.nodeName} node={output} available={audio.available} {...controls} /></details></>}
    </section>
  );
}

const Dashboard = memo(function Dashboard({ snapshot, selectedMouseId, onSelectMouse, onOpenDevice, ...controls }: MutationControls & {
  snapshot: Snapshot; selectedMouseId: string | null; onSelectMouse: (id: string) => void; onOpenDevice: (device: Device) => void;
}) {
  const mice = snapshot.devices.filter((device) => device.kind === "mouse");
  const mouse = mice.find((device) => device.id === selectedMouseId) ?? mice[0];
  const physicalOutputs = snapshot.audio.outputs.filter((output) => !output.isVirtual).length;
  return (
    <>
      <div className="workspace-grid dashboard-layout">
        <div className="panel-stack">
      <section className="overview-hero">
        <div><span className="eyebrow">Your setup, clearly</span><h1>Control.<br /><span>Without the bloat.</span></h1><p>Peripherals and audio in one place.<br />Local. Essential. Unofficial.</p></div>
        <div className="hero-instrument" aria-hidden="true"><div className="instrument-ring"><span>PH</span><i /><i /><i /><i /></div><span className="instrument-caption">Razer / Logitech / PipeWire</span></div>
        <div className="hero-metrics"><div><strong>{String(snapshot.devices.length).padStart(2, "0")}</strong><span>Detected devices</span></div><div><strong>{String(physicalOutputs).padStart(2, "0")}</strong><span>Physical outputs</span></div><div><strong className={snapshot.audio.surroundState === "on" ? "accent-text" : ""}>{snapshot.audio.surroundState === "on" ? "7.1" : "OFF"}</strong><span>Binaural surround</span></div></div>
      </section>
          <section className="panel device-section">
            <div className="section-heading"><div><span className="eyebrow">01 / Devices</span><h2>Your setup <span className="count-label">{snapshot.devices.length}</span></h2></div><button className="text-button refresh-button" disabled={controls.pending.has("discovery")} onClick={() => { void controls.runMutation("discovery", () => controls.api.refreshDevices()); }}>{controls.pending.has("discovery") ? "Searching..." : "Refresh"}<span aria-hidden="true"> +</span></button></div>
            {snapshot.devices.length > 0 ? <div className="devices-grid">{snapshot.devices.map((device) => <DeviceCard key={device.id} device={device} selected={device.id === mouse?.id} onOpen={onOpenDevice} />)}</div> : <div className="empty-state"><DeviceIllustration kind="unknown" /><h3>No devices detected</h3><p>{snapshot.demo ? "Simulated scenario without peripherals. Sample audio remains available; no real hardware is polled." : "Connect a peripheral and refresh. Missing services do not block other providers."}</p></div>}
            <p className="section-footnote">Support verified by capabilities, not brand promises.</p>
          </section>
          <section className="local-note"><span className="note-mark" aria-hidden="true">[ / ]</span><div><h2>Your hardware. Your data.</h2><p>No accounts, telemetry, or remote services. Unexposed features remain visible as limits, not fake controls.</p></div></section>
        </div>
        <aside className="panel-stack">
          <section className="panel quick-panel"><div className="section-heading"><div><span className="eyebrow">Quick controls</span><h2>Mouse</h2></div><span className="small-glyph" aria-hidden="true">[ ]</span></div>
            {mouse ? <><div className="select-field"><label htmlFor="quick-mouse">Quick controls mouse</label><select id="quick-mouse" value={mouse.id} onChange={(event) => onSelectMouse(event.target.value)}>{mice.map((device) => <option key={device.id} value={device.id}>{device.name}</option>)}</select></div><MouseControls key={mouse.id} device={mouse} compact {...controls} /></> : <p className="control-reason">No mouse available. DPI and polling values are not assumed.</p>}
          </section>
          <QuickAudio snapshot={snapshot} {...controls} />
        </aside>
      </div>
    </>
  );
});

export default Dashboard;
