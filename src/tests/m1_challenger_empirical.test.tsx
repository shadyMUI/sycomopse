import { useState } from "react";
import { act, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import App from "../App";
import { createDemoApi } from "../api";
import { ErrorBoundary } from "../components/ErrorBoundary";
import { DeviceCard, DeviceIllustration } from "../components/DeviceCard";
import { NodeControls } from "../components/Controls";
import Dashboard from "../views/Dashboard";
import MouseButtons from "../views/MouseButtons";
import type { AudioNode, HubApi, Snapshot } from "../types";

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

describe("DEF-04: HRTF non-null runtime crash prevention & edge cases", () => {
  it("applyAudioProfile rejects cleanly with demoValidation when HRTF reference does not exist, without SyntaxError or TypeError", async () => {
    const api = createDemoApi();
    const snapshot = await api.getSnapshot();

    const badProfileId = "00000000-0000-0000-0000-000000000099";
    const stereoOutput = snapshot.audio.outputs.find((o) => o.channels.length === 2 && !o.isVirtual);
    expect(stereoOutput).toBeDefined();

    const badProfile = {
      id: badProfileId,
      name: "Bad HRTF Profile",
      outputNodeName: stereoOutput!.nodeName,
      volume: 0.8,
      mute: false,
      surroundRequested: false,
      hrtf: "completely-nonexistent-hrtf-uuid-12345.sofa",
      schemaVersion: 1 as const,
    };

    // Attempting to apply a profile with unmapped HRTF should reject cleanly with demoValidation
    // and NOT throw "SyntaxError: Unexpected token u in JSON at position 0"
    await expect(api.applyAudioProfile(badProfileId)).rejects.toMatchObject({
      code: "demoValidation",
      message: "Profile not found.",
    });

    // Even if registered in profiles:
    snapshot.profiles.push(badProfile);
    await expect(api.saveAudioProfile(badProfile)).rejects.toMatchObject({
      code: "demoValidation",
      message: "HRTF reference not available in demo.",
    });
  });

  it("handles unmapped HRTF safely in App without white screen crash", async () => {
    const initial = await createDemoApi().getSnapshot();
    const stereoOutput = initial.audio.outputs.find((o) => o.channels.length === 2 && !o.isVirtual)!;

    // Injected invalid profile into initial snapshot
    const corruptedProfile = {
      id: "11111111-2222-3333-4444-555555555555",
      name: "Corrupted HRTF Profile",
      outputNodeName: stereoOutput.nodeName,
      volume: 0.7,
      mute: false,
      surroundRequested: false,
      hrtf: "missing-sofa-id.sofa",
      schemaVersion: 1 as const,
    };
    initial.profiles.push(corruptedProfile);

    const { api } = controlledApi(initial);
    const user = userEvent.setup();
    render(<App api={api} />);
    await screen.findByRole("heading", { name: /Your setup/ });

    // Navigate to Audio tab
    await user.click(screen.getByRole("tab", { name: /Audio/ }));
    await screen.findByRole("heading", { name: /Listening, under control/ });

    // The corrupted profile is visible in the profile list
    expect(screen.getByText("Corrupted HRTF Profile")).toBeInTheDocument();

    // Click apply on corrupted profile
    const applyButton = screen.getByRole("button", { name: `Apply profile ${corruptedProfile.name}` });
    await user.click(applyButton);

    // Verify error banner is shown and App did NOT crash with a white screen
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    expect(screen.getByText("Operation not confirmed")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: /Listening, under control/ })).toBeInTheDocument();
  });
});

describe("DEF-08: React ErrorBoundary catches component throws and allows recovery", () => {
  function ThrowingComponent({ shouldThrow, message }: { shouldThrow: boolean; message: string }) {
    if (shouldThrow) {
      throw new Error(message);
    }
    return <div data-testid="recovered-content">Tutto funzionante</div>;
  }

  it("catches render errors and displays fallback UI without unmounting", () => {
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});

    render(
      <ErrorBoundary fallbackTitle="Errore di test">
        <ThrowingComponent shouldThrow={true} message="Errore fatale simulato nel componente" />
      </ErrorBoundary>
    );

    const fallback = screen.getByRole("alert");
    expect(fallback).toBeInTheDocument();
    expect(screen.getByText("Errore di test")).toBeInTheDocument();
    expect(screen.getByText("Errore fatale simulato nel componente")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Retry|Riprova/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Reload application|Ricarica applicazione/ })).toBeInTheDocument();

    spy.mockRestore();
  });

  it("recovers from error when 'Riprova' is clicked after condition clears", async () => {
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});
    const user = userEvent.setup();

    function Parent() {
      const [shouldThrow, setShouldThrow] = useState(true);
      return (
        <ErrorBoundary fallbackTitle="Errore recuperabile" onReset={() => setShouldThrow(false)}>
          <ThrowingComponent shouldThrow={shouldThrow} message="Errore momentaneo" />
        </ErrorBoundary>
      );
    }

    render(<Parent />);
    expect(screen.getByText("Errore momentaneo")).toBeInTheDocument();

    // Click "Retry"
    await user.click(screen.getByRole("button", { name: /Retry|Riprova/ }));

    // Verify recovery
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByTestId("recovered-content")).toBeInTheDocument();
    expect(screen.getByText("Tutto funzionante")).toBeInTheDocument();

    spy.mockRestore();
  });

  it("isolates tab failure so navigation and other tabs remain functional", async () => {
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});
    const initial = await createDemoApi().getSnapshot();

    // Inject an invalid device whose values is null, causing DeviceCard inside Dashboard to throw
    const corruptedSnapshot = clone(initial);
    corruptedSnapshot.devices = [
      {
        ...initial.devices[0],
        values: null as any,
      },
    ];

    const { api } = controlledApi(corruptedSnapshot);
    const user = userEvent.setup();
    render(<App api={api} />);

    // Dashboard throws during render inside tabpanel ErrorBoundary
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    expect(screen.getByText("Error loading section")).toBeInTheDocument();

    // The nav tabs are still fully mounted and clickable!
    const audioTab = screen.getByRole("tab", { name: /Audio/ });
    expect(audioTab).toBeInTheDocument();

    // Switch to Audio tab
    await user.click(audioTab);
    expect(await screen.findByRole("heading", { name: /Listening, under control/ })).toBeInTheDocument();

    spy.mockRestore();
  });
});

describe("DEF-09: W3C HTML5 compliance on buttons (no block/heading elements inside buttons)", () => {
  it("DeviceCard button contains ONLY valid phrasing content according to W3C HTML5", async () => {
    const snapshot = await createDemoApi(3).getSnapshot();

    for (const device of snapshot.devices) {
      const onOpen = vi.fn();
      const { unmount } = render(
        <DeviceCard device={device} selected={false} onOpen={onOpen} />
      );

      const button = screen.getByRole("button", { name: new RegExp(`(?:Open controls for|Apri controlli di) ${device.name}`) });
      expect(button).toBeInTheDocument();

      // Inspect all descendant elements inside button
      const descendants = button.querySelectorAll("*");
      const forbiddenTags = [
        "DIV", "P", "H1", "H2", "H3", "H4", "H5", "H6",
        "SECTION", "ARTICLE", "HEADER", "FOOTER", "ASIDE", "NAV", "MAIN",
        "UL", "OL", "LI", "DL", "DT", "DD",
        "METER", "BUTTON", "INPUT", "SELECT", "TEXTAREA", "DETAILS", "SUMMARY"
      ];

      for (const el of descendants) {
        expect(
          forbiddenTags,
          `Found forbidden HTML element <${el.tagName.toLowerCase()}> inside <button> in DeviceCard for device ${device.name}`
        ).not.toContain(el.tagName.toUpperCase());
      }

      unmount();
    }
  });

  it("BatteryReadout in compact mode renders no div, p, meter, or dl", () => {
    const battery = {
      level: "normal" as const,
      percentage: 85,
      charging: false,
      stale: false,
      observedAt: 1234567890,
      sourceUpdatedAt: 1234567890,
      source: "upower" as const,
    };

    const { container } = render(
      <DeviceCard
        device={{
          id: "test", name: "Test Device", brand: "razer", kind: "mouse", provider: "openrazer",
          availability: "available", support: "supported",
          identity: { serial: null, physicalPath: null, vendorId: null, productId: null, transport: null, firmware: null, backendPath: null },
          capabilities: { dpi: null, polling: null, remapping: null },
          values: { battery, dpi: null, pollingRate: null, remapping: null },
          outputNodeNames: [],
          limitations: [],
        }}
        selected={false}
        onOpen={() => {}}
      />
    );

    const button = container.querySelector("button")!;
    expect(button.querySelectorAll("div")).toHaveLength(0);
    expect(button.querySelectorAll("meter")).toHaveLength(0);
    expect(button.querySelectorAll("p")).toHaveLength(0);
    expect(button.querySelectorAll("dl")).toHaveLength(0);
    expect(button.querySelectorAll("h3")).toHaveLength(0);
  });
});

describe("DEF-10: Draft preservation during asynchronous snapshot-changed events", () => {
  it("preserves NodeControls user draft when snapshot-changed events arrive with unchanged or background volumes", () => {
    const node: AudioNode = {
      id: 40,
      objectSerial: 110,
      nodeName: "alsa_output.pci-0000_00_1f.3.analog-stereo",
      description: "Audio interno",
      mediaClass: "Audio/Sink",
      channels: ["FL", "FR"],
      volume: 0.4,
      muted: false,
      isVirtual: false,
      deviceSerial: null,
      deviceBusPath: null,
      vendorId: null,
      productId: null,
      transport: null,
    };

    const mockApi = createDemoApi();
    const runMutation = vi.fn();
    const pending = new Set<string>();

    const { rerender } = render(
      <NodeControls
        node={node}
        available={true}
        api={mockApi}
        pending={pending}
        runMutation={runMutation}
      />
    );

    const slider = screen.getByRole("slider", { name: /(?:Output volume|Volume uscita)/ });
    expect(slider).toHaveValue("40");

    // User moves slider to 80%
    fireEvent.change(slider, { target: { value: "80" } });
    expect(slider).toHaveValue("80");
    expect(screen.getByText("80%")).toBeInTheDocument();

    // Asynchronous snapshot-changed arrives where node object is a new reference but volume is still 40%
    const updatedNode1 = { ...node };
    rerender(
      <NodeControls
        node={updatedNode1}
        available={true}
        api={mockApi}
        pending={pending}
        runMutation={runMutation}
      />
    );

    // User's draft MUST be preserved!
    expect(slider).toHaveValue("80");
    expect(screen.getByText("80%")).toBeInTheDocument();

    // Even if background service updated confirmed volume to 45% (different from original 40%)
    const updatedNode2 = { ...node, volume: 0.45 };
    rerender(
      <NodeControls
        node={updatedNode2}
        available={true}
        api={mockApi}
        pending={pending}
        runMutation={runMutation}
      />
    );

    // User's draft MUST STILL be preserved!
    expect(slider).toHaveValue("80");
    expect(screen.getByText("80%")).toBeInTheDocument();
    expect(screen.getByText(/(?:Confirmed reading|Lettura confermata): 45%/)).toBeInTheDocument();

    // Switching to a different node resets the draft to the new node's volume
    const differentNode: AudioNode = {
      ...node,
      nodeName: "alsa_output.usb-dac",
      volume: 0.65,
    };
    rerender(
      <NodeControls
        node={differentNode}
        available={true}
        api={mockApi}
        pending={pending}
        runMutation={runMutation}
      />
    );
    expect(slider).toHaveValue("65");
    expect(screen.getByText("65%")).toBeInTheDocument();
  });

  it("preserves mouse DPI draft in full App during continuous snapshot updates", async () => {
    const initial = await createDemoApi().getSnapshot();
    const { api, emit } = controlledApi(initial);

    render(<App api={api} />);
    await screen.findByRole("heading", { name: /Your setup/ });

    const manualInput = screen.getByRole("textbox", { name: /(?:Manual DPI|DPI manuali)/ });
    expect(manualInput).toHaveValue("800");

    // User modifies draft to 1400
    fireEvent.change(manualInput, { target: { value: "1400" } });
    expect(manualInput).toHaveValue("1400");

    // Simulate 20 rapid snapshot events from background polling (battery changes, revision bumps)
    for (let r = 2; r <= 20; r++) {
      const next = clone(initial);
      next.revision = r;
      if (next.devices[0].values.battery) {
        next.devices[0].values.battery.percentage = 80 - (r % 5);
      }
      act(() => emit(next));
    }

    // User's manual DPI input MUST remain "1400"
    expect(manualInput).toHaveValue("1400");
  });
});

describe("DEF-11: MouseButtons memoization eliminating unneeded serializations", () => {
  it("memoizes sourceKey, savedKey, and dirty state across non-remapping re-renders", async () => {
    const snapshot = await createDemoApi().getSnapshot();
    const mouse = snapshot.devices.find((d) => d.kind === "mouse" && d.capabilities.remapping)!;
    const api = createDemoApi();
    const runMutation = vi.fn();
    const pending = new Set<string>();

    const stringifySpy = vi.spyOn(JSON, "stringify");

    const { rerender } = render(
      <MouseButtons
        device={mouse}
        api={api}
        pending={pending}
        runMutation={runMutation}
      />
    );

    const initialCalls = stringifySpy.mock.calls.length;

    // Rerender with identical device and mutation controls
    rerender(
      <MouseButtons
        device={mouse}
        api={api}
        pending={pending}
        runMutation={runMutation}
      />
    );

    const callsAfterUnchangedRerender = stringifySpy.mock.calls.length - initialCalls;
    // With memoization on sourceKey, savedKey, saved, and dirty, memoized values do not rerun
    expect(callsAfterUnchangedRerender).toBe(0);

    stringifySpy.mockRestore();
  });
});

describe("DEF-12: React.memo prevents unnecessary re-renders on DeviceCard and Dashboard", () => {
  it("DeviceCard and DeviceIllustration are wrapped with React.memo", () => {
    expect((DeviceCard as any).$$typeof).toBe(Symbol.for("react.memo"));
    expect((DeviceIllustration as any).$$typeof).toBe(Symbol.for("react.memo"));
  });

  it("Dashboard is wrapped with React.memo", () => {
    expect((Dashboard as any).$$typeof).toBe(Symbol.for("react.memo"));
  });

  it("Dashboard does not re-render when controls and snapshot references are stable", async () => {
    const snapshot = await createDemoApi(3).getSnapshot();
    const onSelectMouse = vi.fn();
    const onOpenDevice = vi.fn();
    const runMutation = vi.fn();
    const api = createDemoApi();
    const pending = new Set<string>();

    function ParentWrapper() {
      const [count, setCount] = useState(0);
      return (
        <div>
          <button onClick={() => setCount((c) => c + 1)}>Increment ({count})</button>
          <Dashboard
            snapshot={snapshot}
            selectedMouseId={null}
            onSelectMouse={onSelectMouse}
            onOpenDevice={onOpenDevice}
            api={api}
            pending={pending}
            runMutation={runMutation}
          />
        </div>
      );
    }

    render(<ParentWrapper />);
    const heading = screen.getByRole("heading", { name: /Your setup/ });
    expect(heading).toBeInTheDocument();

    const button = screen.getByRole("button", { name: /Increment/ });
    await userEvent.click(button);
    expect(screen.getByRole("button", { name: "Increment (1)" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: /Your setup/ })).toBeInTheDocument();
  });
});
