import { useId } from "react";
import type { Device, DeviceKind } from "../types";
import { BatteryReadout, deviceStatus, providerNames } from "./Controls";

export function DeviceIllustration({ kind, className = "" }: { kind: DeviceKind; className?: string }) {
  const id = useId().replaceAll(":", "");
  const mouse = kind === "mouse";
  return (
    <svg className={`device-illustration ${className}`} viewBox="0 0 240 200" fill="none" aria-hidden="true">
      <defs>
        <linearGradient id={`shell-${id}`} x1="70" y1="20" x2="174" y2="175" gradientUnits="userSpaceOnUse"><stop stopColor="#343434" /><stop offset="0.5" stopColor="#202020" /><stop offset="1" stopColor="#151515" /></linearGradient>
        <linearGradient id={`edge-${id}`} x1="75" y1="30" x2="160" y2="170" gradientUnits="userSpaceOnUse"><stop stopColor="#b0b0b0" /><stop offset="0.5" stopColor="#555" /><stop offset="1" stopColor="#363636" /></linearGradient>
        <radialGradient id={`floor-${id}`}><stop stopColor="#44d62c" stopOpacity="0.12" /><stop offset="1" stopColor="#44d62c" stopOpacity="0" /></radialGradient>
      </defs>
      <ellipse cx="120" cy="179" rx="84" ry="13" fill={`url(#floor-${id})`} />
      <path d="M30 158H49M191 158H210M120 9V16" stroke="#414141" strokeWidth="1" />
      {mouse ? (
        <g transform="rotate(-12 120 103)">
          <path d="M121 24C91 24 78 46 73 79L67 127C64 158 86 179 120 179C153 179 177 159 173 127L167 79C162 45 150 24 121 24Z" fill={`url(#shell-${id})`} stroke={`url(#edge-${id})`} strokeWidth="1.5" />
          <path d="M120 27V89M78 80C91 84 103 86 120 87C136 87 151 83 165 79M120 89V100" stroke="#696969" strokeWidth="1.1" />
          <path d="M86 53C80 70 78 94 77 108M154 51C159 67 162 92 163 111" stroke="#ddd" strokeOpacity="0.25" />
          <rect x="114" y="45" width="12" height="25" rx="5" fill="#0c0c0c" stroke="#777" />
          <path d="M117 50H123M117 55H123M117 60H123M117 65H123" stroke="#44d62c" strokeOpacity="0.8" />
          <path d="M72 104L75 93L79 94L77 114" stroke="#6c6c6c" strokeWidth="2" />
          <path d="M99 160C112 165 135 164 145 158" stroke="#44d62c" strokeOpacity="0.65" />
          <circle cx="120" cy="122" r="3" stroke="#444" />
        </g>
      ) : kind === "headset" ? (
        <g>
          <path d="M65 110V83C65 48 89 25 120 25C151 25 175 48 175 83V110" stroke="#777" strokeWidth="13" />
          <path d="M66 91V81C66 49 90 27 120 27C150 27 174 49 174 81V91" stroke={`url(#edge-${id})`} strokeWidth="9" />
          <path d="M80 80C81 56 98 42 120 42C142 42 159 56 160 80" stroke="#252525" strokeWidth="10" />
          <path d="M66 86V112M174 86V112" stroke="#44d62c" strokeWidth="2" />
          <rect x="47" y="95" width="38" height="65" rx="15" transform="rotate(-8 47 95)" fill={`url(#shell-${id})`} stroke={`url(#edge-${id})`} strokeWidth="1.5" />
          <rect x="158" y="90" width="38" height="65" rx="15" transform="rotate(8 158 90)" fill={`url(#shell-${id})`} stroke={`url(#edge-${id})`} strokeWidth="1.5" />
          <path d="M61 108L65 145M178 108L173 145" stroke="#555" strokeWidth="1.5" />
          <path d="M52 145C41 168 62 181 90 172" stroke="#777" strokeWidth="3" strokeLinecap="round" />
          <rect x="85" y="167" width="14" height="7" rx="3.5" transform="rotate(-14 85 167)" fill="#404040" stroke="#44d62c" strokeWidth="1" />
        </g>
      ) : (
        <g><rect x="79" y="54" width="82" height="99" rx="13" fill={`url(#shell-${id})`} stroke={`url(#edge-${id})`} /><path d="M106 54V37H134V54" stroke="#777" strokeWidth="2" /><circle cx="120" cy="100" r="16" stroke="#444" /><circle cx="120" cy="100" r="5" fill="#44d62c" /><path d="M97 132H143" stroke="#555" /></g>
      )}
    </svg>
  );
}

export function DeviceCard({ device, selected, onOpen }: { device: Device; selected: boolean; onOpen: (device: Device) => void }) {
  const status = deviceStatus(device);
  const actionable = device.kind === "mouse" || device.kind === "headset";
  const brand = device.brand === "razer" ? "Razer" : device.brand === "logitech" ? "Logitech" : "Periferica";
  return (
    <article className={`device-card${selected ? " selected" : ""}`} data-testid="device-card">
      <button type="button" className="device-card-button" aria-label={`Apri controlli di ${device.name}`} disabled={!actionable} onClick={() => onOpen(device)}>
        <div className="device-card-top"><span className="tiny-label">{brand} / {device.kind === "mouse" ? "Mouse" : device.kind === "headset" ? "Cuffie" : "Ricevitore"}</span><span className={`status-dot ${status.tone}`} aria-hidden="true" /></div>
        <DeviceIllustration kind={device.kind} />
        <div className="device-card-copy"><h3>{device.name}</h3><span className={`status-pill ${status.tone}`}>{status.label}</span><p>{providerNames[device.provider]}{device.identity.transport ? ` / ${device.identity.transport}` : ""}</p></div>
        <div className="device-card-bottom"><BatteryReadout battery={device.values.battery} compact /><span className="card-arrow" aria-hidden="true">{actionable ? ">" : ""}</span></div>
      </button>
    </article>
  );
}
