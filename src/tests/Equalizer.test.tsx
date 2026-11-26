import { act, fireEvent, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createDemoApi } from "../api";
import { equalizerPresets, filterTargetReason, isOutputApplied } from "../audio";
import type { HubApi } from "../types";
import { audioView } from "./appHarness";

function advance(ms: number) {
  return act(async () => { await vi.advanceTimersByTimeAsync(ms); });
}

function delayFirstWrite(api: HubApi) {
  const commit = api.setEqualizer;
  let complete!: () => Promise<void>;
  const write = vi.spyOn(api, "setEqualizer").mockImplementationOnce((nodeName, settings) => new Promise((resolve, reject) => {
    complete = () => commit(nodeName, settings).then(resolve, reject);
  }));
  return { write, finish: () => act(async () => { await complete(); }) };
}

afterEach(() => { if (vi.isFakeTimers()) vi.useRealTimers(); });

describe("headphone equalizer", () => {
  it("does not treat our virtual sink as applied when another output is pending", async () => {
    const api = createDemoApi();
    const target = (await api.getSnapshot()).audio.selectedOutput!;
    const { audio } = await api.setEqualizer(target, { enabled: true, bands: equalizerPresets[0].bands });
    const headphones = audio.outputs.find((node) => node.nodeName === target)!;
    const other = audio.outputs.find((node) => !node.isVirtual && node.nodeName !== target)!;
    expect(isOutputApplied(audio, headphones)).toBe(true);
    audio.configuredDefaultOutput = other.nodeName;
    expect(isOutputApplied(audio, headphones)).toBe(false);
    expect(filterTargetReason(audio, headphones)).toMatch(/Apply output/);
  });

  it("requires Apply output to recover a filter when its physical target becomes the default directly", async () => {
    const base = createDemoApi();
    const target = (await base.getSnapshot()).audio.selectedOutput!;
    const { audio } = await base.setEqualizer(target, { enabled: true, bands: equalizerPresets[1].bands });
    expect(audio.ownedFilterOutput).toBe(target);
    audio.defaultOutput = audio.configuredDefaultOutput = target;
    audio.equalizerOutput = null;
    const api = createDemoApi(3, audio);
    const route = vi.spyOn(api, "setDefaultOutput");
    const { user, eq } = await audioView(api);
    const applyOutput = screen.getByRole("button", { name: "Apply output" });
    expect(applyOutput).toBeEnabled();
    expect(eq.getByRole("button", { name: "Apply EQ" })).toBeDisabled();
    expect(eq.getByRole("button", { name: "Bypass saved EQ" })).toHaveAccessibleDescription(/Apply output/);
    expect(screen.getByRole("switch", { name: "Surround 7.1" })).toBeDisabled();
    expect(route).not.toHaveBeenCalled();
    await user.click(applyOutput);
    expect(route).toHaveBeenCalledWith(target);
    expect(applyOutput).toBeDisabled();
    expect(eq.getByText("EQ active")).toBeVisible();
  });

  it("can bypass an enabled but idle saved EQ without starting a filter or committing edits", async () => {
    const { audio } = await createDemoApi().getSnapshot();
    const target = audio.selectedOutput!;
    const other = audio.outputs.find((node) => node.nodeName !== target)!.nodeName;
    const bands = equalizerPresets[1].bands;
    audio.equalizers[target] = { enabled: true, bands };
    audio.configuredDefaultOutput = null;
    const api = createDemoApi(3, audio);
    const write = vi.spyOn(api, "setEqualizer");
    const { user, eq } = await audioView(api);
    const preset = eq.getByRole("combobox", { name: "EQ preset" });
    expect(eq.getByText("EQ not active")).toBeVisible();
    expect(eq.getByRole("button", { name: "Apply EQ" })).toBeEnabled();
    await user.selectOptions(preset, "Gaming");
    await user.click(eq.getByRole("button", { name: "Bypass saved EQ" }));
    expect(write.mock.calls).toEqual([[target, { enabled: false, bands }]]);
    expect(eq.getByRole("button", { name: "Re-enable saved EQ" })).toBeEnabled();
    expect(preset).toHaveValue("Gaming");
    expect((await api.getSnapshot()).audio).toMatchObject({
      equalizers: { [target]: { enabled: false, bands } }, defaultOutput: target,
      configuredDefaultOutput: null, ownedFilterOutput: null, equalizerOutput: null, outputs: audio.outputs,
    });
    const picker = screen.getByRole("combobox", { name: "Audio output" });
    for (const name of [other, target]) {
      await user.selectOptions(picker, name);
      await user.click(screen.getByRole("button", { name: "Apply output" }));
    }
    expect((await api.getSnapshot()).audio).toMatchObject({ defaultOutput: target, ownedFilterOutput: null, equalizerOutput: null });
    expect(eq.getByText("EQ bypassed")).toBeVisible();
  });

  it("applies ten custom peaking bands explicitly; bypass and re-enable never commit pending edits", async () => {
    const api = createDemoApi();
    const target = (await api.getSnapshot()).audio.selectedOutput!;
    const write = vi.spyOn(api, "setEqualizer");
    const route = vi.spyOn(api, "setDefaultOutput");
    const { user, eq } = await audioView(api);
    const preset = eq.getByRole("combobox", { name: "EQ preset" });
    for (const name of ["Voice", "Gaming", "Neutral"]) {
      await user.selectOptions(preset, name);
      expect(preset).toHaveValue(name);
    }
    const gains = within(eq.getByRole("group", { name: "Band gains" })).getAllByRole("slider");
    const frequencies = [31.5, 63, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
    expect(gains).toHaveLength(10);
    await user.click(eq.getByText("Advanced: per-band Q"));
    const widths = within(eq.getByRole("group", { name: "Band Q values" })).getAllByRole("slider");
    expect(widths).toHaveLength(10);
    frequencies.forEach((hz, i) => {
      expect(gains[i]).toHaveAccessibleName(`${hz} Hz gain`);
      expect(gains[i]).toHaveAttribute("min", "-12");
      expect(gains[i]).toHaveAttribute("max", "12");
      expect(widths[i]).toHaveAccessibleName(`${hz} Hz Q`);
      expect(widths[i]).toHaveAttribute("min", "0.4");
      expect(widths[i]).toHaveAttribute("max", "4");
      expect(widths[i]).toHaveValue("1.4");
    });
    fireEvent.change(gains[0], { target: { value: "12" } });
    fireEvent.change(gains[1], { target: { value: "2.5" } });
    fireEvent.change(gains[9], { target: { value: "-12" } });
    fireEvent.change(widths[0], { target: { value: "0.4" } });
    fireEvent.change(widths[9], { target: { value: "4" } });
    await act(async () => { await api.refreshDevices(); });
    expect(preset).toHaveValue("Custom");
    expect(eq.getByLabelText("Automatic preamp on apply")).toHaveTextContent("-14.5 dB");
    expect(eq.getByText(/Current preamp: 0 dB/)).toBeVisible();
    expect(write).not.toHaveBeenCalled();
    expect(route).not.toHaveBeenCalled();
    const bands = equalizerPresets[0].bands.map((band) => ({ ...band }));
    bands[0] = { gainDb: 12, q: 0.4 };
    bands[1].gainDb = 2.5;
    bands[9] = { gainDb: -12, q: 4 };
    await user.click(eq.getByRole("button", { name: "Apply EQ" }));
    expect(write).toHaveBeenCalledWith(target, { enabled: true, bands });
    expect(eq.getByText("EQ active")).toBeVisible();
    expect(eq.getByText(/Current preamp: -14.5 dB/)).toBeVisible();
    expect(screen.getByRole("switch", { name: "Surround 7.1" })).not.toBeChecked();
    expect(screen.getByRole("button", { name: "Apply output" })).toBeDisabled();
    vi.useFakeTimers();
    fireEvent.change(preset, { target: { value: "Voice" } });
    await act(async () => { fireEvent.click(eq.getByRole("button", { name: "Bypass EQ" })); });
    expect(write).toHaveBeenLastCalledWith(target, { enabled: false, bands });
    expect(eq.getByText(/Current preamp: 0 dB/)).toBeVisible();
    expect(preset).toHaveValue("Voice");
    await act(async () => { fireEvent.click(eq.getByRole("button", { name: "Re-enable saved EQ" })); });
    expect(write).toHaveBeenLastCalledWith(target, { enabled: true, bands });
    expect(preset).toHaveValue("Voice");
    await advance(300);
    fireEvent.click(eq.getByRole("button", { name: "Discard EQ edits" }));
    expect(preset).toHaveValue("Custom");
    expect(widths[9]).toHaveValue("4");
    expect(write).toHaveBeenCalledTimes(3);
  });

  it.each([false, true])("coalesces live preset, gain and Q edits without changing the managed sink (surround: %s)", async (surround) => {
    const api = createDemoApi();
    const target = (await api.getSnapshot()).audio.selectedOutput!;
    await api.setEqualizer(target, { enabled: true, bands: equalizerPresets[0].bands });
    if (surround) await api.setSurround(true, target);
    const sink = (await api.getSnapshot()).audio.defaultOutput!;
    await api.setVolume(sink, 0.27);
    const { audio: before } = await api.setMute(sink, true);
    const write = vi.spyOn(api, "setEqualizer");
    const { user, eq } = await audioView(api);
    await user.click(eq.getByText("Advanced: per-band Q"));
    vi.useFakeTimers();

    fireEvent.change(eq.getByRole("combobox", { name: "EQ preset" }), { target: { value: "Voice" } });
    await advance(45);
    fireEvent.change(eq.getByRole("slider", { name: "31.5 Hz gain" }), { target: { value: "5" } });
    await advance(45);
    fireEvent.change(eq.getByRole("slider", { name: "250 Hz Q" }), { target: { value: "2.6" } });
    expect(write).not.toHaveBeenCalled();
    await advance(10);

    const bands = equalizerPresets[1].bands.map((band, i) => i === 0 ? { ...band, gainDb: 5 } : i === 3 ? { ...band, q: 2.6 } : band);
    expect(write.mock.calls).toEqual([[target, { enabled: true, bands }]]);
    expect(eq.getByText(/Current preamp: -9 dB/)).toBeVisible();
    expect(eq.getByRole("button", { name: "Apply EQ" })).toBeDisabled();
    expect((await api.getSnapshot()).audio).toEqual({ ...before, equalizers: { [target]: { enabled: true, bands } } });
  });

  it("keeps the latest draft editable during a slow save, even when it matches the previous readback", async () => {
    const api = createDemoApi();
    const target = (await api.getSnapshot()).audio.selectedOutput!;
    const bands = equalizerPresets[0].bands;
    await api.setEqualizer(target, { enabled: true, bands });
    const { write, finish } = delayFirstWrite(api);
    const { eq } = await audioView(api);
    const gain = eq.getByRole("slider", { name: "31.5 Hz gain" });
    vi.useFakeTimers();

    fireEvent.change(gain, { target: { value: "4" } });
    await advance(100);
    expect(write).toHaveBeenCalledOnce();
    expect(gain).toBeEnabled();
    expect(eq.getByRole("combobox", { name: "EQ preset" })).toBeEnabled();
    expect(eq.getByRole("combobox", { name: "Preamp mode" })).toBeEnabled();
    expect(eq.getByRole("button", { name: "Bypass EQ" })).toBeDisabled();
    fireEvent.change(gain, { target: { value: "8" } });
    fireEvent.change(gain, { target: { value: "0" } });
    await advance(300);
    expect(write).toHaveBeenCalledOnce();

    await finish();
    expect(gain).toHaveValue("0");
    expect(eq.getByText(/Current preamp: -4 dB/)).toBeVisible();
    await advance(100);
    expect(write).toHaveBeenCalledTimes(2);
    expect(write).toHaveBeenLastCalledWith(target, { enabled: true, bands });
    expect(eq.getByText(/Current preamp: 0 dB/)).toBeVisible();
  });

  it("retains a rejected live draft and does not repeatedly retry it", async () => {
    const api = createDemoApi();
    const target = (await api.getSnapshot()).audio.selectedOutput!;
    const bands = equalizerPresets[0].bands;
    await api.setEqualizer(target, { enabled: true, bands });
    const write = vi.spyOn(api, "setEqualizer").mockRejectedValueOnce(new Error("Live update was not confirmed."));
    const { eq } = await audioView(api);
    const gain = eq.getByRole("slider", { name: "31.5 Hz gain" });
    vi.useFakeTimers();

    fireEvent.change(gain, { target: { value: "4" } });
    await advance(400);
    expect(write).toHaveBeenCalledOnce();
    expect(screen.getByRole("alert")).toHaveTextContent("Live update was not confirmed.");
    expect(eq.getByText(/Current preamp: 0 dB/)).toBeVisible();
    expect(gain).toHaveValue("4");
    fireEvent.change(gain, { target: { value: "5" } });
    await advance(100);
    expect(write).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(eq.getByText(/Current preamp: -5 dB/)).toBeVisible();
  });

  it("starts Manual at 0 dB without activating EQ and updates manual attenuation and Auto live", async () => {
    const api = createDemoApi();
    const target = (await api.getSnapshot()).audio.selectedOutput!;
    const write = vi.spyOn(api, "setEqualizer");
    const { eq } = await audioView(api);
    const mode = eq.getByRole("combobox", { name: "Preamp mode" });
    const bands = equalizerPresets[2].bands;
    vi.useFakeTimers();

    expect(mode).toHaveValue("auto");
    fireEvent.change(eq.getByRole("combobox", { name: "EQ preset" }), { target: { value: "Gaming" } });
    fireEvent.change(mode, { target: { value: "manual" } });
    const preamp = eq.getByRole("slider", { name: "Manual preamp" });
    expect(preamp).toHaveValue("0");
    expect(preamp).toHaveAttribute("min", "-24");
    expect(preamp).toHaveAttribute("max", "0");
    expect(preamp).toHaveAccessibleDescription(/boosts can clip/i);
    await advance(300);
    expect(write).not.toHaveBeenCalled();

    await act(async () => { fireEvent.click(eq.getByRole("button", { name: "Apply EQ" })); });
    expect(write).toHaveBeenLastCalledWith(target, { enabled: true, bands, preampDb: 0 });
    expect(eq.getByText(/Current preamp: 0 dB/)).toBeVisible();
    fireEvent.change(preamp, { target: { value: "-6.5" } });
    await advance(100);
    expect(write).toHaveBeenLastCalledWith(target, { enabled: true, bands, preampDb: -6.5 });
    expect(eq.getByText(/Current preamp: -6.5 dB/)).toBeVisible();

    fireEvent.change(mode, { target: { value: "auto" } });
    await advance(100);
    expect(write).toHaveBeenLastCalledWith(target, { enabled: true, bands, preampDb: null });
    expect(eq.getByText(/Current preamp: -7 dB/)).toBeVisible();
    fireEvent.change(mode, { target: { value: "manual" } });
    await advance(100);
    expect(write).toHaveBeenLastCalledWith(target, { enabled: true, bands, preampDb: 0 });
    expect(eq.getByRole("slider", { name: "Manual preamp" })).toHaveValue("0");
  });

  it("does not send a delayed trailing edit after another output is selected", async () => {
    const api = createDemoApi();
    const initial = await api.getSnapshot();
    const target = initial.audio.selectedOutput!;
    const other = initial.audio.outputs.find((node) => node.nodeName !== target)!.nodeName;
    await api.setEqualizer(target, { enabled: true, bands: equalizerPresets[0].bands });
    const { write, finish } = delayFirstWrite(api);
    const { eq } = await audioView(api);
    const picker = screen.getByRole("combobox", { name: "Audio output" });
    const gain = eq.getByRole("slider", { name: "31.5 Hz gain" });
    vi.useFakeTimers();

    fireEvent.change(gain, { target: { value: "4" } });
    await advance(100);
    fireEvent.change(gain, { target: { value: "8" } });
    expect(picker).toBeEnabled();
    fireEvent.change(picker, { target: { value: other } });
    expect(picker).toHaveValue(other);
    await finish();
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Apply output" })); });
    await advance(300);
    expect(write).toHaveBeenCalledOnce();
    const { audio } = await api.getSnapshot();
    expect(audio.selectedOutput).toBe(other);
    expect(audio.equalizers[target].bands[0].gainDb).toBe(4);
    expect(audio.equalizers[other]).toBeUndefined();
  });

  it.each(["reselecting the same output", "unmounting the panel"])("drops a queued live edit after %s", async (action) => {
    const api = createDemoApi();
    const initial = await api.getSnapshot();
    const target = initial.audio.selectedOutput!;
    const other = initial.audio.outputs.find((node) => node.nodeName !== target)!.nodeName;
    await api.setEqualizer(target, { enabled: true, bands: equalizerPresets[0].bands });
    const write = vi.spyOn(api, "setEqualizer");
    const { eq } = await audioView(api);
    vi.useFakeTimers();

    fireEvent.change(eq.getByRole("slider", { name: "31.5 Hz gain" }), { target: { value: "5" } });
    if (action === "unmounting the panel") {
      fireEvent.click(screen.getByRole("tab", { name: /Dashboard/ }));
      expect(screen.queryByRole("region", { name: "Headphone equalizer" })).not.toBeInTheDocument();
    } else {
      const picker = screen.getByRole("combobox", { name: "Audio output" });
      fireEvent.change(picker, { target: { value: other } });
      fireEvent.change(picker, { target: { value: target } });
      expect(eq.getByRole("slider", { name: "31.5 Hz gain" })).toHaveValue("5");
    }
    await advance(300);
    expect(write).not.toHaveBeenCalled();
  });

  it("recalls drafts and saved settings by output, including when surround owns the default", async () => {
    const api = createDemoApi();
    const initial = await api.getSnapshot();
    const headset = initial.audio.selectedOutput!;
    const speakers = initial.audio.outputs.find((node) => node.nodeName !== headset)!.nodeName;
    const write = vi.spyOn(api, "setEqualizer");
    const { user, eq } = await audioView(api);
    const preset = eq.getByRole("combobox", { name: "EQ preset" });
    const picker = screen.getByRole("combobox", { name: "Audio output" });
    const applyOutput = screen.getByRole("button", { name: "Apply output" });
    const applyEq = eq.getByRole("button", { name: "Apply EQ" });
    const surround = screen.getByRole("switch", { name: "Surround 7.1" });
    await user.selectOptions(preset, "Voice");
    await user.selectOptions(picker, speakers);
    expect(preset).toHaveValue("Neutral");
    expect(applyEq).toBeDisabled();
    await user.selectOptions(preset, "Gaming");
    await user.click(applyOutput);
    await user.click(applyEq);
    await user.selectOptions(picker, headset);
    expect(preset).toHaveValue("Voice");
    expect(applyEq).toBeDisabled();
    expect(surround).toBeDisabled();
    await user.click(applyOutput);
    await user.click(applyEq);
    expect(write.mock.calls).toEqual([
      [speakers, { enabled: true, bands: equalizerPresets[2].bands }],
      [headset, { enabled: true, bands: equalizerPresets[1].bands }],
    ]);
    await user.selectOptions(picker, speakers);
    expect(preset).toHaveValue("Gaming");
    await user.click(applyOutput);
    expect(eq.getByText("EQ active")).toBeVisible();
    await user.click(surround);
    expect(surround).toBeChecked();
    await user.click(eq.getByRole("button", { name: "Bypass EQ" }));
    expect(surround).toBeChecked();
    expect(applyOutput).toBeDisabled();
    await user.click(eq.getByRole("button", { name: "Re-enable saved EQ" }));
    await user.click(surround);
    expect(eq.getByText("EQ active")).toBeVisible();
    expect(surround).not.toBeChecked();
  });

  it("saves a plain stereo profile without an incidental HRTF and leaves later SOFA selections alone", async () => {
    const api = createDemoApi();
    const { user } = await audioView(api);
    await user.click(screen.getByText(/New profile/, { selector: "summary" }));
    await user.type(screen.getByRole("textbox", { name: "Profile name" }), "Desk stereo");
    await user.click(screen.getByRole("button", { name: "Save profile" }));
    const profile = (await api.getSnapshot()).profiles.find((entry) => entry.name === "Desk stereo");
    expect(profile).toMatchObject({ surroundRequested: false, hrtf: null });
    await user.click(screen.getByRole("button", { name: "Import SOFA HRTF" }));
    const imported = (await api.getSnapshot()).audio.hrtf;
    expect(imported?.reference).toMatch(/\.sofa$/);
    await user.click(screen.getByRole("button", { name: "Apply profile Desk stereo" }));
    expect((await api.getSnapshot()).audio.hrtf).toEqual(imported);
  });

  it("can include the desired output's confirmed EQ in a named profile without applying it", async () => {
    const api = createDemoApi();
    const initial = await api.getSnapshot();
    const headset = initial.audio.selectedOutput!;
    const speakers = initial.audio.outputs.find((node) => node.nodeName !== headset)!.nodeName;
    const saved = { enabled: false, bands: equalizerPresets[1].bands, preampDb: -6 };
    await api.setDefaultOutput(speakers);
    await api.setEqualizer(speakers, saved);
    await api.setDefaultOutput(headset);
    const write = vi.spyOn(api, "setEqualizer");
    const save = vi.spyOn(api, "saveAudioProfile");
    const apply = vi.spyOn(api, "applyAudioProfile");
    const { user, eq } = await audioView(api);
    await user.selectOptions(eq.getByRole("combobox", { name: "EQ preset" }), "Gaming");
    await user.click(screen.getByText(/New profile/, { selector: "summary" }));
    const include = screen.getByRole("switch", { name: "Include equalizer in profile" });
    expect(include).toBeDisabled();
    await user.selectOptions(screen.getByRole("combobox", { name: "Desired profile output" }), speakers);
    await user.click(include);
    await user.type(screen.getByRole("textbox", { name: "Profile name" }), "Desk voice");
    await user.click(screen.getByRole("button", { name: "Save profile" }));
    expect(save).toHaveBeenCalledWith(expect.objectContaining({ name: "Desk voice", outputNodeName: speakers, hrtf: null, equalizer: saved }));
    expect((await api.getSnapshot()).audio.selectedOutput).toBe(headset);
    expect(apply).not.toHaveBeenCalled();
    expect(write).not.toHaveBeenCalled();
  });

  it("restores Auto preamp for a saved legacy EQ profile with no preamp field", async () => {
    const api = createDemoApi();
    const initial = await api.getSnapshot();
    const legacy = initial.profiles[0];
    const equalizer = { enabled: true, bands: equalizerPresets[1].bands };
    await api.setEqualizer(legacy.outputNodeName, { ...equalizer, preampDb: -8 });
    await api.saveAudioProfile({ ...legacy, equalizer });
    const { user, eq } = await audioView(api);

    const mode = eq.getByRole("combobox", { name: "Preamp mode" });
    expect(mode).toHaveValue("manual");
    await user.click(screen.getByRole("button", { name: `Apply profile ${legacy.name}` }));
    expect(mode).toHaveValue("auto");
    expect(eq.getByText(/Current preamp: -4 dB/)).toBeVisible();
    expect((await api.getSnapshot()).audio.equalizers[legacy.outputNodeName]).toEqual(equalizer);
  });
});
