import { useEffect, useId, useState } from "react";
import type { KeyboardEvent } from "react";
import type { Device, MouseAction, MouseBinding, RemappingValues } from "../types";
import type { MutationControls } from "../components/Controls";
import { Toggle, availabilityNames, hasPending, providerNames } from "../components/Controls";
import { isModifierCode, isMouseButtonCode, keyboardCodes, keyCodeLabel, mediaKeyNames, modifierGroups, mouseActionLabel, remappingProblem } from "../remapping";

type ActionKind = MouseAction["kind"] | "media";
interface BindingDraft { buttonId: string; kind: ActionKind; code: number | null; modifiers: number[] }

function savedDraft(device: Device) {
  return {
    autoApply: device.values.remapping?.autoApply ?? false,
    bindings: (device.capabilities.remapping?.buttons ?? []).map((button): BindingDraft => {
      const action = device.values.remapping?.bindings.find((binding) => binding.buttonId === button.id)?.action ?? { kind: "default" };
      return {
        buttonId: button.id, kind: action.kind === "key" && mediaKeyNames[action.code] ? "media" : action.kind,
        code: "code" in action ? action.code : action.kind === "chord" ? action.key : null,
        modifiers: action.kind === "chord" ? [...action.modifiers] : [],
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
    }
    return { buttonId: draft.buttonId, action };
  });
}

const stateNames: Record<RemappingValues["state"], string> = {
  unavailable: "Non disponibile", inactive: "Inattiva", starting: "Avvio in corso", active: "Attiva",
  busy: "Occupata", error: "Errore", paused: "In pausa",
};

export default function MouseButtons({ device, api, pending, runMutation }: MutationControls & { device: Device }) {
  const id = useId();
  const capability = device.capabilities.remapping;
  const observed = device.values.remapping;
  const [draft, setDraft] = useState(() => savedDraft(device));
  const [allowWeakIdentity, setAllowWeakIdentity] = useState(false);
  const [recording, setRecording] = useState<string | null>(null);
  const [captureError, setCaptureError] = useState<string | null>(null);
  const sourceKey = JSON.stringify([capability?.fingerprint, capability?.identityStrength, capability?.buttons.map((button) => [button.id, button.code])]);
  const savedKey = JSON.stringify([observed?.profileId, observed?.bindings, observed?.autoApply]);
  // State-only readbacks do not overwrite unsaved actions or restart key acquisition.
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
  const problem = needsRestore ? "Il profilo contiene pulsanti non rilevati. Ripristina gli originali prima di creare una nuova configurazione." : remappingProblem(capability, bindings, draft.autoApply, consent);
  const dirty = JSON.stringify(draft) !== JSON.stringify(savedDraft(device));
  const retry = observed?.pausedForSafety || observed?.state === "paused" || observed?.state === "error" || !observed?.enabled && bindings.length > 0;
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
      setCaptureError("Usa modificatori consentiti e un solo tasto rilevabile. Puoi scegliere i codici dai controlli o annullare.");
      return;
    }
    changeBinding(buttonId, { kind: "chord", code, modifiers: modifiers as number[] });
    cancelRecording();
  }

  return (
    <section className="panel settings-panel remapping-panel" aria-labelledby={`${id}-heading`}>
      <div className="section-heading"><div><span className="eyebrow">03 / Rimappatura software</span><h2 id={`${id}-heading`}>Pulsanti rilevati <span className="count-label">{capability?.buttons.length ?? 0}</span></h2></div><span className="unit-tag">EV_KEY</span></div>
      <div className="remapping-readback">
        <p className="readback-line" role="status">Stato osservato: <strong>{stateNames[observed?.state ?? "unavailable"]}</strong></p>
        <p className="control-hint">Configurazione salvata: {observed?.enabled ? "abilitata" : "disabilitata"}. Riattivazione automatica salvata: {observed?.autoApply ? "si" : "no"}.</p>
        {observed?.activePreset && <p className="node-name mono">Preset osservato: {observed.activePreset}</p>}
        {observed?.message && <p className="control-hint">{observed.message}</p>}
        {observed?.pausedForSafety && <p className="control-reason">Automatismo sospeso per sicurezza. Un nuovo Applica esplicito puo riattivarlo.</p>}
      </div>
      <p className="control-hint">Le azioni restano in bozza fino ad Applica. Predefinito lascia passare il clic originale. Nessuna macro, sequenza temporizzata o registrazione globale.</p>
      {capability && <p className="control-hint">Provider: {providerNames[capability.provider]} / {availabilityNames[capability.availability]}. Identita: {capability.identityStrength === "serial" ? "seriale verificato" : weak ? "associazione per porta (debole)" : "non verificata"}.</p>}
      {(!writable || capability?.reason) && <p className="control-reason">{capability?.reason || problem}</p>}
      {missing.length > 0 && <p className="control-reason">Pulsanti salvati non rilevati: {missing.map((binding) => binding.buttonId).join(", ")}.</p>}
      {!capability?.buttons.length && <p className="control-reason">Nessun pulsante rilevato. I pulsanti hardware DPI che non emettono eventi input non vengono inventati.</p>}
      <div className="mouse-bindings">
        {capability?.buttons.map((button, index) => {
          const binding = draft.bindings.find((entry) => entry.buttonId === button.id);
          if (!binding) return null;
          const prefix = `${id}-button-${index}`;
          const targetCodes = binding.kind === "mouseButton" ? mouseCodes : binding.kind === "media" ? mediaCodes : binding.kind === "chord" ? mainCodes : keyCodes;
          const saved = observed?.bindings.find((entry) => entry.buttonId === button.id)?.action ?? { kind: "default" };
          return <fieldset className="binding-row" key={button.id}>
            <legend>{button.label || `Pulsante ${button.code}`} <span className="tiny-label mono">{button.code}</span></legend>
            <p className="binding-readback">Azione salvata: {mouseActionLabel(saved)}{!observed?.enabled ? " / inattiva" : ""}</p>
            <div className="binding-fields">
              <div className="select-field"><label htmlFor={`${prefix}-action`}>Azione per {button.label}</label>
                <select id={`${prefix}-action`} value={binding.kind} disabled={disabled} onChange={(event) => {
                  cancelRecording(); changeBinding(button.id, { kind: event.target.value as ActionKind, code: null, modifiers: [] });
                }}>
                  <option value="default">Predefinito</option><option value="mouseButton" disabled={mouseCodes.length === 0}>Clic mouse</option>
                  <option value="key" disabled={keyCodes.length === 0}>Tasto</option><option value="media" disabled={mediaCodes.length === 0}>Multimedia</option>
                  <option value="chord" disabled={mainCodes.length === 0 || !modifierGroups.some((group) => group.codes.some((code) => codes.includes(code)))}>Scorciatoia</option>
                  <option value="disabled">Disabilitato</option>
                </select></div>
              {binding.kind !== "default" && binding.kind !== "disabled" && <div className="select-field"><label htmlFor={`${prefix}-target`}>{binding.kind === "mouseButton" ? "Clic" : binding.kind === "media" ? "Multimedia" : "Tasto"} per {button.label}</label>
                <select id={`${prefix}-target`} value={binding.code ?? ""} disabled={disabled} onChange={(event) => changeBinding(button.id, { code: event.target.value === "" ? null : Number(event.target.value) })}>
                  <option value="">Scegli un target</option>
                  {binding.code !== null && !targetCodes.includes(binding.code) && <option value={binding.code} disabled>Non disponibile: {keyCodeLabel(binding.code)}</option>}
                  {targetCodes.map((code) => <option key={code} value={code}>{keyCodeLabel(code)} [{code}]</option>)}
                </select></div>}
            </div>
            {binding.kind === "chord" && <>
              <div className="chord-modifiers" role="group" aria-label={`Modificatori per ${button.label}`}>
                {modifierGroups.map((group) => <label key={group.label}><input type="checkbox" aria-label={`${group.label} per ${button.label}`} checked={binding.modifiers.some((code) => group.codes.some((entry) => entry === code))}
                  disabled={disabled || !group.codes.some((code) => codes.includes(code))} onChange={(event) => {
                    const modifiers = binding.modifiers.filter((code) => !group.codes.some((entry) => entry === code));
                    const code = group.codes.find((entry) => codes.includes(entry));
                    if (event.target.checked && code !== undefined) modifiers.push(code);
                    changeBinding(button.id, { modifiers });
                  }} />{group.label}</label>)}
              </div>
              <div className="action-row recorder-actions">
                <button type="button" className={`button button-secondary button-small shortcut-recorder${recording === button.id ? " is-recording" : ""}`} disabled={disabled}
                  aria-label={`${recording === button.id ? "Premi una" : "Acquisisci"} scorciatoia per ${button.label}`} aria-pressed={recording === button.id}
                  aria-describedby={`${prefix}-privacy`} onClick={(event) => { event.currentTarget.focus(); setRecording(button.id); setCaptureError(null); }}
                  onKeyDown={(event) => record(event, button.id)} onBlur={cancelRecording}>{recording === button.id ? "Premi la scorciatoia..." : "Acquisisci scorciatoia"}</button>
                <button type="button" className="text-button" aria-label={`Annulla acquisizione per ${button.label}`} disabled={recording !== button.id} onClick={cancelRecording}>Annulla</button>
              </div>
              <p className="control-hint" id={`${prefix}-privacy`}>Acquisizione solo mentre il pulsante e focalizzato. Esc, Tab o perdita del focus annullano. Solo codici fisici Linux, interpretati dal layout di sistema.</p>
              {recording === button.id && captureError && <p className="control-reason" role="status">{captureError}</p>}
            </>}
          </fieldset>;
        })}
      </div>
      <Toggle label="Riattiva automaticamente" checked={draft.autoApply} disabled={disabled}
        description="Preferenza salvata solo con Applica. L'agente mantiene le rimappature dopo Esci e le riattiva a login, riavvio e riconnessione."
        onChange={(autoApply) => setDraft((current) => ({ ...current, autoApply }))} />
      {weak && draft.autoApply && <label className="weak-identity-consent"><input type="checkbox" checked={allowWeakIdentity} disabled={disabled} onChange={(event) => setAllowWeakIdentity(event.target.checked)} />
        <span>Consento la riattivazione automatica per questa porta con identita debole.<small>Un mouse dello stesso modello collegato alla stessa porta potrebbe essere associato al profilo. Cambiando porta il consenso non viene trasferito.</small></span></label>}
      {writable && problem && <p className="control-reason" id={`${id}-problem`}>{problem}</p>}
      <div className="action-row">
        <button className="button button-primary" disabled={!canApply} aria-describedby={writable && problem ? `${id}-problem` : undefined} onClick={() => {
          if (canApply) void runMutation(`mouse/${device.id}/remapping`, () => api.applyMouseRemapping(device.id, bindings, draft.autoApply, consent));
        }}>{pending.has(`mouse/${device.id}/remapping`) ? "Applicazione pulsanti..." : "Applica pulsanti"}</button>
        <button className="button button-secondary" disabled={!canRestore} onClick={() => {
          if (canRestore) { cancelRecording(); void runMutation(`mouse/${device.id}/remapping-disable`, () => api.disableMouseRemapping(device.id)); }
        }}>{pending.has(`mouse/${device.id}/remapping-disable`) ? "Ripristino pulsanti..." : "Ripristina originali"}</button>
        <button className="text-button" disabled={busy || !dirty} onClick={() => { setDraft(savedDraft(device)); cancelRecording(); }}>Annulla bozza pulsanti</button>
      </div>
    </section>
  );
}
