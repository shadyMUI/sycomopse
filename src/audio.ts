import type { AudioNode, AudioSnapshot, EqualizerSettings } from "./types";

export const managedFilterPrefix = "peripheral-hub.surround.";
export const liveEqualizerMutationPrefix = "audio/equalizer-live/";
export const minManualPreampDb = -24;

export function preferredOutputName(audio: AudioSnapshot): string {
  return [audio.selectedOutput, audio.defaultOutput].find((name) => audio.outputs.some((output) => output.nodeName === name))
    ?? audio.outputs[0]?.nodeName ?? "";
}

export function isStereoOutput(node: AudioNode | undefined): node is AudioNode {
  return !!node && !node.isVirtual && node.mediaClass.startsWith("Audio/Sink") && node.objectSerial !== null
    && node.channels.length === 2 && node.channels.includes("FL") && node.channels.includes("FR");
}

export function isOutputApplied(audio: AudioSnapshot, node: AudioNode): boolean {
  if (node.isVirtual) return node.nodeName === audio.defaultOutput;
  if (node.nodeName !== audio.selectedOutput) return false;
  if (audio.ownedFilterOutput === null) {
    return node.nodeName === audio.defaultOutput && (audio.configuredDefaultOutput === null || node.nodeName === audio.configuredDefaultOutput);
  }
  // Owning a filter is not proof it is routed, even if the system now uses its physical target directly.
  return audio.ownedFilterOutput === node.nodeName && (audio.surroundState === "on" || audio.equalizerOutput === node.nodeName)
    && !!audio.defaultOutput?.startsWith(managedFilterPrefix)
    && audio.configuredDefaultOutput === audio.defaultOutput
    && audio.outputs.some((output) => output.nodeName === audio.defaultOutput && output.isVirtual);
}

export function filterTargetReason(audio: AudioSnapshot, output: AudioNode | undefined): string | null {
  if (!audio.available) return "Audio service unavailable.";
  if (!isStereoOutput(output)) return "Select an identifiable physical stereo output (FL/FR).";
  if (!isOutputApplied(audio, output)) return 'Choose "Apply output" to confirm this playback device.';
  return null;
}

export const bandFrequencies = [31.5, 63, 125, 250, 500, 1000, 2000, 4000, 8000, 16000] as const;
export const equalizerPresets = [
  { name: "Neutral", gains: bandFrequencies.map(() => 0) },
  { name: "Voice", gains: [-3, -3, -2, -1, 0, 1, 2, 1, -1, -2] },
  { name: "Gaming", gains: [2, 1, 0, -1, -2, 0, 1, 2, 1, 0] },
].map(({ name, gains }) => ({ name, bands: gains.map((gainDb) => ({ gainDb, q: 1.4 })) }));

export function isValidManualPreamp(preampDb: unknown): preampDb is number {
  return typeof preampDb === "number" && Number.isFinite(preampDb) && preampDb >= minManualPreampDb && preampDb <= 0;
}

export function validEqualizer(settings: EqualizerSettings): boolean {
  return typeof settings?.enabled === "boolean" && (settings.preampDb == null || isValidManualPreamp(settings.preampDb))
    && Array.isArray(settings.bands) && settings.bands.length === bandFrequencies.length
    && bandFrequencies.every((_, i) => {
      const band = settings.bands[i];
      return band && Number.isFinite(band.gainDb) && band.gainDb >= -12 && band.gainDb <= 12
        && Number.isFinite(band.q) && band.q >= 0.4 && band.q <= 4;
    });
}

export function automaticPreamp(bands: EqualizerSettings["bands"]): number {
  return bands.reduce((preamp, { gainDb }) => preamp - Math.max(0, gainDb), 0);
}
