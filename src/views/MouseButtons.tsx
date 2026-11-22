import { useEffect, useId, useMemo, useState } from "react";
import type { KeyboardEvent } from "react";
import type { Device, MouseAction, MouseBinding, RemappingValues } from "../types";
import type { MutationControls } from "../components/Controls";
import { Toggle, availabilityNames, hasPending, providerNames } from "../components/Controls";
import { isModifierCode, isMouseButtonCode, keyboardCodes, keyCodeLabel, mediaKeyNames, modifierGroups, mouseActionLabel, remappingProblem } from "../remapping";

type ActionKind = MouseAction["kind"] | "media";
interface BindingDraft {
  buttonId: string;
  kind: ActionKind;
  code: number | null;
  modifiers: number[];
  cps?: number;
  autoclickButton?: number;
}

function savedDraft(device: Device) {
  return {
    autoApply: device.values.remapping?.autoApply ?? false,
    bindings: (device.capabilities.remapping?.buttons ?? []).map((button): BindingDraft => {
      const action = device.values.remapping?.bindings.find((binding) => binding.buttonId === button.id)?.action ?? { kind: "default" };
      return {
        buttonId: button.id,
        kind: action.kind === "key" && mediaKeyNames[action.code] ? "media" : action.kind,
        code: "code" in action ? action.code : action.kind === "chord" ? action.key : null,
        modifiers: action.kind === "chord" ? [...action.modifiers] : [],
        cps: action.kind === "autoclicker" ? action.cps : 10,
        autoclickButton: action.kind === "autoclicker" ? (action.button ?? 272) : 272,
      };
    }),
  };
}

function compileBindings(drafts: BindingDraft[]): MouseBinding[] {
  return drafts.filter((draft) => draft.kind !== "default").map((draft) => {
    let action: MouseAction;
    switch (draft.kind) {
      case "default": case "disabled": action = { kind: draft.kind }; break;
      case "chord": action = { kind: "chord", modifiers: [...draft.modifiers], key: draft.code ?? 0 }; break;
      case "media": case "key": action = { kind: "key", code: draft.code ?? 0 }; break;
      case "mouseButton": action = { kind: "mouseButton", code: draft.code ?? 0 }; break;
      case "autoclicker":
        action = {
          kind: "autoclicker",
          cps: Math.max(1, Math.min(50, Math.round(draft.cps ?? 10))),
          button: draft.autoclickButton ?? 272,
        };
        break;
    }
    return { buttonId: draft.buttonId, action };
  });
}

const stateNames: Record<RemappingValues["state"], string> = {
  unavailable: "Unavailable", inactive: "Inactive", starting: "Starting...", active: "Active",
  busy: "Busy", error: "Error", paused: "Paused",
};

export default function MouseButtons({ device, api, pending, runMutation }: MutationControls & { device: Device }) {
  const id = useId();
  const capability = device.capabilities.remapping;
  const observed = device.values.remapping;
  const [draft, setDraft] = useState(() => savedDraft(device));
  const [allowWeakIdentity, setAllowWeakIdentity] = useState(false);
  const [recording, setRecording] = useState<string | null>(null);
  const [captureError, setCaptureError] = useState<string | null>(null);
  const sourceKey = useMemo(
    () => JSON.stringify([capability?.fingerprint, capability?.identityStrength, capability?.buttons.map((button) => [button.id, button.code])]),
    [capability?.fingerprint, capability?.identityStrength, capability?.buttons]
  );
  const savedKey = useMemo(
    () => JSON.stringify([observed?.profileId, observed?.bindings, observed?.autoApply]),
    [observed?.profileId, observed?.bindings, observed?.autoApply]
  );
  useEffect(() => { setDraft(savedDraft(device)); setRecording(null); setCaptureError(null); }, [device.id, sourceKey, savedKey]);
  useEffect(() => { setAllowWeakIdentity(false); }, [device.id, capability?.fingerprint, capability?.identityStrength]);

  const busy = hasPending(pending, `mouse/${device.id}/`);
  const writable = capability?.availability === "available" && capability.writable && !!capability.fingerprint && !!capability.identityStrength && capability.buttons.length > 0;
  const disabled = busy || !writable || observed?.state === "starting";
  const weak = capability?.identityStrength === "port";
  const consent = weak && draft.autoApply && allowWeakIdentity;
  const bindings = compileBindings(draft.bindings);
  const missing = observed?.bindings.filter((binding) => !capability?.buttons.some((button) => button.id === binding.buttonId)) ?? [];
  const needsRestore = missing.length > 0 && (observed?.enabled || observed?.autoApply || !!observed?.activePreset);
  const problem = needsRestore ? "Profile contains undetected buttons. Restore defaults before creating a new configuration." : remappingProblem(capability, bindings, draft.autoApply, consent);
  const saved = useMemo(() => savedDraft(device), [device.values.remapping, device.capabilities.remapping]);
  const dirty = useMemo(() => JSON.stringify(draft) !== JSON.stringify(saved), [draft, saved]);
  const retry = observed?.pausedForSafety || observed?.state === "paused" || observed?.state === "error" || (!observed?.enabled && bindings.length > 0);
  const canApply = !disabled && !problem && (dirty || retry);
  const canRestore = !busy && !!observed && (observed.enabled || observed.autoApply || observed.pausedForSafety || !!observed.activePreset || observed.state === "starting");
  const codes = [...new Set(capability?.keyCodes ?? [])].sort((a, b) => a - b);
  const mouseCodes = codes.filter(isMouseButtonCode);
  const keyCodes = codes.filter((code) => !isMouseButtonCode(code));
  const mediaCodes = keyCodes.filter((code) => !!mediaKeyNames[code]);
  const mainCodes = keyCodes.filter((code) => !isModifierCode(code));

  function changeBinding(buttonId: string, change: Partial<BindingDraft>) {
    setDraft((current) => ({ ...current, bindings: current.bindings.map((binding) => binding.buttonId === buttonId ? { ...binding, ...change } : binding) }));
  }

  function cancelRecording() { setRecording(null); setCaptureError(null); }

  function record(event: KeyboardEvent<HTMLButtonElement>, buttonId: string) {
    if (disabled || recording !== buttonId || event.currentTarget !== document.activeElement) return;
    if (event.key === "Tab") { cancelRecording(); return; }
    event.preventDefault();
    event.stopPropagation();
    if (event.key === "Escape") { cancelRecording(); return; }
    if (event.repeat || event.nativeEvent.isComposing) return;
    const code = keyboardCodes[event.code];
    if (isModifierCode(code)) return;
    const pressed = modifierGroups.filter((group) => event[group.flag]);
    const modifiers = pressed.map((group) => group.codes.find((entry) => codes.includes(entry)));
    if (!mainCodes.includes(code) || pressed.length === 0 || modifiers.some((entry) => entry === undefined)) {
      setCaptureError("Use allowed modifiers and a single detectable key. You can choose codes from controls or cancel.");
      return;
    }
    changeBinding(buttonId, { kind: "chord", code, modifiers: modifiers as number[] });
    cancelRecording();
  }

  return (
    <section className="panel settings-panel remapping-panel" aria-labelledby={`${id}-heading`}>
      <div className="section-heading"><div><span className="eyebrow">03 / Software remapping</span><h2 id={`${id}-heading`}>Detected buttons <span className="count-label">{capability?.buttons.length ?? 0}</span></h2></div><span className="unit-tag">EV_KEY</span></div>
      <div className="remapping-readback">
        <p className="readback-line" role="status">Observed state: <strong>{stateNames[observed?.state ?? "unavailable"]}</strong></p>
        <p className="control-hint">Saved configuration: {observed?.enabled ? "enabled" : "disabled"}. Saved auto-apply: {observed?.autoApply ? "yes" : "no"}.</p>
        {observed?.activePreset && <p className="node-name mono">Observed preset: {observed.activePreset}</p>}
        {observed?.message && <p className="control-hint">{observed.message}</p>}
        {observed?.pausedForSafety && <p className="control-reason">Automation paused for safety. Applying explicitly again can re-enable it.</p>}
      </div>
      <p className="control-hint">Actions remain in draft until Applied. Default passes original click through. No global macros or timed sequences.</p>
      {capability && <p className="control-hint">Provider: {providerNames[capability.provider]} / {availabilityNames[capability.availability]}. Identity: {capability.identityStrength === "serial" ? "verified serial" : weak ? "port binding (weak)" : "unverified"}.</p>}
      {(!writable || capability?.reason) && <p className="control-reason">{capability?.reason || problem}</p>}
      {missing.length > 0 && <p className="control-reason">Saved buttons not detected: {missing.map((binding) => binding.buttonId).join(", ")}.</p>}
      {!capability?.buttons.length && <p className="control-reason">No buttons detected. Hardware DPI buttons that do not emit input events are not simulated.</p>}
      <div className="mouse-bindings">
        {capability?.buttons.map((button, index) => {
          const binding = draft.bindings.find((entry) => entry.buttonId === button.id);
          if (!binding) return null;
          const prefix = `${id}-button-${index}`;
          const targetCodes = binding.kind === "mouseButton" ? mouseCodes : binding.kind === "media" ? mediaCodes : binding.kind === "chord" ? mainCodes : keyCodes;
          const saved = observed?.bindings.find((entry) => entry.buttonId === button.id)?.action ?? { kind: "default" };
          return <fieldset className="binding-row" key={button.id}>
            <legend>{button.label || `Button ${button.code}`} <span className="tiny-label mono">{button.code}</span></legend>
            <p className="binding-readback">Saved action: {mouseActionLabel(saved)}{!observed?.enabled ? " / inactive" : ""}</p>
            <div className="binding-fields">
              <div className="select-field"><label htmlFor={`${prefix}-action`}>Action for {button.label}</label>
                <select id={`${prefix}-action`} value={binding.kind} disabled={disabled} onChange={(event) => {
                  cancelRecording(); changeBinding(button.id, { kind: event.target.value as ActionKind, code: null, modifiers: [], cps: 10, autoclickButton: 272 });
                }}>
                  <option value="default">Default</option><option value="mouseButton" disabled={mouseCodes.length === 0}>Mouse click</option>
                  <option value="key" disabled={keyCodes.length === 0}>Key</option><option value="media" disabled={mediaCodes.length === 0}>Media</option>
                  <option value="chord" disabled={mainCodes.length === 0 || !modifierGroups.some((group) => group.codes.some((code) => codes.includes(code)))}>Shortcut</option>
                  <option value="autoclicker">Autoclicker</option>
                  <option value="disabled">Disabled</option>
                </select></div>
              {binding.kind !== "default" && binding.kind !== "disabled" && binding.kind !== "autoclicker" && <div className="select-field"><label htmlFor={`${prefix}-target`}>{binding.kind === "mouseButton" ? "Click" : binding.kind === "media" ? "Media" : "Key"} for {button.label}</label>
                <select id={`${prefix}-target`} value={binding.code ?? ""} disabled={disabled} onChange={(event) => changeBinding(button.id, { code: event.target.value === "" ? null : Number(event.target.value) })}>
                  <option value="">Choose a target</option>
                  {binding.code !== null && !targetCodes.includes(binding.code) && <option value={binding.code} disabled>Unavailable: {keyCodeLabel(binding.code)}</option>}
                  {targetCodes.map((code) => <option key={code} value={code}>{keyCodeLabel(code)} [{code}]</option>)}
                </select></div>}
              {binding.kind === "autoclicker" && <div className="select-field"><label htmlFor={`${prefix}-autoclick-btn`}>Button to click</label>
                <select id={`${prefix}-autoclick-btn`} value={binding.autoclickButton ?? 272} disabled={disabled} onChange={(event) => changeBinding(button.id, { autoclickButton: Number(event.target.value) })}>
                  {mouseCodes.map((code) => <option key={code} value={code}>{keyCodeLabel(code)}</option>)}
                </select></div>}
            </div>
            {binding.kind === "autoclicker" && <div className="autoclicker-container">
              <div className="field-heading">
                <label htmlFor={`${prefix}-cps`}>Autoclicker speed (CPS)</label>
                <output id={`${prefix}-cps-output`}>{binding.cps ?? 10} CPS</output>
              </div>
              <input id={`${prefix}-cps`} type="range" className="range-input" min={1} max={50} step={1}
                value={binding.cps ?? 10} disabled={disabled}
                style={{ "--range-fill": `${(((binding.cps ?? 10) - 1) / 49) * 100}%` } as React.CSSProperties}
                onChange={(event) => changeBinding(button.id, { cps: Number(event.target.value) })} />
              <div className="range-bounds">
                <span>1 CPS (slow)</span>
                <span>25 CPS</span>
                <span>50 CPS (maximum)</span>
              </div>
              <p className="control-hint">Holding {button.label || `button ${button.code}`}, will repeat {keyCodeLabel(binding.autoclickButton ?? 272)} at {binding.cps ?? 10} clicks per second.</p>
            </div>}
            {binding.kind === "chord" && <>
              <div className="chord-modifiers" role="group" aria-label={`Modifiers for ${button.label}`}>
                {modifierGroups.map((group) => <label key={group.label}><input type="checkbox" aria-label={`${group.label} for ${button.label}`} checked={binding.modifiers.some((code) => group.codes.some((entry) => entry === code))}
                  disabled={disabled || !group.codes.some((code) => codes.includes(code))} onChange={(event) => {
                    const modifiers = binding.modifiers.filter((code) => !group.codes.some((entry) => entry === code));
                    const code = group.codes.find((entry) => codes.includes(entry));
                    if (event.target.checked && code !== undefined) modifiers.push(code);
                    changeBinding(button.id, { modifiers });
                  }} />{group.label}</label>)}
              </div>
              <div className="action-row recorder-actions">
                <button type="button" className={`button button-secondary button-small shortcut-recorder${recording === button.id ? " is-recording" : ""}`} disabled={disabled}
                  aria-label={`${recording === button.id ? "Press a" : "Record"} shortcut for ${button.label}`} aria-pressed={recording === button.id}
                  aria-describedby={`${prefix}-privacy`} onClick={(event) => { event.currentTarget.focus(); setRecording(button.id); setCaptureError(null); }}
                  onKeyDown={(event) => record(event, button.id)} onBlur={cancelRecording}>{recording === button.id ? "Press shortcut..." : "Record shortcut"}</button>
                <button type="button" className="text-button" aria-label={`Cancel recording for ${button.label}`} disabled={recording !== button.id} onClick={cancelRecording}>Cancel</button>
              </div>
              <p className="control-hint" id={`${prefix}-privacy`}>Capture active only while button is focused. Esc, Tab, or blur cancels. Linux physical codes only, interpreted by system layout.</p>
              {recording === button.id && captureError && <p className="control-reason" role="status">{captureError}</p>}
            </>}
          </fieldset>;
        })}
      </div>
      <Toggle label="Auto-apply on startup" checked={draft.autoApply} disabled={disabled}
        description="Preference saved only when Applied. The agent keeps remappings after Exit and reactivates them at login, reboot, and reconnect."
        onChange={(autoApply) => setDraft((current) => ({ ...current, autoApply }))} />
      {weak && draft.autoApply && <label className="weak-identity-consent"><input type="checkbox" checked={allowWeakIdentity} disabled={disabled} onChange={(event) => setAllowWeakIdentity(event.target.checked)} />
        <span>I allow automatic reactivation for this port with weak identity.<small>A mouse of the same model connected to the same port may match this profile. Changing ports does not transfer consent.</small></span></label>}
      {writable && problem && <p className="control-reason" id={`${id}-problem`}>{problem}</p>}
      <div className="action-row">
        <button className="button button-primary" disabled={!canApply} aria-describedby={writable && problem ? `${id}-problem` : undefined} onClick={() => {
          if (canApply) void runMutation(`mouse/${device.id}/remapping`, () => api.applyMouseRemapping(device.id, bindings, draft.autoApply, consent));
        }}>{pending.has(`mouse/${device.id}/remapping`) ? "Applying buttons..." : "Apply buttons"}</button>
        <button className="button button-secondary" disabled={!canRestore} onClick={() => {
          if (canRestore) { cancelRecording(); void runMutation(`mouse/${device.id}/remapping-disable`, () => api.disableMouseRemapping(device.id)); }
        }}>{pending.has(`mouse/${device.id}/remapping-disable`) ? "Restoring buttons..." : "Restore defaults"}</button>
        <button className="text-button" disabled={busy || !dirty} onClick={() => { setDraft(savedDraft(device)); cancelRecording(); }}>Discard button draft</button>
      </div>
    </section>
  );
}
