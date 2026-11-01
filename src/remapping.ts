import type { MouseAction, MouseBinding, RemappingCapability } from "./types";

export const mouseButtonNames: Record<number, string> = {
  272: "Clic sinistro", 273: "Clic destro", 274: "Clic centrale", 275: "Laterale",
  276: "Laterale extra", 277: "Avanti", 278: "Indietro", 279: "Task",
};

export const modifierGroups = [
  { label: "Ctrl", flag: "ctrlKey", codes: [29, 97] },
  { label: "Shift", flag: "shiftKey", codes: [42, 54] },
  { label: "Alt", flag: "altKey", codes: [56, 100] },
  { label: "Meta", flag: "metaKey", codes: [125, 126] },
] as const;

export const keyboardCodes: Readonly<Record<string, number>> = {
  Escape: 1, Digit1: 2, Digit2: 3, Digit3: 4, Digit4: 5, Digit5: 6, Digit6: 7, Digit7: 8, Digit8: 9, Digit9: 10, Digit0: 11,
  Minus: 12, Equal: 13, Backspace: 14, Tab: 15, KeyQ: 16, KeyW: 17, KeyE: 18, KeyR: 19, KeyT: 20, KeyY: 21,
  KeyU: 22, KeyI: 23, KeyO: 24, KeyP: 25, BracketLeft: 26, BracketRight: 27, Enter: 28, ControlLeft: 29,
  KeyA: 30, KeyS: 31, KeyD: 32, KeyF: 33, KeyG: 34, KeyH: 35, KeyJ: 36, KeyK: 37, KeyL: 38, Semicolon: 39,
  Quote: 40, Backquote: 41, ShiftLeft: 42, Backslash: 43, KeyZ: 44, KeyX: 45, KeyC: 46, KeyV: 47, KeyB: 48,
  KeyN: 49, KeyM: 50, Comma: 51, Period: 52, Slash: 53, ShiftRight: 54, NumpadMultiply: 55, AltLeft: 56, Space: 57,
  CapsLock: 58, F1: 59, F2: 60, F3: 61, F4: 62, F5: 63, F6: 64, F7: 65, F8: 66, F9: 67, F10: 68,
  NumLock: 69, ScrollLock: 70, Numpad7: 71, Numpad8: 72, Numpad9: 73, NumpadSubtract: 74, Numpad4: 75,
  Numpad5: 76, Numpad6: 77, NumpadAdd: 78, Numpad1: 79, Numpad2: 80, Numpad3: 81, Numpad0: 82, NumpadDecimal: 83,
  IntlBackslash: 86, F11: 87, F12: 88, IntlRo: 89, NumpadEnter: 96, ControlRight: 97, NumpadDivide: 98,
  PrintScreen: 99, AltRight: 100, Home: 102, ArrowUp: 103, PageUp: 104, ArrowLeft: 105, ArrowRight: 106,
  End: 107, ArrowDown: 108, PageDown: 109, Insert: 110, Delete: 111, AudioVolumeMute: 113, AudioVolumeDown: 114,
  AudioVolumeUp: 115, Pause: 119, IntlYen: 124, MetaLeft: 125, MetaRight: 126, ContextMenu: 139,
  BrowserBack: 158, BrowserForward: 159, MediaTrackNext: 163, MediaPlayPause: 164, MediaTrackPrevious: 165,
  MediaStop: 166, F13: 183, F14: 184, F15: 185, F16: 186, F17: 187, F18: 188, F19: 189, F20: 190,
  F21: 191, F22: 192, F23: 193, F24: 194, MediaSelect: 226,
};

export const mediaKeyNames: Record<number, string> = {
  113: "Silenzia audio", 114: "Volume giu", 115: "Volume su", 163: "Traccia successiva",
  164: "Riproduci / pausa", 165: "Traccia precedente", 166: "Stop", 168: "Riavvolgi",
  200: "Riproduci CD", 201: "Pausa CD", 207: "Riproduci", 208: "Avanti veloce", 226: "Apri lettore multimediale",
};

export function isMouseButtonCode(code: number): boolean { return code >= 272 && code <= 279; }
export function isModifierCode(code: number): boolean { return modifierGroups.some((group) => group.codes.some((entry) => entry === code)); }

export function keyCodeLabel(code: number): string {
  if (mouseButtonNames[code]) return mouseButtonNames[code];
  if (mediaKeyNames[code]) return mediaKeyNames[code];
  const modifier = modifierGroups.find((group) => group.codes.some((entry) => entry === code));
  if (modifier) return `${modifier.label} ${modifier.codes[0] === code ? "sinistro" : "destro"}`;
  const name = Object.entries(keyboardCodes).find(([, value]) => value === code)?.[0];
  return name ? name.replace(/^(Key|Digit)/, "") : `Codice Linux ${code}`;
}

export function mouseActionLabel(action: MouseAction): string {
  switch (action.kind) {
    case "default": return "Predefinito (passaggio originale)";
    case "disabled": return "Disabilitato";
    case "mouseButton": return keyCodeLabel(action.code);
    case "key": return keyCodeLabel(action.code);
    case "chord": return [...action.modifiers.map(keyCodeLabel), keyCodeLabel(action.key)].join(" + ");
  }
}

export function remappingProblem(capability: RemappingCapability | null, bindings: MouseBinding[], autoApply: boolean, allowWeakIdentity: boolean): string | null {
  if (!capability || capability.availability !== "available" || !capability.writable) return capability?.reason || "Rimappatura non disponibile in scrittura.";
  if (!capability.fingerprint || !capability.identityStrength) return "Identita del mouse non verificata: nessuna rimappatura viene applicata.";
  if (capability.buttons.length === 0) return "Nessun pulsante rilevato: nessuna sorgente viene presunta.";
  if (!Array.isArray(bindings) || bindings.length > capability.buttons.length) return "Elenco di pulsanti non valido.";
  const ids = new Set<string>();
  const allowed = (code: number) => Number.isInteger(code) && code > 0 && capability.keyCodes.includes(code);
  for (const binding of bindings) {
    if (!binding || !capability.buttons.some((button) => button.id === binding.buttonId) || ids.has(binding.buttonId)) return "La bozza contiene pulsanti non rilevati o duplicati.";
    ids.add(binding.buttonId);
    const action = binding.action;
    if (!action) return "Azione non valida.";
    switch (action.kind) {
      case "default": case "disabled": break;
      case "mouseButton":
        if (!allowed(action.code) || !isMouseButtonCode(action.code)) return "Scegli un clic presente nei target consentiti dal provider.";
        break;
      case "key":
        if (!allowed(action.code) || isMouseButtonCode(action.code)) return "Scegli un tasto presente nei target consentiti dal provider.";
        break;
      case "chord":
        if (!allowed(action.key) || isMouseButtonCode(action.key) || isModifierCode(action.key)
          || !Array.isArray(action.modifiers) || action.modifiers.length === 0 || action.modifiers.length > 4
          || new Set(action.modifiers).size !== action.modifiers.length
          || action.modifiers.some((code) => !allowed(code) || !isModifierCode(code))) return "La scorciatoia richiede modificatori consentiti e un solo tasto principale valido.";
        break;
      default: return "Azione non supportata: macro e sequenze libere non sono consentite.";
    }
  }
  const hasLeftClick = capability.buttons.some((button) => {
    const action = bindings.find((binding) => binding.buttonId === button.id)?.action;
    return (!action || action.kind === "default") && button.code === 272 || action?.kind === "mouseButton" && action.code === 272;
  });
  if (!hasLeftClick) return "Mantieni almeno una via al clic sinistro: lascia il pulsante originale o assegna un altro pulsante al clic sinistro.";
  if (autoApply && capability.identityStrength === "port" && !allowWeakIdentity) return "La riattivazione automatica per un'identita debole richiede il consenso esplicito per questa porta.";
  return null;
}
