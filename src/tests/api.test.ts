import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createDemoApi, createHubApi, createTauriApi, isDpiValueAllowed, parseDpiDraft } from "../api";
import { remappingProblem } from "../remapping";
import type { AudioProfile, MouseBinding, Snapshot } from "../types";

const ipc = vi.hoisted(() => ({ invoke: vi.fn(), isTauri: vi.fn(), listen: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: ipc.invoke, isTauri: ipc.isTauri }));
vi.mock("@tauri-apps/api/event", () => ({ listen: ipc.listen }));

beforeEach(() => {
  ipc.invoke.mockReset();
  ipc.isTauri.mockReset().mockReturnValue(true);
  ipc.listen.mockReset().mockResolvedValue(() => {});
});
afterEach(() => {
  vi.unstubAllEnvs();
  window.history.replaceState(null, "", "/");
});

describe("contratti IPC Tauri", () => {
  it("invoca i comandi esatti con gli argomenti camelCase, senza path HRTF dal frontend", async () => {
    const snapshot = await createDemoApi().getSnapshot();
    ipc.invoke.mockResolvedValue(snapshot);
    const api = createTauriApi();
    const profile = snapshot.profiles[0];
    await api.getSnapshot();
    await api.refreshDevices();
    await api.setDpi("mouse-1", 800, 1600);
    await api.setPollingRate("mouse-1", 250);
    const bindings: MouseBinding[] = [{ buttonId: "button-275", action: { kind: "chord", modifiers: [29], key: 30 } }];
    await api.applyMouseRemapping("mouse-1", bindings, true, false);
    await api.disableMouseRemapping("mouse-1");
    await api.setDefaultOutput("alsa_output.test");
    await api.setVolume("alsa_input.test", 0.42);
    await api.setMute("alsa_input.test", true);
    await api.setSurround(true);
    await api.importHrtf();
    await api.saveAudioProfile(profile);
    await api.applyAudioProfile(profile.id);
    ipc.invoke.mockResolvedValueOnce(undefined);
    await api.exitApp();
    expect(ipc.invoke.mock.calls).toEqual([
      ["get_snapshot"], ["refresh_devices"], ["set_dpi", { deviceId: "mouse-1", x: 800, y: 1600 }],
      ["set_polling_rate", { deviceId: "mouse-1", rate: 250 }],
      ["apply_mouse_remapping", { deviceId: "mouse-1", bindings, autoApply: true, allowWeakIdentity: false }],
      ["disable_mouse_remapping", { deviceId: "mouse-1" }], ["set_default_output", { nodeName: "alsa_output.test" }],
      ["set_volume", { nodeName: "alsa_input.test", volume: 0.42 }], ["set_mute", { nodeName: "alsa_input.test", muted: true }],
      ["set_surround", { enabled: true }], ["import_hrtf"], ["save_audio_profile", { profile }],
      ["apply_audio_profile", { profileId: profile.id }], ["exit_app"],
    ]);
  });

  it("sottoscrive snapshot-changed e restituisce il cleanup Tauri", async () => {
    const stop = vi.fn();
    ipc.listen.mockResolvedValue(stop);
    const listener = vi.fn();
    const unsubscribe = await createTauriApi().subscribe(listener);
    const snapshot = await createDemoApi(0).getSnapshot();
    expect(ipc.listen).toHaveBeenCalledWith("snapshot-changed", expect.any(Function));
    ipc.listen.mock.calls[0][1]({ event: "snapshot-changed", id: 1, payload: snapshot });
    expect(listener).toHaveBeenCalledWith(snapshot);
    unsubscribe();
    expect(stop).toHaveBeenCalledOnce();
  });

  it("nel browser comunica unsupportedPlatform senza invocare IPC o attivare demo", async () => {
    ipc.isTauri.mockReturnValue(false);
    const api = createTauriApi();
    const snapshot = await api.getSnapshot();
    expect(snapshot).toMatchObject({ platform: "unsupportedPlatform", demo: false, devices: [], audio: { available: false } });
    expect(snapshot.diagnostics[0].message).toMatch(/Tauri backend|backend Tauri/);
    await expect(api.setDpi("mouse", 800, 800)).rejects.toMatchObject({ code: "unsupportedPlatform" });
    await expect(api.applyMouseRemapping("mouse", [], false, false)).rejects.toMatchObject({ code: "unsupportedPlatform" });
    await expect(api.disableMouseRemapping("mouse")).rejects.toMatchObject({ code: "unsupportedPlatform" });
    await api.subscribe(() => {});
    expect(ipc.invoke).not.toHaveBeenCalled();
    expect(ipc.listen).not.toHaveBeenCalled();
  });

  it("conserva l'errore del backend e non ripiega su dati demo", async () => {
    const failure = { code: "providerMissing", message: "OpenRazer non disponibile.", detail: "Session bus non raggiungibile." };
    ipc.invoke.mockRejectedValue(failure);
    const api = createTauriApi();
    await expect(api.getSnapshot()).rejects.toEqual(failure);
    expect(api.demo).toBe(false);
  });

  it("la demo richiede esattamente VITE_DEMO=true e accetta query 0/1/3", async () => {
    ipc.isTauri.mockReturnValue(false);
    vi.stubEnv("VITE_DEMO", "1");
    expect((await createHubApi().getSnapshot()).demo).toBe(false);
    vi.stubEnv("VITE_DEMO", "true");
    for (const count of [0, 1, 3]) {
      window.history.replaceState(null, "", `/?devices=${count}`);
      const snapshot = await createHubApi().getSnapshot();
      expect(snapshot.demo).toBe(true);
      expect(snapshot.devices).toHaveLength(count);
    }
    window.history.replaceState(null, "", "/?devices=2");
    expect((await createHubApi().getSnapshot()).devices).toHaveLength(3);
  });
});

describe("adapter demo esplicito", () => {
  it("fornisce istanze dinamiche univoche e non inventa batterie Logitech/cuffie", async () => {
    const snapshot = await createDemoApi(6).getSnapshot();
    expect(snapshot.devices).toHaveLength(6);
    expect(new Set(snapshot.devices.map((device) => device.id)).size).toBe(6);
    expect(new Set(snapshot.devices.map((device) => device.identity.serial)).size).toBe(6);
    for (const device of snapshot.devices.filter((entry) => entry.kind === "headset" || entry.brand === "logitech")) expect(device.values.battery).toBeNull();
    expect(snapshot.devices[0].values.battery?.charging).toBe(false);
    expect(snapshot.devices[0].values.battery?.observedAt).toBeLessThanOrEqual(Math.floor(Date.now() / 1000));
    expect(snapshot.devices[0].values.battery?.observedAt).toBeGreaterThan(Math.floor(Date.now() / 1000) - 5);
    expect(snapshot.devices[1]).toMatchObject({ provider: "usb", support: "partial", capabilities: { dpi: null, polling: null } });
  });

  it("valida range e liste DPI, assi e liste polling senza accettare frequenze universali", async () => {
    const api = createDemoApi();
    const initial = await api.getSnapshot();
    const viper = initial.devices[0];
    const g305 = initial.devices[2];
    await expect(api.setDpi(viper.id, 850, 850)).rejects.toMatchObject({ code: "demoValidation" });
    await expect(api.setPollingRate(viper.id, 250)).rejects.toMatchObject({ code: "demoValidation" });
    await expect(api.setDpi(g305.id, 1600, 3200)).rejects.toMatchObject({ code: "demoValidation" });
    await expect(api.setDpi(g305.id, 1500, 1500)).rejects.toMatchObject({ code: "demoValidation" });
    expect((await api.getSnapshot()).revision).toBe(initial.revision);
    const dpiResult = await api.setDpi(viper.id, 1200, 1600);
    expect(dpiResult.devices[0].values.dpi).toEqual({ x: 1200, y: 1600 });
    const rateResult = await api.setPollingRate(g305.id, 250);
    expect(rateResult.devices[2].values.pollingRate).toBe(250);
    expect(rateResult.revision).toBe(initial.revision + 2);
  });

  it("emette snapshot indipendenti e valida volume/mute sul nodo reale della demo", async () => {
    const api = createDemoApi(0);
    const events: Snapshot[] = [];
    const unsubscribe = await api.subscribe((snapshot) => events.push(snapshot));
    const initial = await api.getSnapshot();
    const microphone = initial.audio.inputs[0];
    await api.setVolume(microphone.nodeName, 0.35);
    await api.setMute(microphone.nodeName, true);
    expect(events).toHaveLength(2);
    expect(events[0].audio.inputs[0].muted).toBe(false);
    expect(events[1].audio.inputs[0]).toMatchObject({ volume: 0.35, muted: true });
    await expect(api.setVolume(microphone.nodeName, 1.5)).rejects.toMatchObject({ code: "demoValidation" });
    await expect(api.setMute("nodo-assente", true)).rejects.toMatchObject({ code: "demoValidation" });
    unsubscribe();
    await api.refreshDevices();
    expect(events).toHaveLength(2);
  });

  it("salvare un profilo non lo applica, e il volume 7.1 non aumenta il volume fisico", async () => {
    const api = createDemoApi();
    const initial = await api.getSnapshot();
    const target = initial.audio.outputs.find((node) => node.nodeName === initial.audio.selectedOutput)!;
    const profile: AudioProfile = { ...initial.profiles[0], id: "f20b029d-71cb-4c2c-a357-49a3068d8a8e", name: "Spaziale", volume: 0.95, surroundRequested: true };
    const saved = await api.saveAudioProfile(profile);
    expect(saved.audio.surroundState).toBe("off");
    expect(saved.audio.outputs.find((node) => node.nodeName === target.nodeName)?.volume).toBe(target.volume);
    const applied = await api.applyAudioProfile(profile.id);
    expect(applied.audio.surroundState).toBe("on");
    expect(applied.audio.outputs.find((node) => node.nodeName === target.nodeName)?.volume).toBe(target.volume);
    expect(applied.audio.outputs.find((node) => node.nodeName === applied.audio.defaultOutput)).toMatchObject({ isVirtual: true, volume: 0.95 });
    const off = await api.setSurround(false);
    expect(off.audio.outputs.every((node) => !node.isVirtual)).toBe(true);
    expect(off.audio.defaultOutput).toBe(target.nodeName);
  });

  it("riconosce solo DPI effettivamente ammessi, inclusi valori fuori elenco", () => {
    expect(isDpiValueAllowed({ kind: "discrete", values: [400, 900, 1800] }, 900)).toBe(true);
    expect(isDpiValueAllowed({ kind: "discrete", values: [400, 900, 1800] }, 800)).toBe(false);
    expect(isDpiValueAllowed({ kind: "range", min: 100, max: 30000, step: 100 }, 30000)).toBe(true);
    expect(isDpiValueAllowed({ kind: "range", min: 100, max: 30000, step: 100 }, 30001)).toBe(false);
    expect(isDpiValueAllowed({ kind: "unknown" }, 800)).toBe(false);
  });

  it.each(["", " ", "abc", "0", "-800", "800.5", "1e3", "800 DPI", "9007199254740993"])("non converte la bozza DPI invalida %j in un numero applicabile", (draft) => {
    expect(parseDpiDraft(draft)).toBeNull();
  });

  it("interpreta interi DPI senza arrotondare o normalizzare la bozza", () => {
    expect(parseDpiDraft("1200")).toBe(1200);
    expect(parseDpiDraft("00800")).toBe(800);
    expect(isDpiValueAllowed({ kind: "range", min: 100, max: 30000, step: 100 }, parseDpiDraft("850"))).toBe(false);
  });

  it("applica e ripristina la rimappatura solo nella demo isolata, conservando la preferenza nel readback", async () => {
    const api = createDemoApi(1);
    const initial = await api.getSnapshot();
    const device = initial.devices[0];
    const bindings: MouseBinding[] = [
      { buttonId: "button-275", action: { kind: "chord", modifiers: [29, 42], key: 30 } },
      { buttonId: "button-276", action: { kind: "key", code: 164 } },
    ];
    const events: Snapshot[] = [];
    const stop = await api.subscribe((snapshot) => events.push(snapshot));
    const result = await api.applyMouseRemapping(device.id, bindings, true, false);
    expect(result.devices[0].values.remapping).toMatchObject({ state: "active", enabled: true, autoApply: true, pausedForSafety: false, bindings });
    expect(result.devices[0].values.remapping?.activePreset).not.toBeNull();
    bindings[0].action = { kind: "disabled" };
    result.devices[0].values.remapping!.bindings = [];
    expect((await api.getSnapshot()).devices[0].values.remapping?.bindings[0].action).toEqual({ kind: "chord", modifiers: [29, 42], key: 30 });
    expect((await createDemoApi(1).getSnapshot()).devices[0].values.remapping).toMatchObject({ state: "inactive", enabled: false, autoApply: false });
    const restored = await api.disableMouseRemapping(device.id);
    expect(restored.devices[0].values.remapping).toMatchObject({ state: "inactive", enabled: false, autoApply: false, activePreset: null });
    expect(events).toHaveLength(2);
    expect(events[0].devices[0].values.remapping?.enabled).toBe(true);
    expect(ipc.invoke).not.toHaveBeenCalled();
    expect(ipc.listen).not.toHaveBeenCalled();
    stop();
  });

  it("rifiuta sorgenti sconosciute, target non consentiti, chord invalidi e perdita del clic sinistro", async () => {
    const api = createDemoApi(1);
    const initial = await api.getSnapshot();
    const invalid: MouseBinding[][] = [
      [{ buttonId: "not-detected", action: { kind: "key", code: 30 } }],
      [{ buttonId: "button-275", action: { kind: "key", code: 9999 } }],
      [{ buttonId: "button-275", action: { kind: "key", code: 272 } }],
      [{ buttonId: "button-275", action: { kind: "mouseButton", code: 30 } }],
      [{ buttonId: "button-275", action: { kind: "chord", modifiers: [], key: 30 } }],
      [{ buttonId: "button-275", action: { kind: "chord", modifiers: [30], key: 31 } }],
      [{ buttonId: "button-275", action: { kind: "chord", modifiers: [29, 29], key: 30 } }],
      [{ buttonId: "button-275", action: { kind: "chord", modifiers: [29], key: 42 } }],
      [{ buttonId: "button-272", action: { kind: "disabled" } }],
      [{ buttonId: "button-275", action: { kind: "disabled" } }, { buttonId: "button-275", action: { kind: "default" } }],
      [{ buttonId: "button-275", action: { kind: "macro", expression: "arbitrary DSL" } } as unknown as MouseBinding],
    ];
    for (const bindings of invalid) await expect(api.applyMouseRemapping(initial.devices[0].id, bindings, false, false)).rejects.toMatchObject({ code: "demoValidation" });
    expect((await api.getSnapshot()).revision).toBe(initial.revision);
    const result = await api.applyMouseRemapping(initial.devices[0].id, [
      { buttonId: "button-272", action: { kind: "disabled" } },
      { buttonId: "button-275", action: { kind: "mouseButton", code: 272 } },
    ], false, false);
    expect(result.devices[0].values.remapping?.enabled).toBe(true);
  });

  it("non acquisisce il mouse per default/passthrough e non inventa target dalle capabilities", async () => {
    const api = createDemoApi(1);
    const initial = await api.getSnapshot();
    const device = initial.devices[0];
    const result = await api.applyMouseRemapping(device.id, [{ buttonId: "button-272", action: { kind: "default" } }], false, false);
    expect(result.devices[0].values.remapping).toMatchObject({ state: "inactive", enabled: false, activePreset: null });
    const capability = device.capabilities.remapping!;
    expect(remappingProblem({ ...capability, keyCodes: [29, 30] }, [{ buttonId: "button-275", action: { kind: "mouseButton", code: 272 } }], false, false)).toMatch(/allowed by the provider|target consentiti/);
  });

  it("valida identita debole e consenso indipendentemente da DPI o polling scrivibili", async () => {
    const device = (await createDemoApi(1).getSnapshot()).devices[0];
    device.capabilities.dpi = null;
    device.capabilities.polling = null;
    const capability = { ...device.capabilities.remapping!, identityStrength: "port" as const };
    const bindings: MouseBinding[] = [{ buttonId: "button-275", action: { kind: "key", code: 30 } }];
    expect(remappingProblem(capability, bindings, true, false)).toMatch(/explicit consent|consenso esplicito/);
    expect(remappingProblem(capability, bindings, true, true)).toBeNull();
    expect(remappingProblem(capability, bindings, false, false)).toBeNull();
    expect(remappingProblem({ ...capability, fingerprint: null }, bindings, true, true)).toMatch(/not verified|non verificata/);
    expect(remappingProblem({ ...capability, writable: false }, bindings, false, false)).not.toBeNull();
    expect(remappingProblem({ ...capability, availability: "unsupportedPlatform" }, bindings, false, false)).not.toBeNull();
  });

  it("importare una HRTF disattiva il filtro e non riattiva effetti o cambia il volume fisico", async () => {
    const api = createDemoApi();
    const on = await api.setSurround(true);
    const target = on.audio.outputs.find((node) => node.nodeName === on.audio.selectedOutput)!;
    const imported = await api.importHrtf();
    expect(imported.audio.surroundState).toBe("off");
    expect(imported.audio.outputs.every((node) => !node.isVirtual)).toBe(true);
    expect(imported.audio.selectedOutput).toBe(target.nodeName);
    expect(imported.audio.outputs.find((node) => node.nodeName === target.nodeName)?.volume).toBe(target.volume);
    expect(imported.audio.hrtf?.reference).toMatch(/^[0-9a-f-]{36}\.sofa$/);
    const previousProfile = await api.applyAudioProfile(imported.profiles[0].id);
    expect(previousProfile.audio.hrtf?.reference).toBe("system");
  });
});
