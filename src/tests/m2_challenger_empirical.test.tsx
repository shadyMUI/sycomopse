import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import App from "../App";
import { createDemoApi, generateUuid } from "../api";
import type { HubApi, Snapshot } from "../types";

function clone<T>(val: T): T {
  return JSON.parse(JSON.stringify(val)) as T;
}

function controlledApi(initial: Snapshot) {
  let current = clone(initial);
  const listeners = new Set<(snapshot: Snapshot) => void>();
  const demoApi = createDemoApi();
  const api: HubApi = {
    ...demoApi,
    demo: initial.demo,
    getSnapshot: vi.fn(async () => clone(current)),
    subscribe: vi.fn(async (listener: (snapshot: Snapshot) => void) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    }),
  };
  return {
    api,
    emit(next: Snapshot) {
      current = clone(next);
      for (const listener of listeners) listener(clone(next));
    },
  };
}

describe("DEF-28: RFC 4122 v4 UUID generator correctness and uniqueness", () => {
  const rfc4122v4Regex = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

  it("generates 10,000 unique, strictly valid RFC 4122 v4 UUIDs via standard crypto.randomUUID", () => {
    const set = new Set<string>();
    for (let i = 0; i < 10000; i++) {
      const uuid = generateUuid();
      expect(uuid).toMatch(rfc4122v4Regex);
      set.add(uuid);
    }
    expect(set.size).toBe(10000);
  });

  it("generates valid RFC 4122 v4 UUIDs when crypto.randomUUID is undefined (legacy WebKitGTK)", () => {
    const originalRandomUUID = crypto.randomUUID;
    try {
      // @ts-expect-error intentionally undefining for test
      crypto.randomUUID = undefined;
      const set = new Set<string>();
      for (let i = 0; i < 1000; i++) {
        const uuid = generateUuid();
        expect(uuid).toMatch(rfc4122v4Regex);
        set.add(uuid);
      }
      expect(set.size).toBe(1000);
    } finally {
      crypto.randomUUID = originalRandomUUID;
    }
  });

  it("generates valid RFC 4122 v4 UUIDs when crypto object is completely undefined", () => {
    const originalCrypto = globalThis.crypto;
    try {
      // @ts-expect-error intentionally undefining for test
      delete globalThis.crypto;
      const set = new Set<string>();
      for (let i = 0; i < 1000; i++) {
        const uuid = generateUuid();
        expect(uuid).toMatch(rfc4122v4Regex);
        set.add(uuid);
      }
      expect(set.size).toBe(1000);
    } finally {
      globalThis.crypto = originalCrypto;
    }
  });
});

describe("DEF-27: AudioPanel draft form state preservation", () => {
  it("preserves uncommitted profile draft form fields across snapshot updates and output changes", async () => {
    const user = userEvent.setup();
    const demoApi = createDemoApi();
    const initialSnapshot = await demoApi.getSnapshot();
    const { api, emit } = controlledApi(initialSnapshot);

    render(<App api={api} />);
    await screen.findByRole("heading", { name: /(?:Your setup|Il tuo setup)/ });

    // Navigate to Audio tab
    await user.click(screen.getByRole("tab", { name: /Audio/ }));
    await screen.findByRole("heading", { name: /(?:Listening, under control\.|Ascolto, sotto controllo\.)/ });

    // Open "New profile" details
    const summary = screen.getByText(/(?:New profile|Nuovo profilo)/, { selector: "summary" });
    await user.click(summary);

    // Enter draft profile values
    const nameInput = screen.getByLabelText(/(?:Profile name|Nome del profilo)/);
    await user.type(nameInput, "Bozza Spaziale In Corso");
    expect(nameInput).toHaveValue("Bozza Spaziale In Corso");

    const volumeSlider = screen.getByRole("slider", { name: /(?:Desired volume|Volume desiderato)/ });
    fireEvent.change(volumeSlider, { target: { value: "85" } });
    expect(volumeSlider).toHaveValue("85");

    const surroundSwitch = screen.getByRole("switch", { name: /(?:Desired surround in profile|Surround desiderato nel profilo)/ });
    await user.click(surroundSwitch);
    expect(surroundSwitch).toBeChecked();

    // Trigger an external snapshot-changed event (e.g. revision bump, device status update)
    const updatedSnapshot = clone(initialSnapshot);
    updatedSnapshot.revision += 1;
    emit(updatedSnapshot);

    // Verify draft is completely preserved
    expect(screen.getByLabelText(/(?:Profile name|Nome del profilo)/)).toHaveValue("Bozza Spaziale In Corso");
    expect(screen.getByRole("slider", { name: /(?:Desired volume|Volume desiderato)/ })).toHaveValue("85");
    expect(screen.getByRole("switch", { name: /(?:Desired surround in profile|Surround desiderato nel profilo)/ })).toBeChecked();

    // Change output selection in playback section
    const otherOutput = initialSnapshot.audio.outputs.find((o) => !o.isVirtual && o.nodeName !== initialSnapshot.audio.selectedOutput);
    if (otherOutput) {
      fireEvent.change(screen.getByLabelText(/(?:Audio output|Uscita audio)/), { target: { value: otherOutput.nodeName } });
    }

    // Verify draft remains untouched
    expect(screen.getByLabelText(/(?:Profile name|Nome del profilo)/)).toHaveValue("Bozza Spaziale In Corso");
    expect(screen.getByRole("slider", { name: /(?:Desired volume|Volume desiderato)/ })).toHaveValue("85");
    expect(screen.getByRole("switch", { name: /(?:Desired surround in profile|Surround desiderato nel profilo)/ })).toBeChecked();
  });
});

describe("DEF-24: Redundant snapshot readback suppression on connection failure", () => {
  it("does not trigger secondary getSnapshot when initial connection fails", async () => {
    const initialSnapshot = await createDemoApi().getSnapshot();
    const { api } = controlledApi(initialSnapshot);
    vi.spyOn(api, "getSnapshot").mockRejectedValue({ code: "connectionFailed", message: "Connessione fallita", detail: null });

    render(<App api={api} />);
    await screen.findByRole("heading", { name: /(?:Backend unreachable\.|Backend non raggiungibile\.)/ });

    // getSnapshot should have been attempted exactly once on startup, and NOT retried in catch
    expect(api.getSnapshot).toHaveBeenCalledTimes(1);
  });

  it("triggers secondary getSnapshot for reconciliation when a normal mutation fails", async () => {
    const user = userEvent.setup();
    const demoApi = createDemoApi(1);
    const initialSnapshot = await demoApi.getSnapshot();
    const { api } = controlledApi(initialSnapshot);

    const getSnapshotSpy = vi.spyOn(api, "getSnapshot");
    vi.spyOn(api, "setDpi").mockRejectedValue({ code: "writeFailed", message: "Errore DPI", detail: null });

    render(<App api={api} />);
    await screen.findByRole("heading", { name: /(?:Your setup|Il tuo setup)/ });

    const applyButton = screen.getByRole("button", { name: /(?:Apply DPI|Applica DPI)/ });
    fireEvent.change(screen.getByRole("slider", { name: /(?:Target DPI|DPI desiderati)/ }), { target: { value: "1200" } });
    await waitFor(() => expect(applyButton).toBeEnabled(), { timeout: 5000 });
    await user.click(applyButton);

    await screen.findByRole("alert", {}, { timeout: 3000 });
    // Initial mount call + reconciliation call = 2
    expect(getSnapshotSpy).toHaveBeenCalledTimes(2);
  });
});

describe("DEF-30: WCAG 2.1 AA mathematical color contrast verification", () => {
  function hexToRgb(hex: string): [number, number, number] {
    const bigint = parseInt(hex.slice(1), 16);
    return [(bigint >> 16) & 255, (bigint >> 8) & 255, bigint & 255];
  }

  function srgbToLinear(c: number): number {
    c = c / 255;
    return c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
  }

  function relativeLuminance(hex: string): number {
    const [r, g, b] = hexToRgb(hex);
    return 0.2126 * srgbToLinear(r) + 0.7152 * srgbToLinear(g) + 0.0722 * srgbToLinear(b);
  }

  function contrastRatio(hex1: string, hex2: string): number {
    const l1 = relativeLuminance(hex1);
    const l2 = relativeLuminance(hex2);
    return (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05);
  }

  it("proves .brand-subtitle (#b4b4b4 on #1b1b1b) exceeds WCAG 2.1 AA 4.5:1 ratio", () => {
    const ratio = contrastRatio("#b4b4b4", "#1b1b1b");
    expect(ratio).toBeGreaterThanOrEqual(4.5);
    expect(ratio).toBeCloseTo(8.31, 1);
  });

  it("proves .instrument-caption (#b4b4b4 on #222222) exceeds WCAG 2.1 AA 4.5:1 ratio", () => {
    const ratio = contrastRatio("#b4b4b4", "#222222");
    expect(ratio).toBeGreaterThanOrEqual(4.5);
    expect(ratio).toBeCloseTo(7.67, 1);
  });

  it("proves .device-card-copy p (#b4b4b4 on #181818) exceeds WCAG 2.1 AA 4.5:1 ratio", () => {
    const ratio = contrastRatio("#b4b4b4", "#181818");
    expect(ratio).toBeGreaterThanOrEqual(4.5);
    expect(ratio).toBeCloseTo(8.56, 1);
  });

  it("proves .tiny-label (#b4b4b4 on #111111) exceeds WCAG 2.1 AA 4.5:1 ratio", () => {
    const ratio = contrastRatio("#b4b4b4", "#111111");
    expect(ratio).toBeGreaterThanOrEqual(4.5);
    expect(ratio).toBeCloseTo(9.11, 1);
  });
});
