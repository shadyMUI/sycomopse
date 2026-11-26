import { useId, useLayoutEffect, useRef, useState } from "react";
import { automaticPreamp, bandFrequencies, equalizerPresets, filterTargetReason, isStereoOutput, isValidManualPreamp, liveEqualizerMutationPrefix, minManualPreampDb } from "../audio";
import { hasPending, type MutationControls } from "../components/Controls";
import type { AudioNode, AudioSnapshot, EqualizerSettings } from "../types";

type Bands = EqualizerSettings["bands"];
type Curve = Pick<EqualizerSettings, "bands" | "preampDb">;
const neutralCurve = { bands: equalizerPresets[0].bands };
const liveIntervalMs = 100;

function sameBands(a: Bands, b: Bands) {
  return a.length === b.length && a.every((band, i) => band.gainDb === b[i].gainDb && band.q === b[i].q);
}

function sameCurve(a: Curve, b: Curve) {
  return (a.preampDb ?? null) === (b.preampDb ?? null) && sameBands(a.bands, b.bands);
}

function decibels(value: number) {
  return `${value > 0 ? "+" : ""}${Number(value.toFixed(2))} dB`;
}

function BandSliders({ bands, field, disabled, onChange }: {
  bands: Bands; field: "gainDb" | "q"; disabled: boolean; onChange: (bands: Bands) => void;
}) {
  const gain = field === "gainDb";
  return (
    <div className="eq-bands" role="group" aria-label={gain ? "Band gains" : "Band Q values"}>
      {bandFrequencies.map((frequency, index) => {
        const value = bands[index][field];
        const text = gain ? decibels(value) : String(value);
        return (
          <label key={frequency} className="eq-band">
            <span>{frequency} Hz</span>
            <input type="range" min={gain ? -12 : 0.4} max={gain ? 12 : 4} step={0.1}
              value={value} aria-label={`${frequency} Hz ${gain ? "gain" : "Q"}`} aria-valuetext={text} disabled={disabled} onChange={(event) => {
                const next = event.currentTarget.valueAsNumber;
                onChange(bands.map((band, i) => i === index ? { ...band, [field]: next } : band));
              }} />
            <span className="eq-value" aria-hidden="true">{text}</span>
          </label>
        );
      })}
    </div>
  );
}

export default function EqualizerPanel({ audio, output, api, pending, runMutation }: MutationControls & {
  audio: AudioSnapshot; output: AudioNode | undefined;
}) {
  const id = useId();
  const [drafts, setDrafts] = useState(() => new Map<string, Curve>());
  const [liveEdit, setLiveEdit] = useState<{ scope: string; deadline: number; curve: Curve } | null>(null);
  const flushLiveEdit = useRef<() => void>(() => {});
  const nodeName = output?.nodeName ?? "";
  const saved = audio.equalizers[nodeName];
  const confirmed: Curve = saved ?? neutralCurve;
  const curve = drafts.get(nodeName) ?? confirmed;
  const { bands, preampDb } = curve;
  const dirty = !sameCurve(curve, confirmed);
  const preset = equalizerPresets.find((entry) => sameBands(entry.bands, bands));
  const active = audio.equalizerOutput === nodeName;
  const bypass = active || saved?.enabled;
  const mutationKey = `audio/equalizer/${nodeName}`;
  const liveKey = `${liveEqualizerMutationPrefix}${nodeName}`;
  const savingLive = pending.has(liveKey);
  const blocked = (hasPending(pending, "audio/") && !savingLive) || audio.surroundState === "starting" || audio.surroundState === "stopping";
  const targetReason = filterTargetReason(audio, output);
  const reason = targetReason ?? (blocked ? "Waiting for the audio change to finish." : null);
  const editingDisabled = blocked || !audio.available;
  const sinkSerial = audio.outputs.find((node) => node.nodeName === audio.defaultOutput)?.objectSerial;
  const liveScope = active && !targetReason && !hasPending(pending, "audio/profile-apply/")
    ? JSON.stringify([nodeName, output?.objectSerial, audio.defaultOutput, sinkSerial, audio.surroundState]) : null;
  const deadline = !blocked && !savingLive && liveEdit?.scope === liveScope ? liveEdit.deadline : null;
  const preampLabel = active ? "Automatic preamp" : "Automatic preamp on apply";
  const currentPreamp = active ? confirmed.preampDb ?? automaticPreamp(confirmed.bands) : 0;

  function clearDraft(expected?: Curve) {
    setDrafts((previous) => {
      const current = previous.get(nodeName);
      if (!current || (expected && current !== expected)) return previous;
      const updated = new Map(previous);
      updated.delete(nodeName);
      return updated;
    });
  }

  async function save(next: Curve, key: string) {
    if (await runMutation(key, () => api.setEqualizer(nodeName, { ...next, enabled: true }))) clearDraft(next);
  }

  // Retain the displayed draft, but don't send it after leaving its confirmed route.
  useLayoutEffect(() => { setLiveEdit(null); }, [api, liveScope]);

  useLayoutEffect(() => {
    flushLiveEdit.current = () => {
      if (deadline === null || !liveEdit) return;
      setLiveEdit(null);
      if (sameCurve(liveEdit.curve, confirmed)) clearDraft(liveEdit.curve);
      else void save(liveEdit.curve, liveKey);
    };
  });

  useLayoutEffect(() => {
    if (deadline === null) return;
    // Keep the first edit's deadline while the sliders replace the queued curve.
    const timer = window.setTimeout(() => flushLiveEdit.current(), Math.max(0, deadline - performance.now()));
    return () => window.clearTimeout(timer);
  }, [api, liveScope, deadline]);

  function edit(next: Curve) {
    if (editingDisabled) return;
    // A value equal to the last readback may still need to undo a write in flight.
    if (!savingLive && sameCurve(next, confirmed)) {
      setLiveEdit(null);
      clearDraft();
      return;
    }
    setDrafts((previous) => {
      const updated = new Map(previous);
      updated.set(nodeName, next);
      return updated;
    });
    if (liveScope !== null) {
      const nextDeadline = performance.now() + liveIntervalMs;
      setLiveEdit((previous) => ({ scope: liveScope, deadline: previous?.scope === liveScope ? previous.deadline : nextDeadline, curve: next }));
    }
  }

  return (
    <section className="panel settings-panel" aria-labelledby={`${id}-heading`}>
      <div className="section-heading"><div><span className="eyebrow">02 / Tone</span><h2 id={`${id}-heading`}>Headphone equalizer</h2></div>
        <span role="status" className={`status-pill ${active ? "available" : "muted"}`}>{active ? "EQ active" : saved?.enabled ? "EQ not active" : saved ? "EQ bypassed" : "EQ off"}</span>
      </div>
      {reason && <p className="control-reason" id={`${id}-reason`}>{reason}</p>}
      {isStereoOutput(output) && <>
        <p className="control-hint eq-target">Ten peaking bands for {output.description || nodeName}. Stereo EQ works without surround or a SOFA file.</p>
        <div className="select-field"><label htmlFor={`${id}-preset`}>EQ preset</label>
          <select id={`${id}-preset`} value={preset?.name ?? "Custom"} disabled={editingDisabled} onChange={(event) => {
            const choice = equalizerPresets.find((entry) => entry.name === event.target.value);
            if (choice) edit({ ...curve, bands: choice.bands });
          }}>
            {equalizerPresets.map(({ name }) => <option key={name}>{name}</option>)}
            <option disabled>Custom</option>
          </select>
        </div>
        <p className="control-hint">Adjust each band from -12 to +12 dB. {active ? "Edits update live on the confirmed output." : "Choose Apply EQ to activate your edits."}</p>
        <BandSliders bands={bands} field="gainDb" disabled={editingDisabled} onChange={(next) => edit({ ...curve, bands: next })} />
        <details className="quick-details"><summary>Advanced: per-band Q</summary>
          <p className="control-hint">Q controls the width of each band. Higher values affect a narrower range (0.4 to 4; default 1.4).</p>
          <BandSliders bands={bands} field="q" disabled={editingDisabled} onChange={(next) => edit({ ...curve, bands: next })} />
        </details>
        <div className="eq-preamp">
          <div className="select-field"><label htmlFor={`${id}-preamp-mode`}>Preamp mode</label>
            <select id={`${id}-preamp-mode`} value={preampDb == null ? "auto" : "manual"} disabled={editingDisabled}
              onChange={(event) => edit({ ...curve, preampDb: event.target.value === "manual" ? 0 : null })}>
              <option value="auto">Auto</option><option value="manual">Manual</option>
            </select>
          </div>
          <div className="field-heading">{preampDb == null ? <span className="field-label">{preampLabel}</span> : <label htmlFor={`${id}-preamp`}>Manual preamp</label>}
            <output aria-label={preampDb == null ? preampLabel : "Manual preamp setting"} aria-live="off">{decibels(preampDb ?? automaticPreamp(bands))}</output>
          </div>
          {preampDb != null && <input id={`${id}-preamp`} type="range" min={minManualPreampDb} max={0} step={0.1} value={preampDb} disabled={editingDisabled}
            aria-valuetext={decibels(preampDb)} aria-describedby={`${id}-clipping`} onChange={(event) => {
              const value = event.currentTarget.valueAsNumber;
              if (isValidManualPreamp(value)) edit({ ...curve, preampDb: value });
            }} />}
          <p className="control-hint">Current preamp: {decibels(currentPreamp)}.</p>
          <p className="control-hint" id={`${id}-clipping`}>{preampDb == null ? "Auto subtracts the sum of boosts; cuts do not offset them." : "Band boosts can clip in Manual. Lower the preamp for more headroom."}</p>
        </div>
        <div className="action-row">
          <button className="button button-primary" disabled={!!reason || savingLive || active} aria-describedby={reason ? `${id}-reason` : undefined} onClick={() => {
            if (!reason && !savingLive && !active) void save(curve, mutationKey);
          }}>Apply EQ</button>
          {saved && <button className="button button-secondary" disabled={!!reason || savingLive} aria-describedby={reason ? `${id}-reason` : undefined} onClick={() => {
            if (reason || savingLive) return;
            setLiveEdit(null);
            void runMutation(mutationKey, () => api.setEqualizer(nodeName, { ...saved, enabled: !bypass }));
          }}>{active ? "Bypass EQ" : bypass ? "Bypass saved EQ" : "Re-enable saved EQ"}</button>}
          {dirty && <button className="text-button" disabled={blocked || savingLive} onClick={() => {
            setLiveEdit(null);
            clearDraft();
          }}>Discard EQ edits</button>}
        </div>
        {!reason && !active && saved?.enabled && <p className="control-hint">The saved EQ is not running. Bypass saved EQ to turn off recall without starting it.</p>}
        {active && (liveEdit || savingLive) ? <p className="control-hint">Updating EQ live...</p> : dirty && <p className="control-hint">Unapplied EQ edits.{active ? " Adjust a control to try again." : saved ? " Bypass and re-enable use the saved EQ, not these edits." : ""}</p>}
      </>}
    </section>
  );
}
