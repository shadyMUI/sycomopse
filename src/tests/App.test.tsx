import { StrictMode } from "react";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import App from "../App";
import { createDemoApi, createTauriApi } from "../api";
import type { HubApi, Snapshot } from "../types";

function clone(snapshot: Snapshot): Snapshot { return JSON.parse(JSON.stringify(snapshot)) as Snapshot; }

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

function controlledApi(initial: Snapshot) {
  let current = clone(initial);
  const listeners = new Set<(snapshot: Snapshot) => void>();
  const api: HubApi = {
    ...createDemoApi(), demo: initial.demo,
    getSnapshot: vi.fn(async () => clone(current)),
    subscribe: vi.fn(async (listener: (snapshot: Snapshot) => void) => {
      listeners.add(listener);
      return () => { listeners.delete(listener); };
    }),
  };
  return { api, emit(next: Snapshot) { current = clone(next); for (const listener of listeners) listener(clone(next)); } };
}

async function dashboard(api: HubApi) {
  render(<App api={api} />);
  await screen.findByRole("heading", { name: /Il tuo setup/ });
}

describe("dashboard dinamica e confine demo", () => {
  it.each([0, 1, 3, 5])("mostra esattamente %i card dall'array, senza dispositivi hardcoded", async (count) => {
    await dashboard(createDemoApi(count));
    expect(screen.queryAllByTestId("device-card")).toHaveLength(count);
    expect(screen.getByText("Demo / nessun hardware collegato")).toBeVisible();
    if (count === 0) expect(screen.getByRole("heading", { name: "Nessun dispositivo rilevato" })).toBeVisible();
  });

  it("mantiene il badge demo in tutte le viste e naviga a tastiera", async () => {
    const user = userEvent.setup();
    await dashboard(createDemoApi());
    await user.click(screen.getByRole("tab", { name: /Mouse/ }));
    expect(screen.getByText("Demo / nessun hardware collegato")).toBeVisible();
    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("tab", { name: /Audio/ })).toHaveFocus();
    expect(screen.getByRole("tab", { name: /Audio/ })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByText("Demo / nessun hardware collegato")).toBeVisible();
    await user.keyboard("{Home}");
    expect(screen.getByRole("tab", { name: /Panoramica/ })).toHaveFocus();
  });

  it("nel browser normale mostra il backend mancante e mai un fallback demo", async () => {
    await dashboard(createTauriApi());
    expect(screen.getByRole("heading", { name: "Piattaforma non supportata" })).toBeVisible();
    expect(screen.getByText("Backend non disponibile")).toBeVisible();
    expect(screen.queryByText("Demo / nessun hardware collegato")).not.toBeInTheDocument();
    expect(screen.queryAllByTestId("device-card")).toHaveLength(0);
    expect(screen.getByRole("switch", { name: "Surround 7.1" })).toBeDisabled();
  });

  it("apre le schede dalle card senza cambiare il default audio", async () => {
    const user = userEvent.setup();
    const api = createDemoApi();
    const setOutput = vi.spyOn(api, "setDefaultOutput");
    await dashboard(api);
    await user.click(screen.getByRole("button", { name: "Apri controlli di Razer BlackShark V3 X HyperSpeed" }));
    expect(screen.getByRole("tab", { name: /Audio/ })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("tabpanel", { name: /Audio/ })).toHaveFocus();
    expect(screen.getByText("Supporto parziale")).toBeVisible();
    expect(screen.getByText("Batteria non esposta")).toBeVisible();
    expect(screen.queryByText("78%")).not.toBeInTheDocument();
    expect(setOutput).not.toHaveBeenCalled();
  });
});

describe("controlli mouse e readback", () => {
  it.each(["", "abc", "0", "99", "30001", "850", "800.5", "1e3"])("conserva DPI digitati %j senza IPC e disabilita solo Applica DPI", async (value) => {
    const user = userEvent.setup();
    const api = createDemoApi(1);
    const setDpi = vi.spyOn(api, "setDpi");
    const setRate = vi.spyOn(api, "setPollingRate");
    await dashboard(api);
    await user.click(screen.getByRole("tab", { name: /Mouse/ }));
    const manual = screen.getByRole("textbox", { name: "DPI manuali" });
    fireEvent.change(manual, { target: { value } });
    expect(manual).toHaveValue(value);
    expect(manual).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByRole("button", { name: "Applica DPI" })).toBeDisabled();
    expect(screen.getByRole("switch", { name: "Collega assi X/Y" })).toBeEnabled();
    await user.click(screen.getByRole("button", { name: /500\s*Hz/ }));
    expect(screen.getByRole("button", { name: "Applica polling" })).toBeEnabled();
    expect(setDpi).not.toHaveBeenCalled();
    expect(setRate).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Applica DPI" }));
    expect(setDpi).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Applica polling" }));
    expect(setRate).toHaveBeenCalledWith("demo-viper-1", 500);
    expect(manual).toHaveValue(value);
    expect(screen.getByText("800 DPI", { selector: "strong" })).toBeVisible();
  });

  it("sincronizza DPI manuali e slider compatti, reset e readback senza invii digitando", async () => {
    const user = userEvent.setup();
    const api = createDemoApi(1);
    const setDpi = vi.spyOn(api, "setDpi");
    await dashboard(api);
    const manual = screen.getByRole("textbox", { name: "DPI manuali" });
    expect(manual).toHaveValue("800");
    await user.clear(manual);
    await user.type(manual, "1200");
    expect(screen.getByRole("slider", { name: "DPI desiderati" })).toHaveValue("1200");
    expect(setDpi).not.toHaveBeenCalled();
    fireEvent.change(screen.getByRole("slider", { name: "DPI desiderati" }), { target: { value: "1600" } });
    expect(manual).toHaveValue("1600");
    await user.click(screen.getByRole("button", { name: "Ripristina bozza" }));
    expect(manual).toHaveValue("800");
    await user.clear(manual);
    await user.type(manual, "1200");
    await user.click(screen.getByRole("button", { name: "Applica DPI" }));
    expect(setDpi).toHaveBeenCalledWith("demo-viper-1", 1200, 1200);
    await waitFor(() => expect(screen.getByText("1200 DPI", { selector: "strong" })).toBeVisible());
    expect(manual).toHaveValue("1200");
    expect(screen.getByRole("button", { name: "Applica DPI" })).toBeDisabled();
  });

  it("conserva la bozza manuale discreta fuori elenco e accetta soltanto i valori esposti", async () => {
    const user = userEvent.setup();
    const api = createDemoApi();
    const setDpi = vi.spyOn(api, "setDpi");
    await dashboard(api);
    await user.selectOptions(screen.getByLabelText("Mouse dei controlli rapidi"), "demo-g305-1");
    const manual = screen.getByRole("textbox", { name: "DPI manuali" });
    await user.clear(manual);
    await user.type(manual, "1500");
    expect(manual).toHaveValue("1500");
    expect(screen.getByRole("button", { name: "Applica DPI" })).toBeDisabled();
    expect(screen.getByRole("combobox", { name: "Scegli DPI desiderati" })).toBeVisible();
    await user.clear(manual);
    await user.type(manual, "3200");
    expect(screen.getByRole("slider", { name: "DPI desiderati" })).toHaveValue("3");
    expect(setDpi).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Applica DPI" }));
    expect(setDpi).toHaveBeenCalledWith("demo-g305-1", 3200, 3200);
  });

  it("mantiene bozze stringa X/Y, invalidita e reset di entrambi gli assi", async () => {
    const user = userEvent.setup();
    const api = createDemoApi(1);
    const setDpi = vi.spyOn(api, "setDpi");
    await dashboard(api);
    await user.click(screen.getByRole("tab", { name: /Mouse/ }));
    await user.click(screen.getByRole("switch", { name: "Collega assi X/Y" }));
    const x = screen.getByRole("textbox", { name: "DPI X manuali" });
    const y = screen.getByRole("textbox", { name: "DPI Y manuali" });
    fireEvent.change(x, { target: { value: "1200" } });
    fireEvent.change(y, { target: { value: "" } });
    expect(x).toHaveValue("1200");
    expect(y).toHaveValue("");
    expect(screen.getByRole("button", { name: "Applica DPI" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Ripristina bozza" }));
    expect(x).toHaveValue("800");
    expect(y).toHaveValue("800");
    fireEvent.change(x, { target: { value: "1200" } });
    fireEvent.change(y, { target: { value: "1600" } });
    expect(setDpi).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Applica DPI" }));
    expect(setDpi).toHaveBeenCalledWith("demo-viper-1", 1200, 1600);
    await waitFor(() => expect(screen.getByText("X 1200 / Y 1600 DPI", { selector: "strong" })).toBeVisible());
    await user.click(screen.getByRole("switch", { name: "Collega assi X/Y" }));
    expect(screen.getByRole("textbox", { name: "DPI manuali" })).toHaveValue("1200");
    await user.click(screen.getByRole("button", { name: "Applica DPI" }));
    expect(setDpi).toHaveBeenLastCalledWith("demo-viper-1", 1200, 1200);
  });

  it("riconcilia le bozze DPI con readback recenti e resetta gli invalidi cambiando mouse", async () => {
    const user = userEvent.setup();
    const initial = await createDemoApi().getSnapshot();
    const { api, emit } = controlledApi(initial);
    await dashboard(api);
    const manual = screen.getByRole("textbox", { name: "DPI manuali" });
    fireEvent.change(manual, { target: { value: "bad" } });
    const next = clone(initial);
    next.revision = 2;
    next.devices[0].values.dpi = { x: 1800, y: 1800 };
    act(() => emit(next));
    expect(manual).toHaveValue("1800");
    expect(screen.getByRole("slider", { name: "DPI desiderati" })).toHaveValue("1800");
    fireEvent.change(manual, { target: { value: "" } });
    await user.selectOptions(screen.getByLabelText("Mouse dei controlli rapidi"), "demo-g305-1");
    expect(screen.getByRole("textbox", { name: "DPI manuali" })).toHaveValue("1600");
  });

  it("azzera la bozza cambiando mouse e applica DPI discreti per posizione, non per range", async () => {
    const user = userEvent.setup();
    const api = createDemoApi();
    const setDpi = vi.spyOn(api, "setDpi");
    const setRate = vi.spyOn(api, "setPollingRate");
    await dashboard(api);
    await user.click(screen.getByRole("button", { name: "Apri controlli di Razer Viper V3 HyperSpeed" }));
    const viperSlider = screen.getByRole("slider", { name: "DPI desiderati" });
    fireEvent.change(viperSlider, { target: { value: "1200" } });
    expect(setDpi).not.toHaveBeenCalled();
    await user.selectOptions(screen.getByLabelText("Mouse selezionato"), "demo-g305-1");
    const slider = screen.getByRole("slider", { name: "DPI desiderati" });
    expect(slider).toHaveValue("2");
    expect(slider).toHaveAttribute("min", "0");
    expect(slider).toHaveAttribute("max", "5");
    expect(slider).toHaveAttribute("aria-valuetext", "1600 DPI");
    expect(screen.getByRole("button", { name: "Applica DPI" })).toBeDisabled();
    fireEvent.change(slider, { target: { value: "3" } });
    expect(slider).toHaveAttribute("aria-valuetext", "3200 DPI");
    expect(setDpi).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Applica DPI" }));
    expect(setDpi).toHaveBeenCalledWith("demo-g305-1", 3200, 3200);
    await waitFor(() => expect(screen.getByText("3200 DPI", { selector: "strong" })).toBeVisible());
    await user.click(screen.getByRole("button", { name: /250\s*Hz/ }));
    expect(setRate).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Applica polling" }));
    expect(setRate).toHaveBeenCalledWith("demo-g305-1", 250);
    expect(screen.queryByRole("switch", { name: "Collega assi X/Y" })).not.toBeInTheDocument();
  });

  it("collega X/Y inizialmente e li separa solo quando il provider lo supporta", async () => {
    const user = userEvent.setup();
    const api = createDemoApi(1);
    const setDpi = vi.spyOn(api, "setDpi");
    await dashboard(api);
    await user.click(screen.getByRole("tab", { name: /Mouse/ }));
    const link = screen.getByRole("switch", { name: "Collega assi X/Y" });
    expect(link).toBeChecked();
    await user.click(link);
    fireEvent.change(screen.getByRole("slider", { name: "DPI X desiderati" }), { target: { value: "1200" } });
    fireEvent.change(screen.getByRole("slider", { name: "DPI Y desiderati" }), { target: { value: "1600" } });
    await user.click(screen.getByRole("button", { name: "Applica DPI" }));
    expect(setDpi).toHaveBeenCalledWith("demo-viper-1", 1200, 1600);
  });

  it("un valore letto fuori elenco resta in sola lettura finche viene scelto un DPI valido", async () => {
    const user = userEvent.setup();
    const initial = await createDemoApi().getSnapshot();
    initial.devices[2].values.dpi = { x: 1500, y: 1500 };
    const { api } = controlledApi(initial);
    const setDpi = vi.spyOn(api, "setDpi");
    await dashboard(api);
    await user.selectOptions(screen.getByLabelText("Mouse dei controlli rapidi"), "demo-g305-1");
    expect(screen.queryByRole("slider", { name: "DPI desiderati" })).not.toBeInTheDocument();
    expect(screen.getByText("1500 DPI", { selector: "strong" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Applica DPI" })).toBeDisabled();
    await user.selectOptions(screen.getByRole("combobox", { name: "Scegli DPI desiderati" }), "3200");
    expect(screen.getByRole("slider", { name: "DPI desiderati" })).toHaveValue("3");
    await user.click(screen.getByRole("button", { name: "Applica DPI" }));
    expect(setDpi).toHaveBeenCalledWith("demo-g305-1", 3200, 3200);
  });

  it("un provider mancante limita solo il mouse e non inventa una batteria", async () => {
    const user = userEvent.setup();
    const initial = await createDemoApi(1).getSnapshot();
    initial.devices[0].availability = "providerMissing";
    initial.devices[0].values.battery = null;
    initial.devices[0].capabilities.dpi!.writable = false;
    initial.devices[0].capabilities.dpi!.reason = "OpenRazer non attivo.";
    initial.diagnostics[0] = { ...initial.diagnostics[0], code: "providerMissing", severity: "warning", message: "OpenRazer non attivo." };
    const { api } = controlledApi(initial);
    await dashboard(api);
    expect(screen.getAllByText("Provider assente")[0]).toBeVisible();
    expect(screen.getByRole("switch", { name: "Surround 7.1" })).toBeEnabled();
    await user.click(screen.getByLabelText("Servizi e diagnostica"));
    expect(screen.getByText("OpenRazer non attivo.", { selector: ".diagnostic-item p" })).toBeVisible();
    await user.click(screen.getByRole("tab", { name: /Mouse/ }));
    expect(screen.getByRole("slider", { name: "DPI desiderati" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Applica DPI" })).toBeDisabled();
    expect(screen.getByText("Batteria non esposta")).toBeVisible();
    expect(screen.queryByText("In carica")).not.toBeInTheDocument();
  });

  it("espone una batteria coarse/stale come livello approssimativo e fonte, senza percentuale inventata", async () => {
    const user = userEvent.setup();
    const initial = await createDemoApi(1).getSnapshot();
    const now = Math.floor(Date.now() / 1000);
    initial.devices[0].values.battery = { percentage: null, level: "low", charging: null, source: "upower", observedAt: now, sourceUpdatedAt: now - 900, stale: true };
    await dashboard(controlledApi(initial).api);
    await user.click(screen.getByRole("tab", { name: /Mouse/ }));
    expect(screen.getByText("Basso")).toBeVisible();
    expect(screen.getByText("Livello approssimativo")).toBeVisible();
    expect(screen.getByText("Dato precedente")).toBeVisible();
    expect(screen.getByText("Fonte: UPower")).toBeVisible();
    expect(screen.getByText(new Date((now - 900) * 1000).toLocaleString("it-IT", { day: "2-digit", month: "2-digit", hour: "2-digit", minute: "2-digit" }))).toBeVisible();
    expect(screen.queryByRole("meter")).not.toBeInTheDocument();
    expect(screen.queryByText("25%")).not.toBeInTheDocument();
  });

  it("conserva la lettura confermata su errore DPI e riconcilia la snapshot", async () => {
    const user = userEvent.setup();
    const api = createDemoApi(1);
    const getSnapshot = vi.spyOn(api, "getSnapshot");
    vi.spyOn(api, "setDpi").mockRejectedValue({ code: "writeFailed", message: "Scrittura DPI non confermata.", detail: null });
    await dashboard(api);
    fireEvent.change(screen.getByRole("slider", { name: "DPI desiderati" }), { target: { value: "1200" } });
    await user.click(screen.getByRole("button", { name: "Applica DPI" }));
    await screen.findByRole("alert");
    expect(screen.getByText("800 DPI", { selector: "strong" })).toBeVisible();
    expect(getSnapshot).toHaveBeenCalledTimes(2);
  });
});

describe("pulsanti rilevati, bozze e privacy", () => {
  async function buttonsView(api: HubApi) {
    const user = userEvent.setup();
    await dashboard(api);
    await user.click(screen.getByRole("tab", { name: /Mouse/ }));
    return { user, panel: within(screen.getByRole("region", { name: /Pulsanti rilevati/ })) };
  }

  it("rimappa un mouse generico senza DPI scrivibili e usa solo sorgenti e target rilevati", async () => {
    const initial = await createDemoApi(1).getSnapshot();
    const device = initial.devices[0];
    device.name = "Mouse generico";
    device.brand = "generic";
    device.provider = "inputremapper";
    device.availability = "providerMissing";
    device.support = "unsupported";
    device.capabilities.dpi = null;
    device.capabilities.polling = null;
    device.values.dpi = null;
    device.values.pollingRate = null;
    device.values.battery = null;
    device.capabilities.remapping!.buttons = device.capabilities.remapping!.buttons.filter((button) => [272, 275].includes(button.code));
    device.capabilities.remapping!.keyCodes = [272, 30];
    const { api } = controlledApi(initial);
    const result = clone(initial);
    result.revision = 2;
    result.devices[0].values.remapping = { ...device.values.remapping!, state: "active", enabled: true, bindings: [{ buttonId: "button-275", action: { kind: "key", code: 30 } }], profileId: "generic-profile", activePreset: "generic-preset" };
    const apply = vi.spyOn(api, "applyMouseRemapping").mockResolvedValue(result);
    const setDpi = vi.spyOn(api, "setDpi");
    const { user, panel } = await buttonsView(api);
    expect(screen.getByRole("textbox", { name: "DPI manuali" })).toHaveAttribute("readonly");
    expect(screen.queryByRole("slider", { name: "DPI desiderati" })).not.toBeInTheDocument();
    expect(panel.getAllByRole("combobox", { name: /^Azione per/ })).toHaveLength(2);
    expect(panel.queryByRole("combobox", { name: "Azione per Clic destro" })).not.toBeInTheDocument();
    await user.selectOptions(panel.getByRole("combobox", { name: "Azione per Laterale" }), "key");
    const target = panel.getByRole("combobox", { name: "Tasto per Laterale" });
    expect(within(target).getAllByRole("option").map((option) => option.getAttribute("value"))).toEqual(["", "30"]);
    expect(panel.getByRole("button", { name: "Applica pulsanti" })).toBeDisabled();
    await user.selectOptions(target, "30");
    expect(panel.getByRole("button", { name: "Applica pulsanti" })).toBeEnabled();
    expect(apply).not.toHaveBeenCalled();
    await user.click(panel.getByRole("button", { name: "Applica pulsanti" }));
    expect(apply).toHaveBeenCalledWith(device.id, [{ buttonId: "button-275", action: { kind: "key", code: 30 } }], false, false);
    expect(setDpi).not.toHaveBeenCalled();
    await waitFor(() => expect(panel.getByText("Attiva", { selector: "strong" })).toBeVisible());
  });

  it("compila clic, multimedia, disabilitato e default senza DSL o scritture durante la bozza", async () => {
    const api = createDemoApi(1);
    const apply = vi.spyOn(api, "applyMouseRemapping");
    const { user, panel } = await buttonsView(api);
    await user.selectOptions(panel.getByRole("combobox", { name: "Azione per Laterale" }), "mouseButton");
    await user.selectOptions(panel.getByRole("combobox", { name: "Clic per Laterale" }), "273");
    await user.selectOptions(panel.getByRole("combobox", { name: "Azione per Laterale extra" }), "disabled");
    expect(apply).not.toHaveBeenCalled();
    await user.click(panel.getByRole("button", { name: "Applica pulsanti" }));
    expect(apply).toHaveBeenLastCalledWith("demo-viper-1", [
      { buttonId: "button-275", action: { kind: "mouseButton", code: 273 } },
      { buttonId: "button-276", action: { kind: "disabled" } },
    ], false, false);
    await waitFor(() => expect(panel.getByText("Attiva", { selector: "strong" })).toBeVisible());
    await user.selectOptions(panel.getByRole("combobox", { name: "Azione per Laterale" }), "media");
    await user.selectOptions(panel.getByRole("combobox", { name: "Multimedia per Laterale" }), "164");
    await user.click(panel.getByRole("button", { name: "Applica pulsanti" }));
    expect(apply).toHaveBeenLastCalledWith("demo-viper-1", [
      { buttonId: "button-275", action: { kind: "key", code: 164 } },
      { buttonId: "button-276", action: { kind: "disabled" } },
    ], false, false);
    await user.selectOptions(panel.getByRole("combobox", { name: "Azione per Laterale" }), "default");
    await user.click(panel.getByRole("button", { name: "Applica pulsanti" }));
    expect(apply).toHaveBeenLastCalledWith("demo-viper-1", [{ buttonId: "button-276", action: { kind: "disabled" } }], false, false);
  });

  it("non consente di perdere tutte le vie al clic sinistro, ma permette di spostarlo", async () => {
    const api = createDemoApi(1);
    const apply = vi.spyOn(api, "applyMouseRemapping");
    const { user, panel } = await buttonsView(api);
    await user.selectOptions(panel.getByRole("combobox", { name: "Azione per Clic sinistro" }), "disabled");
    expect(panel.getByText(/Mantieni almeno una via al clic sinistro/)).toBeVisible();
    expect(panel.getByRole("button", { name: "Applica pulsanti" })).toBeDisabled();
    await user.click(panel.getByRole("button", { name: "Applica pulsanti" }));
    expect(apply).not.toHaveBeenCalled();
    await user.selectOptions(panel.getByRole("combobox", { name: "Azione per Laterale" }), "mouseButton");
    await user.selectOptions(panel.getByRole("combobox", { name: "Clic per Laterale" }), "272");
    expect(panel.getByRole("button", { name: "Applica pulsanti" })).toBeEnabled();
    await user.click(panel.getByRole("button", { name: "Applica pulsanti" }));
    expect(apply).toHaveBeenCalledWith("demo-viper-1", [
      { buttonId: "button-272", action: { kind: "disabled" } }, { buttonId: "button-275", action: { kind: "mouseButton", code: 272 } },
    ], false, false);
  });

  it("acquisisce una scorciatoia solo nel recorder focalizzato, senza listener keyboard su document/window o IPC", async () => {
    const api = createDemoApi(1);
    const apply = vi.spyOn(api, "applyMouseRemapping");
    const { user, panel } = await buttonsView(api);
    const windowListener = vi.spyOn(window, "addEventListener");
    const documentListener = vi.spyOn(document, "addEventListener");
    await user.selectOptions(panel.getByRole("combobox", { name: "Azione per Laterale" }), "chord");
    fireEvent.keyDown(window, { key: "c", code: "KeyC", ctrlKey: true });
    fireEvent.keyDown(document, { key: "c", code: "KeyC", ctrlKey: true });
    expect(panel.getByRole("combobox", { name: "Tasto per Laterale" })).toHaveValue("");
    const recorder = panel.getByRole("button", { name: "Acquisisci scorciatoia per Laterale" });
    await user.click(recorder);
    expect(recorder).toHaveFocus();
    fireEvent.keyDown(window, { key: "c", code: "KeyC", ctrlKey: true });
    expect(panel.getByRole("combobox", { name: "Tasto per Laterale" })).toHaveValue("");
    fireEvent.keyDown(recorder, { key: "c", code: "KeyC", ctrlKey: true, repeat: true });
    expect(panel.getByRole("combobox", { name: "Tasto per Laterale" })).toHaveValue("");
    fireEvent.keyDown(recorder, { key: "c", code: "KeyC", ctrlKey: true });
    expect(panel.getByRole("combobox", { name: "Tasto per Laterale" })).toHaveValue("46");
    expect(panel.getByRole("checkbox", { name: "Ctrl per Laterale" })).toBeChecked();
    expect(recorder).toHaveAttribute("aria-pressed", "false");
    expect(apply).not.toHaveBeenCalled();
    expect(windowListener.mock.calls.filter(([type]) => /^key/.test(type))).toHaveLength(0);
    expect(documentListener.mock.calls.filter(([type]) => /^key/.test(type))).toHaveLength(0);
    windowListener.mockRestore();
    documentListener.mockRestore();
    await user.click(panel.getByRole("button", { name: "Applica pulsanti" }));
    expect(apply).toHaveBeenCalledWith("demo-viper-1", [{ buttonId: "button-275", action: { kind: "chord", modifiers: [29], key: 46 } }], false, false);
  });

  it.each(["Escape", "Tab", "blur", "cancel"])("annulla il recorder con %s senza alterare la bozza o registrare altro", async (cancel) => {
    const api = createDemoApi(1);
    const apply = vi.spyOn(api, "applyMouseRemapping");
    const { user, panel } = await buttonsView(api);
    await user.selectOptions(panel.getByRole("combobox", { name: "Azione per Laterale" }), "chord");
    const recorder = panel.getByRole("button", { name: "Acquisisci scorciatoia per Laterale" });
    await user.click(recorder);
    if (cancel === "blur") await user.click(screen.getByRole("textbox", { name: "DPI manuali" }));
    else if (cancel === "cancel") await user.click(panel.getByRole("button", { name: "Annulla acquisizione per Laterale" }));
    else fireEvent.keyDown(recorder, { key: cancel, code: cancel });
    expect(recorder).toHaveAttribute("aria-pressed", "false");
    fireEvent.keyDown(recorder, { key: "v", code: "KeyV", ctrlKey: true });
    expect(panel.getByRole("combobox", { name: "Tasto per Laterale" })).toHaveValue("");
    expect(panel.getByRole("button", { name: "Applica pulsanti" })).toBeDisabled();
    expect(apply).not.toHaveBeenCalled();
  });

  it("salva autoApply solo con Applica e Ripristina originali lo disabilita nel readback", async () => {
    const api = createDemoApi(1);
    const apply = vi.spyOn(api, "applyMouseRemapping");
    const restore = vi.spyOn(api, "disableMouseRemapping");
    const { user, panel } = await buttonsView(api);
    await user.selectOptions(panel.getByRole("combobox", { name: "Azione per Laterale" }), "key");
    await user.selectOptions(panel.getByRole("combobox", { name: "Tasto per Laterale" }), "30");
    const autoApply = panel.getByRole("switch", { name: "Riattiva automaticamente" });
    await user.click(autoApply);
    expect(autoApply).toBeChecked();
    expect(panel.getByText(/Riattivazione automatica salvata: no/)).toBeVisible();
    expect(apply).not.toHaveBeenCalled();
    await user.click(panel.getByRole("button", { name: "Applica pulsanti" }));
    expect(apply).toHaveBeenCalledWith("demo-viper-1", [{ buttonId: "button-275", action: { kind: "key", code: 30 } }], true, false);
    await waitFor(() => expect(panel.getByText(/Riattivazione automatica salvata: si/)).toBeVisible());
    expect(autoApply).toBeChecked();
    await user.click(panel.getByRole("button", { name: "Ripristina originali" }));
    expect(restore).toHaveBeenCalledWith("demo-viper-1");
    await waitFor(() => expect(panel.getByText("Inattiva", { selector: "strong" })).toBeVisible());
    expect(autoApply).not.toBeChecked();
    expect(panel.getByText(/Riattivazione automatica salvata: no/)).toBeVisible();
  });

  it("richiede consenso esplicito per autoApply su identita debole e non lo trasferisce a un nuovo fingerprint", async () => {
    const initial = await createDemoApi(1).getSnapshot();
    initial.devices[0].capabilities.remapping!.identityStrength = "port";
    const { api, emit } = controlledApi(initial);
    const result = clone(initial);
    result.revision = 2;
    result.devices[0].values.remapping = { ...initial.devices[0].values.remapping!, state: "active", enabled: true, autoApply: true, profileId: "weak-profile", bindings: [{ buttonId: "button-275", action: { kind: "key", code: 30 } }] };
    const apply = vi.spyOn(api, "applyMouseRemapping").mockResolvedValue(result);
    const { user, panel } = await buttonsView(api);
    await user.selectOptions(panel.getByRole("combobox", { name: "Azione per Laterale" }), "key");
    await user.selectOptions(panel.getByRole("combobox", { name: "Tasto per Laterale" }), "30");
    expect(panel.getByRole("button", { name: "Applica pulsanti" })).toBeEnabled();
    await user.click(panel.getByRole("switch", { name: "Riattiva automaticamente" }));
    const consent = panel.getByRole("checkbox", { name: /Consento la riattivazione automatica/ });
    expect(consent).not.toBeChecked();
    expect(panel.getByRole("button", { name: "Applica pulsanti" })).toBeDisabled();
    await user.click(consent);
    await user.click(panel.getByRole("button", { name: "Applica pulsanti" }));
    expect(apply).toHaveBeenCalledWith("demo-viper-1", [{ buttonId: "button-275", action: { kind: "key", code: 30 } }], true, true);
    const moved = clone(result);
    moved.revision = 3;
    moved.devices[0].capabilities.remapping!.fingerprint = "other-port";
    act(() => emit(moved));
    const newPanel = within(screen.getByRole("region", { name: /Pulsanti rilevati/ }));
    expect(newPanel.getByRole("checkbox", { name: /Consento la riattivazione automatica/ })).not.toBeChecked();
    expect(newPanel.getByRole("button", { name: "Applica pulsanti" })).toBeDisabled();
  });

  it("preserva bozze su stato busy/paused ed errori, senza ottimismo o accoppiamento a DPI invalidi", async () => {
    const initial = await createDemoApi(1).getSnapshot();
    initial.devices[0].values.remapping = { ...initial.devices[0].values.remapping!, state: "active", enabled: true, profileId: "profile", bindings: [{ buttonId: "button-275", action: { kind: "key", code: 30 } }] };
    const { api, emit } = controlledApi(initial);
    const apply = vi.spyOn(api, "applyMouseRemapping").mockRejectedValue({ code: "busy", message: "Un altro remapper mantiene il grab.", detail: null });
    const getSnapshot = vi.spyOn(api, "getSnapshot");
    const { user, panel } = await buttonsView(api);
    await user.selectOptions(panel.getByRole("combobox", { name: "Tasto per Laterale" }), "31");
    fireEvent.change(screen.getByRole("textbox", { name: "DPI manuali" }), { target: { value: "bad" } });
    const next = clone(initial);
    next.revision = 2;
    next.devices[0].values.remapping!.state = "paused";
    next.devices[0].values.remapping!.pausedForSafety = true;
    act(() => emit(next));
    expect(panel.getByRole("combobox", { name: "Tasto per Laterale" })).toHaveValue("31");
    expect(panel.getByText("In pausa", { selector: "strong" })).toBeVisible();
    expect(panel.getByRole("button", { name: "Applica pulsanti" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Applica DPI" })).toBeDisabled();
    next.revision = 3;
    next.devices[0].values.remapping!.state = "busy";
    act(() => emit(next));
    await user.click(panel.getByRole("button", { name: "Applica pulsanti" }));
    await screen.findByRole("alert");
    expect(apply).toHaveBeenCalledOnce();
    expect(getSnapshot).toHaveBeenCalledTimes(2);
    expect(panel.getByRole("combobox", { name: "Tasto per Laterale" })).toHaveValue("31");
    expect(panel.getByText("Occupata", { selector: "strong" })).toBeVisible();
    expect(panel.getByText("Azione salvata: A")).toBeVisible();
    expect(screen.getByRole("textbox", { name: "DPI manuali" })).toHaveValue("bad");
  });

  it("attende il backend e ignora una risposta remap stale dopo un readback piu recente", async () => {
    const initial = await createDemoApi(1).getSnapshot();
    const { api, emit } = controlledApi(initial);
    const write = deferred<Snapshot>();
    const apply = vi.spyOn(api, "applyMouseRemapping").mockReturnValue(write.promise);
    const { user, panel } = await buttonsView(api);
    await user.selectOptions(panel.getByRole("combobox", { name: "Azione per Laterale" }), "key");
    await user.selectOptions(panel.getByRole("combobox", { name: "Tasto per Laterale" }), "30");
    await user.click(panel.getByRole("button", { name: "Applica pulsanti" }));
    expect(panel.getByRole("button", { name: "Applicazione pulsanti..." })).toBeDisabled();
    expect(panel.getByText("Inattiva", { selector: "strong" })).toBeVisible();
    const latest = clone(initial);
    latest.revision = 8;
    latest.devices[0].values.remapping = { ...initial.devices[0].values.remapping!, state: "active", enabled: true, profileId: "new-profile", bindings: [{ buttonId: "button-275", action: { kind: "key", code: 31 } }] };
    act(() => emit(latest));
    const older = clone(latest);
    older.revision = 7;
    older.devices[0].values.remapping!.bindings[0].action = { kind: "key", code: 30 };
    await act(async () => { write.resolve(older); await write.promise; });
    expect(panel.getByRole("combobox", { name: "Tasto per Laterale" })).toHaveValue("31");
    expect(panel.getByText("Azione salvata: S")).toBeVisible();
    expect(screen.getByText("LETTURA / 0008")).toBeVisible();
    expect(apply).toHaveBeenCalledOnce();
  });

  it("mantiene Ripristina originali disponibile anche se il provider non e piu scrivibile", async () => {
    const initial = await createDemoApi(1).getSnapshot();
    initial.devices[0].capabilities.remapping!.availability = "providerMissing";
    initial.devices[0].capabilities.remapping!.writable = false;
    initial.devices[0].values.remapping = { ...initial.devices[0].values.remapping!, state: "paused", enabled: true, autoApply: true, pausedForSafety: true, profileId: "profile", bindings: [{ buttonId: "button-275", action: { kind: "key", code: 30 } }] };
    const { api } = controlledApi(initial);
    const result = clone(initial);
    result.revision = 2;
    result.devices[0].values.remapping = { ...initial.devices[0].values.remapping!, state: "inactive", enabled: false, autoApply: false, pausedForSafety: false };
    const restore = vi.spyOn(api, "disableMouseRemapping").mockResolvedValue(result);
    const { user, panel } = await buttonsView(api);
    expect(panel.getByRole("combobox", { name: "Azione per Laterale" })).toBeDisabled();
    expect(panel.getByRole("button", { name: "Applica pulsanti" })).toBeDisabled();
    expect(panel.getByRole("button", { name: "Ripristina originali" })).toBeEnabled();
    await user.click(panel.getByRole("button", { name: "Ripristina originali" }));
    expect(restore).toHaveBeenCalledWith("demo-viper-1");
    await waitFor(() => expect(panel.getByText("Inattiva", { selector: "strong" })).toBeVisible());
    expect(panel.getByRole("switch", { name: "Riattiva automaticamente" })).not.toBeChecked();
  });

  it("non inventa sorgenti salvate scomparse e blocca Apply finche vengono ripristinati gli originali", async () => {
    const initial = await createDemoApi(1).getSnapshot();
    initial.devices[0].capabilities.remapping!.buttons = initial.devices[0].capabilities.remapping!.buttons.filter((button) => button.code === 272);
    initial.devices[0].values.remapping = { ...initial.devices[0].values.remapping!, state: "paused", enabled: true, profileId: "profile", bindings: [{ buttonId: "missing-button", action: { kind: "key", code: 30 } }] };
    const { api } = controlledApi(initial);
    const apply = vi.spyOn(api, "applyMouseRemapping");
    const { panel } = await buttonsView(api);
    expect(panel.getAllByRole("combobox", { name: /^Azione per/ })).toHaveLength(1);
    expect(panel.getByText(/Pulsanti salvati non rilevati: missing-button/)).toBeVisible();
    expect(panel.getByRole("button", { name: "Applica pulsanti" })).toBeDisabled();
    expect(panel.getByRole("button", { name: "Ripristina originali" })).toBeEnabled();
    expect(apply).not.toHaveBeenCalled();
  });

  it("dopo Ripristina originali permette una nuova bozza anche se il profilo conserva sorgenti scomparse", async () => {
    const initial = await createDemoApi(1).getSnapshot();
    initial.devices[0].values.remapping = { ...initial.devices[0].values.remapping!, state: "paused", enabled: true, profileId: "profile", bindings: [{ buttonId: "missing-button", action: { kind: "key", code: 30 } }] };
    const { api } = controlledApi(initial);
    const restored = clone(initial);
    restored.revision = 2;
    restored.devices[0].values.remapping = { ...initial.devices[0].values.remapping!, state: "inactive", enabled: false, autoApply: false, activePreset: null };
    vi.spyOn(api, "disableMouseRemapping").mockResolvedValue(restored);
    const apply = vi.spyOn(api, "applyMouseRemapping").mockResolvedValue(restored);
    const { user, panel } = await buttonsView(api);
    await user.click(panel.getByRole("button", { name: "Ripristina originali" }));
    await waitFor(() => expect(panel.getByText("Inattiva", { selector: "strong" })).toBeVisible());
    expect(panel.getByText(/Pulsanti salvati non rilevati: missing-button/)).toBeVisible();
    await user.selectOptions(panel.getByRole("combobox", { name: "Azione per Laterale" }), "disabled");
    expect(panel.getByRole("button", { name: "Applica pulsanti" })).toBeEnabled();
    await user.click(panel.getByRole("button", { name: "Applica pulsanti" }));
    expect(apply).toHaveBeenCalledWith("demo-viper-1", [{ buttonId: "button-275", action: { kind: "disabled" } }], false, false);
  });

  it("resetta le bozze dei pulsanti cambiando mouse e annulla localmente senza IPC", async () => {
    const api = createDemoApi();
    const apply = vi.spyOn(api, "applyMouseRemapping");
    const { user, panel } = await buttonsView(api);
    await user.selectOptions(panel.getByRole("combobox", { name: "Azione per Laterale" }), "disabled");
    await user.click(panel.getByRole("button", { name: "Annulla bozza pulsanti" }));
    expect(panel.getByRole("combobox", { name: "Azione per Laterale" })).toHaveValue("default");
    await user.selectOptions(panel.getByRole("combobox", { name: "Azione per Laterale" }), "disabled");
    await user.selectOptions(screen.getByLabelText("Mouse selezionato"), "demo-g305-1");
    const nextPanel = within(screen.getByRole("region", { name: /Pulsanti rilevati/ }));
    expect(nextPanel.getByRole("combobox", { name: "Azione per Laterale" })).toHaveValue("default");
    expect(nextPanel.getByRole("button", { name: "Applica pulsanti" })).toBeDisabled();
    expect(apply).not.toHaveBeenCalled();
  });

  it.each(["unknownIdentity", "unsupportedPlatform", "noButtons", "noCapability"])("resta in sola lettura con %s senza tentare rimappature", async (scenario) => {
    const initial = await createDemoApi(1).getSnapshot();
    const capability = initial.devices[0].capabilities.remapping!;
    if (scenario === "unknownIdentity") capability.fingerprint = null;
    if (scenario === "unsupportedPlatform") { initial.platform = "unsupportedPlatform"; initial.demo = false; capability.availability = "unsupportedPlatform"; }
    if (scenario === "noButtons") capability.buttons = [];
    if (scenario === "noCapability") { initial.devices[0].capabilities.remapping = null; initial.devices[0].values.remapping = null; }
    const { api } = controlledApi(initial);
    const apply = vi.spyOn(api, "applyMouseRemapping");
    const { panel } = await buttonsView(api);
    expect(panel.getByRole("button", { name: "Applica pulsanti" })).toBeDisabled();
    expect(panel.getByRole("switch", { name: "Riattiva automaticamente" })).toBeDisabled();
    for (const control of panel.queryAllByRole("combobox")) expect(control).toBeDisabled();
    expect(apply).not.toHaveBeenCalled();
  });
});

describe("audio confermato e profili espliciti", () => {
  it("un toggle fallito non diventa On e viene eseguita la rilettura", async () => {
    const user = userEvent.setup();
    const api = createDemoApi();
    const getSnapshot = vi.spyOn(api, "getSnapshot");
    vi.spyOn(api, "setSurround").mockRejectedValue({ code: "missingHrtf", message: "SOFA non disponibile.", detail: null });
    await dashboard(api);
    const toggle = screen.getByRole("switch", { name: "Surround 7.1" });
    await user.click(toggle);
    await screen.findByRole("alert");
    expect(toggle).not.toBeChecked();
    expect(screen.getByText("Disattivato")).toBeVisible();
    expect(screen.queryByText("Attivo", { exact: true })).not.toBeInTheDocument();
    expect(getSnapshot).toHaveBeenCalledTimes(2);
  });

  it("Off resta disponibile anche quando scompare il target di un surround On", async () => {
    const user = userEvent.setup();
    const initial = await createDemoApi(0).getSnapshot();
    initial.audio = { ...initial.audio, available: false, outputs: [], selectedOutput: null, defaultOutput: null, surroundState: "on", canEnableSurround: false, surroundReason: "Target scollegato." };
    const { api } = controlledApi(initial);
    const result = clone(initial);
    result.revision += 1;
    result.audio.surroundState = "off";
    const setSurround = vi.spyOn(api, "setSurround").mockResolvedValue(result);
    await dashboard(api);
    const toggle = screen.getByRole("switch", { name: "Surround 7.1" });
    expect(toggle).toBeChecked();
    expect(toggle).toBeEnabled();
    await user.click(toggle);
    expect(setSurround).toHaveBeenCalledWith(false);
    await waitFor(() => expect(toggle).not.toBeChecked());
  });

  it("DPI pending non blocca il surround o il resto dell'audio", async () => {
    const user = userEvent.setup();
    const api = createDemoApi(1);
    const initial = await api.getSnapshot();
    const write = deferred<Snapshot>();
    vi.spyOn(api, "setDpi").mockReturnValue(write.promise);
    await dashboard(api);
    fireEvent.change(screen.getByRole("slider", { name: "DPI desiderati" }), { target: { value: "1200" } });
    await user.click(screen.getByRole("button", { name: "Applica DPI" }));
    expect(screen.getByRole("button", { name: "Applicazione DPI..." })).toBeDisabled();
    const surround = screen.getByRole("switch", { name: "Surround 7.1" });
    expect(surround).toBeEnabled();
    await user.click(surround);
    await waitFor(() => expect(surround).toBeChecked());
    await act(async () => { write.resolve(initial); await write.promise; });
    expect(surround).toBeChecked();
  });

  it("resetta le bozze audio per nodo e lascia volume/mute null in sola lettura", async () => {
    const user = userEvent.setup();
    const initial = await createDemoApi().getSnapshot();
    initial.audio.inputs[0].volume = null;
    initial.audio.inputs[0].muted = null;
    await dashboard(controlledApi(initial).api);
    await user.click(screen.getByRole("tab", { name: /Audio/ }));
    const picker = screen.getByLabelText("Uscita audio");
    fireEvent.change(screen.getByRole("slider", { name: "Volume uscita" }), { target: { value: "20" } });
    await user.selectOptions(picker, initial.audio.outputs[0].nodeName);
    expect(screen.getByRole("slider", { name: "Volume uscita" })).toHaveValue("62");
    await user.selectOptions(picker, initial.audio.outputs[1].nodeName);
    expect(screen.getByRole("slider", { name: "Volume uscita" })).toHaveValue("68");
    expect(screen.getByRole("slider", { name: "Guadagno microfono" })).toBeDisabled();
    expect(screen.getByRole("switch", { name: "Silenzia microfono" })).toBeDisabled();
    expect(screen.getByText("Stato mute non leggibile.")).toBeVisible();
    expect(screen.getByRole("slider", { name: "Guadagno microfono" })).toHaveAttribute("aria-valuetext", "Volume non disponibile");
  });

  it("importa HRTF senza argomenti e salva/applica profili solo su azione esplicita", async () => {
    const user = userEvent.setup();
    const api = createDemoApi();
    const importHrtf = vi.spyOn(api, "importHrtf");
    const save = vi.spyOn(api, "saveAudioProfile");
    const apply = vi.spyOn(api, "applyAudioProfile");
    await dashboard(api);
    await user.click(screen.getByRole("tab", { name: /Audio/ }));
    expect(apply).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Importa HRTF SOFA" }));
    expect(importHrtf).toHaveBeenCalledWith();
    await user.click(screen.getByText(/Nuovo profilo/, { selector: "summary" }));
    await user.type(screen.getByLabelText("Nome del profilo"), "Spaziale");
    fireEvent.change(screen.getByRole("slider", { name: /Volume desiderato/ }), { target: { value: "90" } });
    await user.click(screen.getByRole("switch", { name: "Surround desiderato nel profilo" }));
    await user.click(screen.getByRole("button", { name: "Salva profilo" }));
    expect(save).toHaveBeenCalledWith(expect.objectContaining({ schemaVersion: 1, name: "Spaziale", volume: 0.9, surroundRequested: true }));
    expect(apply).not.toHaveBeenCalled();
    expect(screen.getByRole("switch", { name: "Surround 7.1" })).not.toBeChecked();
    await user.click(await screen.findByRole("button", { name: "Applica profilo Spaziale" }));
    expect(apply).toHaveBeenCalledOnce();
    await waitFor(() => expect(screen.getByRole("switch", { name: "Surround 7.1" })).toBeChecked());
    expect(screen.getByRole("slider", { name: "Volume uscita" })).toHaveValue("68");
  });
});

describe("revisioni, hotplug e lifecycle", () => {
  it("ignora eventi e risposte mutazione piu vecchi della snapshot confermata", async () => {
    const user = userEvent.setup();
    const initial = await createDemoApi(1).getSnapshot();
    const { api, emit } = controlledApi(initial);
    const write = deferred<Snapshot>();
    vi.spyOn(api, "setDpi").mockReturnValue(write.promise);
    await dashboard(api);
    fireEvent.change(screen.getByRole("slider", { name: "DPI desiderati" }), { target: { value: "3200" } });
    await user.click(screen.getByRole("button", { name: "Applica DPI" }));
    const latest = clone(initial);
    latest.revision = 9;
    latest.devices[0].values.dpi = { x: 1600, y: 1600 };
    act(() => emit(latest));
    const older = clone(initial);
    older.revision = 8;
    older.devices[0].values.dpi = { x: 3200, y: 3200 };
    act(() => emit(older));
    await act(async () => { write.resolve(older); await write.promise; });
    expect(screen.getByText("1600 DPI", { selector: "strong" })).toBeVisible();
    expect(screen.getByText("LETTURA / 0009")).toBeVisible();
  });

  it("non sovrascrive un evento recente con una lettura iniziale tardiva", async () => {
    const initial = await createDemoApi(1).getSnapshot();
    const { api, emit } = controlledApi(initial);
    const loading = deferred<Snapshot>();
    vi.spyOn(api, "getSnapshot").mockReturnValue(loading.promise);
    render(<App api={api} />);
    await waitFor(() => expect(api.subscribe).toHaveBeenCalledOnce());
    const latest = clone(initial);
    latest.revision = 4;
    latest.devices[0].values.dpi = { x: 1800, y: 1800 };
    act(() => emit(latest));
    await act(async () => { loading.resolve(initial); await loading.promise; });
    expect(screen.getByText("1800 DPI", { selector: "strong" })).toBeVisible();
    expect(screen.getByText("LETTURA / 0004")).toBeVisible();
  });

  it("unplug del mouse selezionato passa al successivo senza valori della bozza precedente", async () => {
    const user = userEvent.setup();
    const initial = await createDemoApi().getSnapshot();
    const { api, emit } = controlledApi(initial);
    await dashboard(api);
    await user.click(screen.getByRole("tab", { name: /Mouse/ }));
    fireEvent.change(screen.getByRole("slider", { name: "DPI desiderati" }), { target: { value: "1200" } });
    const next = clone(initial);
    next.revision = 2;
    next.devices = next.devices.filter((device) => device.id !== "demo-viper-1");
    act(() => emit(next));
    expect(screen.getByRole("heading", { name: "Logitech G305" })).toBeVisible();
    expect(screen.getByRole("slider", { name: "DPI desiderati" })).toHaveValue("2");
    next.revision = 3;
    next.devices = [];
    act(() => emit(next));
    expect(screen.getByRole("heading", { name: "Nessun mouse rilevato" })).toBeVisible();
  });

  it("StrictMode pulisce anche una sottoscrizione risolta dopo l'unmount", async () => {
    const api = createDemoApi(0);
    const first = deferred<() => void>();
    const second = deferred<() => void>();
    const stopFirst = vi.fn();
    const stopSecond = vi.fn();
    const subscribe = vi.spyOn(api, "subscribe").mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    const { unmount } = render(<StrictMode><App api={api} /></StrictMode>);
    await waitFor(() => expect(subscribe).toHaveBeenCalledTimes(2));
    await act(async () => { first.resolve(stopFirst); second.resolve(stopSecond); await Promise.all([first.promise, second.promise]); });
    expect(stopFirst).toHaveBeenCalledOnce();
    expect(stopSecond).not.toHaveBeenCalled();
    unmount();
    expect(stopSecond).toHaveBeenCalledOnce();
  });
});
