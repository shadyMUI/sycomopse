import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { AppError, DpiValues, HubApi, Snapshot } from "./types";

const browserMessage = "Backend unavailable: hardware controls require Sycomopse desktop on Linux. This browser does not have the Tauri backend.";

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
      equalizers: {}, equalizerOutput: null, ownedFilterOutput: null,
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
  return { code: "backendError", message: "Operation failed.", detail: typeof error === "string" ? error : null };
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
    setSurround: (enabled, expectedOutput) => command("set_surround", enabled ? { enabled, expectedOutput } : { enabled }),
    setEqualizer: (nodeName, settings) => command("set_equalizer", { nodeName, settings }),
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

// DEF-28: RFC 4122 v4 UUID generator with fallback for legacy WebKitGTK
export function generateUuid(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  if (typeof crypto !== "undefined" && typeof crypto.getRandomValues === "function") {
    const bytes = new Uint8Array(16);
    crypto.getRandomValues(bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40; // Version 4
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // Variant 10xx
    const hex = [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
    return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20, 32)}`;
  }
  return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (c) => {
    const r = (Math.random() * 16) | 0;
    const v = c === "x" ? r : (r & 0x3) | 0x8;
    return v.toString(16);
  });
}

// Re-export createDemoApi for backwards compatibility (tree-shaken in production)
export { createDemoApi } from "./demoApi";

// DEF-13: Synchronous createHubApi with lazy-loaded demo engine when VITE_DEMO=true
export function createHubApi(): HubApi {
  if (import.meta.env.VITE_DEMO === "true") {
    let demoInstancePromise: Promise<HubApi> | null = null;
    const getDemo = () => {
      if (!demoInstancePromise) {
        const requested = typeof window !== "undefined" ? new URLSearchParams(window.location.search).get("devices") : null;
        const count = requested !== null && ["0", "1", "3"].includes(requested) ? Number(requested) : 3;
        demoInstancePromise = import("./demoApi").then((m) => m.createDemoApi(count));
      }
      return demoInstancePromise;
    };

    return {
      demo: true,
      getSnapshot: () => getDemo().then((api) => api.getSnapshot()),
      refreshDevices: () => getDemo().then((api) => api.refreshDevices()),
      setDpi: (deviceId, x, y) => getDemo().then((api) => api.setDpi(deviceId, x, y)),
      setPollingRate: (deviceId, rate) => getDemo().then((api) => api.setPollingRate(deviceId, rate)),
      applyMouseRemapping: (deviceId, bindings, autoApply, allowWeakIdentity) =>
        getDemo().then((api) => api.applyMouseRemapping(deviceId, bindings, autoApply, allowWeakIdentity)),
      disableMouseRemapping: (deviceId) => getDemo().then((api) => api.disableMouseRemapping(deviceId)),
      setDefaultOutput: (nodeName) => getDemo().then((api) => api.setDefaultOutput(nodeName)),
      setVolume: (nodeName, volume) => getDemo().then((api) => api.setVolume(nodeName, volume)),
      setMute: (nodeName, muted) => getDemo().then((api) => api.setMute(nodeName, muted)),
      setSurround: (enabled, expectedOutput) => getDemo().then((api) => api.setSurround(enabled, expectedOutput)),
      setEqualizer: (nodeName, settings) => getDemo().then((api) => api.setEqualizer(nodeName, settings)),
      importHrtf: () => getDemo().then((api) => api.importHrtf()),
      saveAudioProfile: (profile) => getDemo().then((api) => api.saveAudioProfile(profile)),
      applyAudioProfile: (profileId) => getDemo().then((api) => api.applyAudioProfile(profileId)),
      subscribe: async (listener) => {
        const api = await getDemo();
        return api.subscribe(listener);
      },
      exitApp: () => getDemo().then((api) => api.exitApp()),
    };
  }
  return createTauriApi();
}
