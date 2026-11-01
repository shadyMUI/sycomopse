import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { AppError, AudioNode, AudioProfile, Device, DpiValues, HubApi, Snapshot } from "./types";
import { keyboardCodes, mouseButtonNames, remappingProblem } from "./remapping";

const browserMessage = "Backend non disponibile: i controlli hardware richiedono Peripheral Hub desktop su Linux. Questo browser non dispone del backend Tauri.";

function unsupportedSnapshot(): Snapshot {
  return {
    revision: 0,
    platform: "unsupportedPlatform",
    demo: false,
    devices: [],
    audio: {
      available: false, outputs: [], inputs: [], defaultOutput: null,
      configuredDefaultOutput: null, selectedOutput: null, defaultInput: null,
      surroundState: "off", canEnableSurround: false, surroundReason: browserMessage, hrtf: null,
    },
    profiles: [],
    diagnostics: [{ id: "browser", provider: "app", code: "unsupportedPlatform", severity: "warning", message: browserMessage, detail: null }],
  };
}

export function asAppError(error: unknown): AppError {
  if (typeof error === "object" && error !== null && "message" in error && typeof error.message === "string") {
    return {
      code: "code" in error && typeof error.code === "string" ? error.code : "backendError",
      message: error.message,
      detail: "detail" in error && typeof error.detail === "string" ? error.detail : null,
    };
  }
  return { code: "backendError", message: "Operazione non riuscita.", detail: typeof error === "string" ? error : null };
}

export function isDpiValueAllowed(values: DpiValues, value: number | null): boolean {
  if (value === null || !Number.isInteger(value) || value <= 0) return false;
  if (values.kind === "discrete") return values.values.includes(value);
  if (values.kind === "range") {
    return values.step > 0 && value >= values.min && value <= values.max && (value - values.min) % values.step === 0;
  }
  return false;
}

export function parseDpiDraft(draft: string): number | null {
  if (!/^\d+$/.test(draft)) return null;
  const value = Number(draft);
  return Number.isSafeInteger(value) && value > 0 ? value : null;
}

export function createTauriApi(): HubApi {
  async function command<T = Snapshot>(name: string, args?: Record<string, unknown>): Promise<T> {
    if (!isTauri()) throw { code: "unsupportedPlatform", message: browserMessage, detail: null } satisfies AppError;
    try {
      return args === undefined ? await invoke<T>(name) : await invoke<T>(name, args);
    } catch (error) {
      throw asAppError(error);
    }
  }

  return {
    demo: false,
    getSnapshot: () => isTauri() ? command("get_snapshot") : Promise.resolve(unsupportedSnapshot()),
    refreshDevices: () => isTauri() ? command("refresh_devices") : Promise.resolve(unsupportedSnapshot()),
    setDpi: (deviceId, x, y) => command("set_dpi", { deviceId, x, y }),
    setPollingRate: (deviceId, rate) => command("set_polling_rate", { deviceId, rate }),
    applyMouseRemapping: (deviceId, bindings, autoApply, allowWeakIdentity) => command("apply_mouse_remapping", { deviceId, bindings, autoApply, allowWeakIdentity }),
    disableMouseRemapping: (deviceId) => command("disable_mouse_remapping", { deviceId }),
    setDefaultOutput: (nodeName) => command("set_default_output", { nodeName }),
    setVolume: (nodeName, volume) => command("set_volume", { nodeName, volume }),
    setMute: (nodeName, muted) => command("set_mute", { nodeName, muted }),
    setSurround: (enabled) => command("set_surround", { enabled }),
    importHrtf: () => command("import_hrtf"),
    saveAudioProfile: (profile) => command("save_audio_profile", { profile }),
    applyAudioProfile: (profileId) => command("apply_audio_profile", { profileId }),
    exitApp: () => command<void>("exit_app"),
    async subscribe(listener) {
      if (!isTauri()) return () => {};
      return listen<Snapshot>("snapshot-changed", (event) => listener(event.payload));
    },
  };
}

function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

function demoError(message: string): never {
  throw { code: "demoValidation", message, detail: "Validazione dei dati simulati; nessun comando hardware eseguito." } satisfies AppError;
}

export function createDemoApi(count = 3): HubApi {
  if (!Number.isInteger(count) || count < 0 || count > 24) demoError("Numero di dispositivi demo non valido (0-24).");
  const now = Math.floor(Date.now() / 1000);
  const identity = {
    serial: null, physicalPath: null, vendorId: null, productId: null,
    transport: "Simulato", firmware: null, backendPath: null,
  };
  const remapping = {
    provider: "inputremapper" as const, availability: "available" as const, writable: true,
    fingerprint: null, identityStrength: "serial" as const,
    buttons: [272, 273, 274, 275, 276].map((code) => ({ id: `button-${code}`, label: mouseButtonNames[code], code })),
    keyCodes: [...new Set([...Object.values(keyboardCodes), ...Object.keys(mouseButtonNames).map(Number)])].sort((a, b) => a - b), reason: null,
  };
  const remappingValues = {
    state: "inactive" as const, enabled: false, autoApply: false, pausedForSafety: false,
    profileId: null, bindings: [], activePreset: null, message: "Rimappatura simulata: nessun evento input viene acquisito o emesso.",
  };
  const samples: Device[] = [
    {
      id: "demo-viper", name: "Razer Viper V3 HyperSpeed", brand: "razer", kind: "mouse", provider: "openrazer",
      identity: { ...identity, vendorId: 0x1532, productId: 0x00b8, transport: "2.4 GHz (demo)" },
      availability: "available", support: "supported",
      capabilities: {
        dpi: { values: { kind: "range", min: 100, max: 30000, step: 100 }, writable: true, separateAxes: true, reason: null },
        polling: { values: [125, 500, 1000], writable: true, reason: null },
        remapping,
      },
      values: {
        dpi: { x: 800, y: 800 }, pollingRate: 1000,
        battery: { percentage: 78, level: null, charging: false, source: "openrazer", observedAt: now, sourceUpdatedAt: now, stale: false },
        remapping: remappingValues,
      },
      outputNodeNames: [],
      limitations: ["Batteria AA: nessuna ricarica nel mouse.", "DPI, polling e rimappatura simulati. RGB e calibrazione non inclusi."],
    },
    {
      id: "demo-blackshark", name: "Razer BlackShark V3 X HyperSpeed", brand: "razer", kind: "headset", provider: "usb",
      identity: { ...identity, vendorId: 0x1532, productId: 0x057c, transport: "USB (demo)" },
      availability: "available", support: "partial",
      capabilities: { dpi: null, polling: null, remapping: null },
      values: { dpi: null, pollingRate: null, battery: null, remapping: null }, outputNodeNames: [],
      limitations: ["Identificazione USB e audio standard PipeWire.", "Batteria e controlli proprietari non esposti da OpenRazer stock."],
    },
    {
      id: "demo-g305", name: "Logitech G305", brand: "logitech", kind: "mouse", provider: "ratbag",
      identity: { ...identity, vendorId: 0x046d, productId: 0x4074, transport: "HID++ wireless (demo)" },
      availability: "available", support: "supported",
      capabilities: {
        dpi: { values: { kind: "discrete", values: [400, 800, 1600, 3200, 6400, 12000] }, writable: true, separateAxes: false, reason: null },
        polling: { values: [125, 250, 500, 1000], writable: true, reason: null },
        remapping,
      },
      values: { dpi: { x: 1600, y: 1600 }, pollingRate: 500, battery: null, remapping: remappingValues }, outputNodeNames: [],
      limitations: ["Elenco DPI simulato: quello reale dipende dal provider.", "Batteria non presunta da ratbagd. PID logico HID++, non identita USB del ricevitore."],
    },
  ];
  const devices = Array.from({ length: count }, (_, index) => {
    const sample = clone(samples[index % samples.length]);
    const instance = Math.floor(index / samples.length) + 1;
    sample.id += `-${instance}`;
    if (instance > 1) sample.name += ` (${instance})`;
    sample.identity.serial = `DEMO-${index + 1}`;
    sample.identity.physicalPath = `demo/device/${index + 1}`;
    sample.identity.backendPath = `demo/${sample.provider}/${index + 1}`;
    if (sample.capabilities.remapping) sample.capabilities.remapping.fingerprint = `demo-fingerprint-${index + 1}`;
    if (sample.kind === "headset") sample.outputNodeNames = [`alsa_output.usb-demo-blackshark-${instance}.analog-stereo`];
    return sample;
  });
  const baseNode: AudioNode = {
    id: 40, objectSerial: 1040, nodeName: "alsa_output.pci-0000_00_1f.3.analog-stereo",
    description: "Audio integrato - analogico stereo", mediaClass: "Audio/Sink", channels: ["FL", "FR"],
    volume: 0.62, muted: false, isVirtual: false, deviceSerial: null, deviceBusPath: null,
    vendorId: null, productId: null, transport: "Analogico (demo)",
  };
  const headsetNodes = devices.filter((device) => device.kind === "headset").map((device, index) => ({
    ...baseNode, id: 60 + index, objectSerial: 1060 + index, nodeName: device.outputNodeNames[0],
    description: `${device.name} - stereo USB`, volume: 0.68, deviceSerial: device.identity.serial,
    deviceBusPath: device.identity.physicalPath, vendorId: device.identity.vendorId, productId: device.identity.productId,
    transport: "USB (demo)",
  }));
  const outputs = [baseNode, ...headsetNodes];
  const selected = headsetNodes[0] ?? baseNode;
  const microphone: AudioNode = {
    ...baseNode, id: 80, objectSerial: 1080, nodeName: "alsa_input.pci-0000_00_1f.3.analog-stereo",
    description: "Ingresso microfono - audio integrato", mediaClass: "Audio/Source", channels: ["MONO"], volume: 0.5,
  };
  let state: Snapshot = {
    revision: 1, platform: "linux", demo: true, devices,
    audio: {
      available: true, outputs, inputs: [microphone], defaultOutput: selected.nodeName,
      configuredDefaultOutput: selected.nodeName, selectedOutput: selected.nodeName, defaultInput: microphone.nodeName,
      surroundState: "off", canEnableSurround: true, surroundReason: null,
      hrtf: { reference: "system", displayName: "HRTF di esempio (simulata)", readable: true },
    },
    profiles: [{ schemaVersion: 1, id: "8238c480-03a2-4d61-80c5-e6c4ddf7ce14", name: "Ascolto stereo", outputNodeName: selected.nodeName, hrtf: "system", volume: 0.6, mute: false, surroundRequested: false }],
    diagnostics: [
      { id: "demo-openrazer", provider: "openrazer", code: "available", severity: "info", message: "OpenRazer simulato", detail: "Nessuna connessione DBus e nessuna lettura hardware." },
      { id: "demo-ratbag", provider: "ratbag", code: "available", severity: "info", message: "ratbagd simulato", detail: "Liste e readback sintetici, non una verifica del supporto reale." },
      { id: "demo-inputremapper", provider: "inputremapper", code: "available", severity: "info", message: "Input Remapper simulato", detail: "Nessun grab, recorder globale, agente o evento hardware. Stato isolato in memoria." },
      { id: "demo-pipewire", provider: "pipewire", code: "available", severity: "info", message: "PipeWire simulato", detail: "Nessun flusso audio o filtro HRTF viene creato." },
      { id: "demo-upower", provider: "upower", code: "absent", severity: "info", message: "Nessun dato UPower simulato", detail: null },
      { id: "demo-headsetcontrol", provider: "headsetcontrol", code: "unsupported", severity: "info", message: "Batteria cuffie non esposta", detail: "La cuffia demo e' identificata solo via USB." },
    ],
  };
  const listeners = new Set<(snapshot: Snapshot) => void>();
  const hrtfs = new Map([["system", clone(state.audio.hrtf!)]]);
  const virtualName = "peripheral-hub.demo.surround-7.1";

  function commit(change: (next: Snapshot) => void): Snapshot {
    const next = clone(state);
    change(next);
    next.revision += 1;
    state = next;
    for (const listener of listeners) listener(clone(state));
    return clone(state);
  }

  function mouse(next: Snapshot, id: string): Device {
    const device = next.devices.find((entry) => entry.id === id);
    if (!device || device.kind !== "mouse" || device.availability !== "available") demoError("Mouse non disponibile.");
    return device;
  }

  function node(next: Snapshot, nodeName: string): AudioNode {
    const found = [...next.audio.outputs, ...next.audio.inputs].find((entry) => entry.nodeName === nodeName);
    if (!found) demoError("Nodo audio non disponibile.");
    return found;
  }

  function surround(next: Snapshot, enabled: boolean): void {
    if (enabled) {
      const target = next.audio.outputs.find((entry) => entry.nodeName === next.audio.selectedOutput && !entry.isVirtual);
      if (!next.audio.canEnableSurround || !target || target.channels.length !== 2 || !next.audio.hrtf?.readable) demoError("Surround non disponibile per il target o la HRTF selezionati.");
      if (!next.audio.outputs.some((entry) => entry.nodeName === virtualName)) {
        next.audio.outputs.push({ ...clone(baseNode), id: 501, objectSerial: 1501, nodeName: virtualName, description: "Peripheral Hub - ingresso 7.1 (simulato)", channels: ["FL", "FR", "FC", "LFE", "RL", "RR", "SL", "SR"], isVirtual: true, transport: "Virtuale (demo)" });
      }
      next.audio.surroundState = "on";
      next.audio.defaultOutput = virtualName;
      next.audio.configuredDefaultOutput = virtualName;
    } else {
      next.audio.outputs = next.audio.outputs.filter((entry) => entry.nodeName !== virtualName);
      next.audio.surroundState = "off";
      next.audio.defaultOutput = next.audio.outputs.find((entry) => entry.nodeName === next.audio.selectedOutput)?.nodeName ?? next.audio.outputs.find((entry) => !entry.isVirtual)?.nodeName ?? null;
      next.audio.configuredDefaultOutput = next.audio.defaultOutput;
    }
  }

  function validateProfile(next: Snapshot, profile: AudioProfile): AudioNode {
    if (profile.schemaVersion !== 1 || !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(profile.id) || !profile.name.trim() || profile.name.length > 80) demoError("Profilo non valido: nome, UUID o versione non corretti.");
    if (!Number.isFinite(profile.volume) || profile.volume < 0 || profile.volume > 1) demoError("Il volume deve essere tra 0 e 100%.");
    const output = next.audio.outputs.find((entry) => entry.nodeName === profile.outputNodeName && !entry.isVirtual);
    if (!output) demoError("L'uscita fisica del profilo non e' disponibile.");
    if (profile.hrtf !== null && !hrtfs.get(profile.hrtf)?.readable) demoError("Riferimento HRTF non disponibile nella demo.");
    if (profile.surroundRequested && (!profile.hrtf || !hrtfs.get(profile.hrtf)?.readable || output.channels.length !== 2)) demoError("Il profilo 7.1 richiede una HRTF leggibile e un'uscita stereo.");
    return output;
  }

  return {
    demo: true,
    async getSnapshot() { return clone(state); },
    async refreshDevices() { return commit(() => {}); },
    async setDpi(deviceId, x, y) {
      return commit((next) => {
        const device = mouse(next, deviceId);
        const capability = device.capabilities.dpi;
        if (!capability?.writable || !isDpiValueAllowed(capability.values, x) || !isDpiValueAllowed(capability.values, y) || (!capability.separateAxes && x !== y)) demoError("DPI non validi per questo mouse.");
        device.values.dpi = { x, y };
      });
    },
    async setPollingRate(deviceId, rate) {
      return commit((next) => {
        const device = mouse(next, deviceId);
        if (!device.capabilities.polling?.writable || !device.capabilities.polling.values.includes(rate)) demoError("Polling non supportato da questo mouse.");
        device.values.pollingRate = rate;
      });
    },
    async applyMouseRemapping(deviceId, bindings, autoApply, allowWeakIdentity) {
      return commit((next) => {
        const device = next.devices.find((entry) => entry.id === deviceId && entry.kind === "mouse");
        if (!device) demoError("Mouse non disponibile.");
        if (typeof autoApply !== "boolean" || typeof allowWeakIdentity !== "boolean") demoError("Preferenze di rimappatura non valide.");
        const problem = remappingProblem(device.capabilities.remapping, bindings, autoApply, allowWeakIdentity);
        if (problem) demoError(problem);
        const profileId = device.values.remapping?.profileId ?? `demo-profile-${device.id}`;
        const enabled = bindings.some((binding) => binding.action.kind !== "default");
        device.values.remapping = {
          state: enabled ? "active" : "inactive", enabled, autoApply: enabled && autoApply, pausedForSafety: false,
          profileId, bindings: clone(bindings), activePreset: enabled ? `${profileId}-${next.revision + 1}` : null,
          message: "Rimappatura simulata: nessun evento input viene acquisito o emesso.",
        };
      });
    },
    async disableMouseRemapping(deviceId) {
      return commit((next) => {
        const device = next.devices.find((entry) => entry.id === deviceId && entry.kind === "mouse");
        if (!device?.values.remapping) demoError("Configurazione di rimappatura demo non disponibile.");
        device.values.remapping = {
          ...device.values.remapping, state: "inactive", enabled: false, autoApply: false,
          pausedForSafety: false, activePreset: null, message: "Pulsanti originali ripristinati nella demo; nessun comando hardware.",
        };
      });
    },
    async setDefaultOutput(nodeName) {
      return commit((next) => {
        const output = next.audio.outputs.find((entry) => entry.nodeName === nodeName);
        if (!output) demoError("Uscita audio non disponibile.");
        if (output.isVirtual && next.audio.surroundState !== "on") demoError("Il nodo virtuale richiede il surround attivo.");
        if (!output.isVirtual) next.audio.selectedOutput = nodeName;
        next.audio.defaultOutput = next.audio.surroundState === "on" ? virtualName : nodeName;
        next.audio.configuredDefaultOutput = next.audio.defaultOutput;
      });
    },
    async setVolume(nodeName, volume) {
      return commit((next) => {
        const target = node(next, nodeName);
        if (target.volume === null || !Number.isFinite(volume) || volume < 0 || volume > 1) demoError("Volume non disponibile o fuori intervallo (0-100%).");
        target.volume = volume;
      });
    },
    async setMute(nodeName, muted) {
      return commit((next) => {
        const target = node(next, nodeName);
        if (target.muted === null || typeof muted !== "boolean") demoError("Stato mute non disponibile.");
        target.muted = muted;
      });
    },
    async setSurround(enabled) { return commit((next) => surround(next, enabled)); },
    async importHrtf() {
      return commit((next) => {
        if (next.audio.surroundState === "on") surround(next, false);
        next.audio.hrtf = { reference: "975c10a0-f67e-478e-94a6-bcad73bce14c.sofa", displayName: "SOFA importata (simulata)", readable: true };
        hrtfs.set(next.audio.hrtf.reference, clone(next.audio.hrtf));
      });
    },
    async saveAudioProfile(profile) {
      return commit((next) => {
        validateProfile(next, profile);
        const index = next.profiles.findIndex((entry) => entry.id === profile.id);
        if (index === -1) next.profiles.push(clone(profile));
        else next.profiles[index] = clone(profile);
      });
    },
    async applyAudioProfile(profileId) {
      return commit((next) => {
        const profile = next.profiles.find((entry) => entry.id === profileId);
        if (!profile) demoError("Profilo non trovato.");
        const output = validateProfile(next, profile);
        next.audio.selectedOutput = output.nodeName;
        if (profile.hrtf !== null) next.audio.hrtf = clone(hrtfs.get(profile.hrtf)!);
        surround(next, profile.surroundRequested);
        const volumeTarget = profile.surroundRequested ? node(next, virtualName) : output;
        volumeTarget.volume = profile.volume;
        volumeTarget.muted = profile.mute;
      });
    },
    async subscribe(listener) {
      listeners.add(listener);
      return () => { listeners.delete(listener); };
    },
    async exitApp() {},
  };
}

export function createHubApi(): HubApi {
  if (import.meta.env.VITE_DEMO === "true") {
    const requested = new URLSearchParams(window.location.search).get("devices");
    return createDemoApi(requested !== null && ["0", "1", "3"].includes(requested) ? Number(requested) : 3);
  }
  return createTauriApi();
}
