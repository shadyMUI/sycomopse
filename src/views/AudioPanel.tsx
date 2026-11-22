import { useEffect, useState } from "react";
import type { AudioProfile, Device, Snapshot } from "../types";
import { generateUuid } from "../api";
import { BatteryReadout, NodeControls, OutputSelector, SurroundControl, Toggle, deviceStatus, hasPending } from "../components/Controls";
import type { MutationControls } from "../components/Controls";
import { DeviceIllustration } from "../components/DeviceCard";

export default function AudioPanel({ snapshot, initialOutput = null, initialDevice = null, ...controls }: MutationControls & { snapshot: Snapshot; initialOutput?: string | null; initialDevice?: Device | null }) {
  const audio = snapshot.audio;
  const [outputName, setOutputName] = useState(initialOutput ?? audio.selectedOutput ?? audio.defaultOutput ?? audio.outputs[0]?.nodeName ?? "");
  const [inputName, setInputName] = useState(audio.defaultInput ?? audio.inputs[0]?.nodeName ?? "");
  const [profileName, setProfileName] = useState("");
  const [profileOutput, setProfileOutput] = useState(audio.selectedOutput ?? audio.outputs.find((node) => !node.isVirtual)?.nodeName ?? "");
  const initialNode = audio.outputs.find((node) => node.nodeName === profileOutput);
  const [profileVolume, setProfileVolume] = useState(Math.round(Math.max(0, Math.min(1, initialNode?.volume ?? 0.5)) * 100));
  const [profileMute, setProfileMute] = useState(false);
  const [profileSurround, setProfileSurround] = useState(false);
  const busy = hasPending(controls.pending, "audio/");
  useEffect(() => {
    if (initialOutput) {
      setOutputName(initialOutput);
    }
  }, [initialOutput]);
  useEffect(() => {
    setOutputName((current) => audio.outputs.some((node) => node.nodeName === current) ? current : audio.selectedOutput ?? audio.defaultOutput ?? audio.outputs[0]?.nodeName ?? "");
    setProfileOutput((current) => audio.outputs.some((node) => node.nodeName === current && !node.isVirtual) ? current : audio.outputs.find((node) => !node.isVirtual)?.nodeName ?? "");
  }, [audio.outputs, audio.selectedOutput, audio.defaultOutput]);
  useEffect(() => { setInputName((current) => audio.inputs.some((node) => node.nodeName === current) ? current : audio.defaultInput ?? audio.inputs[0]?.nodeName ?? ""); }, [audio.inputs, audio.defaultInput]);
  const output = audio.outputs.find((node) => node.nodeName === outputName);
  const associatedDevices = snapshot.devices.filter((device) => device.kind === "headset" && device.outputNodeNames.includes(outputName));
  const headset = associatedDevices.length === 1 ? associatedDevices[0] : null;
  const input = audio.inputs.find((node) => node.nodeName === inputName);
  const physicalOutputs = audio.outputs.filter((node) => !node.isVirtual);
  const desiredOutput = physicalOutputs.find((node) => node.nodeName === profileOutput);
  const activeOutput = audio.outputs.find((node) => node.nodeName === audio.defaultOutput);
  const selectedPhysical = audio.outputs.find((node) => node.nodeName === audio.selectedOutput && !node.isVirtual);
  const isCurrent = output && output.nodeName === (output.isVirtual ? audio.defaultOutput : audio.selectedOutput);
  const desktop = snapshot.platform === "linux" || snapshot.demo;
  const canSave = desktop && !!profileName.trim() && !!desiredOutput && (!profileSurround || (audio.hrtf?.readable && desiredOutput.channels.length === 2));

  async function saveProfile() {
    if (!canSave) return;
    const profile: AudioProfile = {
      schemaVersion: 1, id: generateUuid(), name: profileName.trim(), outputNodeName: profileOutput,
      hrtf: audio.hrtf?.reference ?? null, volume: profileVolume / 100, mute: profileMute, surroundRequested: profileSurround,
    };
    if (await controls.runMutation("audio/profile-save", () => controls.api.saveAudioProfile(profile))) setProfileName("");
  }

  return (
    <>
      <div className="view-heading"><div><span className="eyebrow">Audio / PipeWire</span><h1>Listening, under control.</h1><p>Standard routing and software surround. No proprietary effects assumed.</p></div><span className={`status-pill ${audio.available ? "available" : "muted"}`}>{audio.available ? "Audio available" : "Audio unavailable"}</span></div>
      {initialDevice && initialDevice.outputNodeNames.length === 0 && <div className="platform-banner"><div><h2>{initialDevice.name}</h2><p>Device identified, but no audio node definitively associated. Outputs listed below are PipeWire nodes, not an assumed binding to the headset.</p></div></div>}
      <div className="workspace-grid audio-layout">
        <div className="panel-stack">
          <section className="panel settings-panel output-panel"><div className="section-heading"><div><span className="eyebrow">01 / Playback</span><h2>Output</h2></div><span className="unit-tag">OUT</span></div>
            <OutputSelector outputs={audio.outputs} value={outputName} onChange={setOutputName} disabled={busy || !audio.available} />
            {output ? <><div className="audio-node-detail"><span className={`status-pill ${output.isVirtual ? "partial" : "available"}`}>{output.isVirtual ? "Virtual node" : "Physical output"}</span><span>{output.channels.length} channels{output.transport ? ` / ${output.transport}` : ""}</span></div><p className="node-name mono">{output.nodeName}</p><button className="button button-primary" disabled={busy || !audio.available || !!isCurrent} onClick={() => { void controls.runMutation("audio/output", () => controls.api.setDefaultOutput(output.nodeName)); }}>{controls.pending.has("audio/output") ? "Applying..." : "Apply output"}</button><p className="control-hint">Selection applies to new streams connected automatically. Already pinned streams are not forced.</p><div className="control-divider" /><NodeControls key={output.nodeName} node={output} available={audio.available} {...controls} /></> : <p className="control-reason">No output exposed by audio service. Headset model is not inferred from an analog node.</p>}
            <div className="routing-readback"><span className="tiny-label">Confirmed routing</span><strong>{activeOutput?.description ?? audio.defaultOutput ?? "No default output"}</strong>{audio.defaultOutput && <code>{audio.defaultOutput}</code>}</div>
          </section>
          <section className="panel settings-panel"><div className="section-heading"><div><span className="eyebrow">02 / Recording</span><h2>Microphone</h2></div><span className="unit-tag">IN</span></div>
            {audio.inputs.length > 0 ? <><div className="select-field"><label htmlFor="audio-input">Microphone input</label><select id="audio-input" value={inputName} disabled={busy || !audio.available} onChange={(event) => setInputName(event.target.value)}>{audio.inputs.map((node) => <option key={node.nodeName} value={node.nodeName}>{node.description || node.nodeName}{node.isVirtual ? " [virtual]" : " [physical]"}</option>)}</select></div>{input && <><p className="node-name mono">{input.nodeName}</p><NodeControls key={input.nodeName} node={input} input available={audio.available} {...controls} /></>}</> : <p className="control-reason">No microphone input exposed. Gain and mute unavailable.</p>}
          </section>
        </div>
        <aside className="panel-stack">
          {headset && <section className="panel headset-support"><div className="section-heading"><h2>{headset.name}</h2><span className={`status-pill ${deviceStatus(headset).tone}`}>{deviceStatus(headset).label}</span></div><BatteryReadout battery={headset.values.battery} />{headset.limitations.length > 0 && <ul className="plain-list">{headset.limitations.map((limitation, index) => <li key={`${headset.id}-limit-${index}`}>{limitation}</li>)}</ul>}</section>}
          <section className="panel spatial-panel"><span className="eyebrow">03 / Soundstage</span><div className={`spatial-visual${audio.surroundState === "on" ? " active" : ""}`} aria-hidden="true"><div className="spatial-orbit orbit-outer" /><div className="spatial-orbit orbit-inner" />{["FL", "FR", "FC", "LFE", "RL", "RR", "SL", "SR"].map((channel, index) => <span className={`channel-dot channel-${index}`} key={channel}>{channel}</span>)}<DeviceIllustration kind="headset" /></div><SurroundControl audio={audio} {...controls} /><p className="control-hint">Eight input channels, HRTF, two output channels. Requires a stereo physical target; does not alter Bluetooth profile.</p><div className="routing-readback"><span className="tiny-label">Physical target</span><strong>{selectedPhysical?.description ?? "Not available"}</strong>{selectedPhysical && <code>{selectedPhysical.nodeName}</code>}</div>
            <div className="control-divider" /><div className="section-heading"><h2>HRTF / SOFA</h2><span className="tiny-label">Filter</span></div><div className="hrtf-file"><span className="file-mark" aria-hidden="true">[~]</span><div><strong>{audio.hrtf?.displayName ?? "No HRTF selected"}</strong><small>{audio.hrtf ? audio.hrtf.readable ? "Readable file" : "Unreadable file" : "Select a SOFA file from local dialog."}</small></div></div>{audio.hrtf && <p className="node-name mono">{audio.hrtf.reference}</p>}<button className="button button-secondary button-small" aria-label="Import SOFA HRTF" disabled={busy || !desktop} onClick={() => { void controls.runMutation("audio/hrtf", () => controls.api.importHrtf()); }}>Import SOFA <span aria-hidden="true">+</span></button><p className="control-hint">The backend opens the dialog and validates the file. A valid import deactivates active filter; reactivation remains explicit. No arbitrary PipeWire configuration.</p>{snapshot.demo && <p className="demo-explanation">In demo mode, import and filter are simulated: no file is read or audio processed.</p>}
          </section>
          <section className="panel profiles-panel"><div className="section-heading"><div><span className="eyebrow">04 / Presets</span><h2>Audio profiles</h2></div><span className="count-label">{snapshot.profiles.length}</span></div><p className="control-hint">Desired state, not active. Profiles are not applied on startup. With 7.1, profile volume and mute affect the virtual node without boosting physical volume.</p>
            {snapshot.profiles.length === 0 ? <p className="control-reason">No saved profiles.</p> : <div className="profile-list">{snapshot.profiles.map((profile) => {
              const target = physicalOutputs.find((node) => node.nodeName === profile.outputNodeName);
              return <article className="profile-card" key={profile.id}><div><h3>{profile.name}</h3><p>Target: {Math.round(profile.volume * 100)}% / {profile.mute ? "muted" : "unmuted"} / {profile.surroundRequested ? "7.1" : "stereo"}</p><code>{profile.outputNodeName}</code>{!target && <small className="control-reason">Physical output unavailable. Profile cannot be applied.</small>}</div><button className="button button-secondary button-small" aria-label={`Apply profile ${profile.name}`} disabled={busy || !audio.available || !target} onClick={() => { void controls.runMutation(`audio/profile-apply/${profile.id}`, () => controls.api.applyAudioProfile(profile.id)); }}>Apply</button></article>;
            })}</div>}
            <details className="profile-editor"><summary>New profile <span aria-hidden="true">+</span></summary><form onSubmit={(event) => { event.preventDefault(); void saveProfile(); }}><div className="select-field"><label htmlFor="profile-name">Profile name</label><input id="profile-name" maxLength={80} value={profileName} disabled={busy || !desktop} placeholder="e.g. Evening listening" onChange={(event) => setProfileName(event.target.value)} /></div><OutputSelector label="Desired profile output" outputs={physicalOutputs} value={profileOutput} onChange={setProfileOutput} disabled={busy || !desktop} /><div className="profile-volume"><label htmlFor="profile-volume">Desired volume <strong>{profileVolume}%</strong></label><input id="profile-volume" className="range-input" type="range" min={0} max={100} step={1} value={profileVolume} disabled={busy || !desktop} onChange={(event) => setProfileVolume(Number(event.target.value))} /></div><Toggle label="Desired mute in profile" checked={profileMute} disabled={busy || !desktop} onChange={setProfileMute} /><Toggle label="Desired surround in profile" checked={profileSurround} disabled={busy || !desktop} description="Saving does not enable surround. Use Apply on the profile." onChange={setProfileSurround} />{profileSurround && (!audio.hrtf?.readable || desiredOutput?.channels.length !== 2) && <p className="control-reason">7.1 profile requires a readable HRTF and a stereo target.</p>}<button type="submit" className="button button-primary" disabled={busy || !canSave}>Save profile</button></form></details>
          </section>
        </aside>
      </div>
    </>
  );
}
