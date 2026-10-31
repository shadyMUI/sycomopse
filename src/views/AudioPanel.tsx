import { useEffect, useState } from "react";
import type { AudioProfile, Device, Snapshot } from "../types";
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
      schemaVersion: 1, id: crypto.randomUUID(), name: profileName.trim(), outputNodeName: profileOutput,
      hrtf: audio.hrtf?.reference ?? null, volume: profileVolume / 100, mute: profileMute, surroundRequested: profileSurround,
    };
    if (await controls.runMutation("audio/profile-save", () => controls.api.saveAudioProfile(profile))) setProfileName("");
  }

  return (
    <>
      <div className="view-heading"><div><span className="eyebrow">Audio / PipeWire</span><h1>Ascolto, sotto controllo.</h1><p>Routing standard e surround software. Nessun effetto proprietario presunto.</p></div><span className={`status-pill ${audio.available ? "available" : "muted"}`}>{audio.available ? "Audio disponibile" : "Audio non disponibile"}</span></div>
      {initialDevice && initialDevice.outputNodeNames.length === 0 && <div className="platform-banner"><div><h2>{initialDevice.name}</h2><p>Dispositivo identificato, ma nessun nodo audio associato con certezza. Le uscite elencate sotto sono i nodi PipeWire, non un'associazione presunta alle cuffie.</p></div></div>}
      <div className="workspace-grid audio-layout">
        <div className="panel-stack">
          <section className="panel settings-panel output-panel"><div className="section-heading"><div><span className="eyebrow">01 / Riproduzione</span><h2>Uscita</h2></div><span className="unit-tag">OUT</span></div>
            <OutputSelector outputs={audio.outputs} value={outputName} onChange={setOutputName} disabled={busy || !audio.available} />
            {output ? <><div className="audio-node-detail"><span className={`status-pill ${output.isVirtual ? "partial" : "available"}`}>{output.isVirtual ? "Nodo virtuale" : "Uscita fisica"}</span><span>{output.channels.length} canali{output.transport ? ` / ${output.transport}` : ""}</span></div><p className="node-name mono">{output.nodeName}</p><button className="button button-primary" disabled={busy || !audio.available || !!isCurrent} onClick={() => { void controls.runMutation("audio/output", () => controls.api.setDefaultOutput(output.nodeName)); }}>{controls.pending.has("audio/output") ? "Applicazione..." : "Applica uscita"}</button><p className="control-hint">La scelta vale per i nuovi flussi collegati automaticamente. I flussi gia fissati non vengono forzati.</p><div className="control-divider" /><NodeControls key={output.nodeName} node={output} available={audio.available} {...controls} /></> : <p className="control-reason">Nessuna uscita esposta dal servizio audio. Nessun modello di cuffia viene dedotto da un nodo analogico.</p>}
            <div className="routing-readback"><span className="tiny-label">Routing confermato</span><strong>{activeOutput?.description ?? audio.defaultOutput ?? "Nessuna uscita predefinita"}</strong>{audio.defaultOutput && <code>{audio.defaultOutput}</code>}</div>
          </section>
          <section className="panel settings-panel"><div className="section-heading"><div><span className="eyebrow">02 / Acquisizione</span><h2>Microfono</h2></div><span className="unit-tag">IN</span></div>
            {audio.inputs.length > 0 ? <><div className="select-field"><label htmlFor="audio-input">Ingresso microfono</label><select id="audio-input" value={inputName} disabled={busy || !audio.available} onChange={(event) => setInputName(event.target.value)}>{audio.inputs.map((node) => <option key={node.nodeName} value={node.nodeName}>{node.description || node.nodeName}{node.isVirtual ? " [virtuale]" : " [fisico]"}</option>)}</select></div>{input && <><p className="node-name mono">{input.nodeName}</p><NodeControls key={input.nodeName} node={input} input available={audio.available} {...controls} /></>}</> : <p className="control-reason">Nessun ingresso microfono esposto. Guadagno e mute non disponibili.</p>}
          </section>
        </div>
        <aside className="panel-stack">
          {headset && <section className="panel headset-support"><div className="section-heading"><h2>{headset.name}</h2><span className={`status-pill ${deviceStatus(headset).tone}`}>{deviceStatus(headset).label}</span></div><BatteryReadout battery={headset.values.battery} />{headset.limitations.length > 0 && <ul className="plain-list">{headset.limitations.map((limitation, index) => <li key={index}>{limitation}</li>)}</ul>}</section>}
          <section className="panel spatial-panel"><span className="eyebrow">03 / Spazio sonoro</span><div className={`spatial-visual${audio.surroundState === "on" ? " active" : ""}`} aria-hidden="true"><div className="spatial-orbit orbit-outer" /><div className="spatial-orbit orbit-inner" />{["FL", "FR", "FC", "LFE", "RL", "RR", "SL", "SR"].map((channel, index) => <span className={`channel-dot channel-${index}`} key={channel}>{channel}</span>)}<DeviceIllustration kind="headset" /></div><SurroundControl audio={audio} {...controls} /><p className="control-hint">Otto canali in ingresso, HRTF, due canali in uscita. Richiede un target fisico stereo; non cambia il profilo Bluetooth.</p><div className="routing-readback"><span className="tiny-label">Target fisico</span><strong>{selectedPhysical?.description ?? "Non disponibile"}</strong>{selectedPhysical && <code>{selectedPhysical.nodeName}</code>}</div>
            <div className="control-divider" /><div className="section-heading"><h2>HRTF / SOFA</h2><span className="tiny-label">Filtro</span></div><div className="hrtf-file"><span className="file-mark" aria-hidden="true">[~]</span><div><strong>{audio.hrtf?.displayName ?? "HRTF non selezionata"}</strong><small>{audio.hrtf ? audio.hrtf.readable ? "File leggibile" : "File non leggibile" : "Seleziona un file SOFA dal dialogo locale."}</small></div></div>{audio.hrtf && <p className="node-name mono">{audio.hrtf.reference}</p>}<button className="button button-secondary button-small" aria-label="Importa HRTF SOFA" disabled={busy || !desktop} onClick={() => { void controls.runMutation("audio/hrtf", () => controls.api.importHrtf()); }}>Importa SOFA <span aria-hidden="true">+</span></button><p className="control-hint">Il backend apre il dialogo e valida il file. Un'importazione valida disattiva il filtro attivo; la riattivazione resta esplicita. Nessuna configurazione PipeWire arbitraria.</p>{snapshot.demo && <p className="demo-explanation">Nella demo l'importazione e il filtro sono simulati: nessun file letto e nessun audio elaborato.</p>}
          </section>
          <section className="panel profiles-panel"><div className="section-heading"><div><span className="eyebrow">04 / Configurazioni</span><h2>Profili acustici</h2></div><span className="count-label">{snapshot.profiles.length}</span></div><p className="control-hint">Stato desiderato, non attivo. I profili non vengono applicati all'avvio. Con 7.1, volume e mute del profilo agiscono sul nodo virtuale senza aumentare il volume fisico.</p>
            {snapshot.profiles.length === 0 ? <p className="control-reason">Nessun profilo salvato.</p> : <div className="profile-list">{snapshot.profiles.map((profile) => {
              const target = physicalOutputs.find((node) => node.nodeName === profile.outputNodeName);
              return <article className="profile-card" key={profile.id}><div><h3>{profile.name}</h3><p>Desiderato: {Math.round(profile.volume * 100)}% / {profile.mute ? "mute" : "non silenziato"} / {profile.surroundRequested ? "7.1" : "stereo"}</p><code>{profile.outputNodeName}</code>{!target && <small className="control-reason">Uscita fisica non disponibile. Il profilo non puo essere applicato.</small>}</div><button className="button button-secondary button-small" aria-label={`Applica profilo ${profile.name}`} disabled={busy || !audio.available || !target} onClick={() => { void controls.runMutation(`audio/profile-apply/${profile.id}`, () => controls.api.applyAudioProfile(profile.id)); }}>Applica</button></article>;
            })}</div>}
            <details className="profile-editor"><summary>Nuovo profilo <span aria-hidden="true">+</span></summary><form onSubmit={(event) => { event.preventDefault(); void saveProfile(); }}><div className="select-field"><label htmlFor="profile-name">Nome del profilo</label><input id="profile-name" maxLength={80} value={profileName} disabled={busy || !desktop} placeholder="Es. Ascolto serale" onChange={(event) => setProfileName(event.target.value)} /></div><OutputSelector label="Uscita desiderata del profilo" outputs={physicalOutputs} value={profileOutput} onChange={setProfileOutput} disabled={busy || !desktop} /><div className="profile-volume"><label htmlFor="profile-volume">Volume desiderato <strong>{profileVolume}%</strong></label><input id="profile-volume" className="range-input" type="range" min={0} max={100} step={1} value={profileVolume} disabled={busy || !desktop} onChange={(event) => setProfileVolume(Number(event.target.value))} /></div><Toggle label="Mute desiderato nel profilo" checked={profileMute} disabled={busy || !desktop} onChange={setProfileMute} /><Toggle label="Surround desiderato nel profilo" checked={profileSurround} disabled={busy || !desktop} description="Salvare non attiva il surround. Serve Applica sul profilo." onChange={setProfileSurround} />{profileSurround && (!audio.hrtf?.readable || desiredOutput?.channels.length !== 2) && <p className="control-reason">Il profilo 7.1 richiede una HRTF leggibile e un target stereo.</p>}<button type="submit" className="button button-primary" disabled={busy || !canSave}>Salva profilo</button></form></details>
          </section>
        </aside>
      </div>
    </>
  );
}
