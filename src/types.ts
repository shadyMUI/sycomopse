export type ProviderKind = "openrazer" | "ratbag" | "upower" | "headsetcontrol" | "pipewire" | "inputremapper" | "usb" | "app";
export type AvailabilityCode = "available" | "absent" | "unsupported" | "providerMissing" | "permissionDenied" | "unknown" | "unsupportedPlatform";
export type Brand = "razer" | "logitech" | "generic";
export type DeviceKind = "mouse" | "headset" | "receiver" | "unknown";
export type DpiValues = { kind: "range"; min: number; max: number; step: number } | { kind: "discrete"; values: number[] } | { kind: "unknown" };
export interface DpiCapability { values: DpiValues; writable: boolean; separateAxes: boolean; reason: string | null }
export interface PollingCapability { values: number[]; writable: boolean; reason: string | null }
export interface RemappingCapability {
  provider: ProviderKind; availability: AvailabilityCode; writable: boolean;
  fingerprint: string | null; identityStrength: "serial" | "port" | null;
  buttons: { id: string; label: string; code: number }[];
  keyCodes: number[]; reason: string | null;
}
export type MouseAction = { kind: "default" } | { kind: "mouseButton"; code: number }
  | { kind: "key"; code: number } | { kind: "chord"; modifiers: number[]; key: number }
  | { kind: "autoclicker"; cps: number; button?: number } | { kind: "disabled" };
export interface MouseBinding { buttonId: string; action: MouseAction }
export interface RemappingValues {
  state: "unavailable" | "inactive" | "starting" | "active" | "busy" | "error" | "paused";
  enabled: boolean; autoApply: boolean; pausedForSafety: boolean; profileId: string | null;
  bindings: MouseBinding[]; activePreset: string | null; message: string | null;
}
export interface Battery {
  percentage: number | null;
  level: "unknown" | "critical" | "low" | "normal" | "high" | "full" | null;
  charging: boolean | null;
  source: ProviderKind;
  // Unix epoch seconds, not JavaScript milliseconds.
  observedAt: number;
  sourceUpdatedAt: number | null;
  stale: boolean;
}
export interface DeviceIdentity {
  serial: string | null; physicalPath: string | null;
  vendorId: number | null; productId: number | null;
  transport: string | null; firmware: string | null; backendPath: string | null;
}
export interface Device {
  id: string; name: string; brand: Brand; kind: DeviceKind; provider: ProviderKind;
  identity: DeviceIdentity; availability: AvailabilityCode;
  support: "supported" | "partial" | "unsupported";
  capabilities: { dpi: DpiCapability | null; polling: PollingCapability | null; remapping: RemappingCapability | null };
  values: { dpi: { x: number; y: number } | null; pollingRate: number | null; battery: Battery | null; remapping: RemappingValues | null };
  outputNodeNames: string[]; limitations: string[];
}
export interface AudioNode {
  id: number; objectSerial: number | null; nodeName: string; description: string; mediaClass: string;
  channels: string[]; volume: number | null; muted: boolean | null; isVirtual: boolean;
  deviceSerial: string | null; deviceBusPath: string | null;
  vendorId: number | null; productId: number | null; transport: string | null;
}
export type SurroundState = "off" | "starting" | "on" | "stopping" | "error";
export interface AudioSnapshot {
  available: boolean; outputs: AudioNode[]; inputs: AudioNode[];
  defaultOutput: string | null; configuredDefaultOutput: string | null;
  selectedOutput: string | null; defaultInput: string | null;
  surroundState: SurroundState; canEnableSurround: boolean; surroundReason: string | null;
  hrtf: { reference: string; displayName: string; readable: boolean } | null;
}
export interface AudioProfile {
  schemaVersion: 1; id: string; name: string; outputNodeName: string; hrtf: string | null;
  volume: number; mute: boolean; surroundRequested: boolean;
}
export interface Diagnostic {
  id: string; provider: ProviderKind; code: AvailabilityCode; severity: "info" | "warning" | "error";
  message: string; detail: string | null;
}
export interface Snapshot {
  revision: number; platform: "linux" | "unsupportedPlatform"; demo: boolean;
  devices: Device[]; audio: AudioSnapshot; profiles: AudioProfile[]; diagnostics: Diagnostic[];
}
export interface AppError { code: string; message: string; detail: string | null }
export interface HubApi {
  readonly demo: boolean;
  getSnapshot(): Promise<Snapshot>;
  refreshDevices(): Promise<Snapshot>;
  setDpi(deviceId: string, x: number, y: number): Promise<Snapshot>;
  setPollingRate(deviceId: string, rate: number): Promise<Snapshot>;
  applyMouseRemapping(deviceId: string, bindings: MouseBinding[], autoApply: boolean, allowWeakIdentity: boolean): Promise<Snapshot>;
  disableMouseRemapping(deviceId: string): Promise<Snapshot>;
  setDefaultOutput(nodeName: string): Promise<Snapshot>;
  setVolume(nodeName: string, volume: number): Promise<Snapshot>;
  setMute(nodeName: string, muted: boolean): Promise<Snapshot>;
  setSurround(enabled: boolean): Promise<Snapshot>;
  importHrtf(): Promise<Snapshot>;
  saveAudioProfile(profile: AudioProfile): Promise<Snapshot>;
  applyAudioProfile(profileId: string): Promise<Snapshot>;
  subscribe(listener: (snapshot: Snapshot) => void): Promise<() => void>;
  exitApp(): Promise<void>;
}
