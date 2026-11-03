export interface BatteryState {
  charge: number;
  charging: boolean;
  thresholdEvent: number;
  batteryQuantity: number;
  updatedAt: number;
}

export interface DeviceView {
  id: string;
  name: string;
  deviceType: string;
  protocolVersion: number;
  incomingCapabilities: string[];
  outgoingCapabilities: string[];
  address: string | null;
  tcpPort: number | null;
  paired: boolean;
  connected: boolean;
  fingerprint: string | null;
  fingerprintDisplay: string | null;
  battery: BatteryState;
  model: string | null;
  lastSeen: number;
}

export interface AdbDevice {
  serial: string;
  state: string;
  model: string | null;
  product: string | null;
  device: string | null;
  transportId: string | null;
  connection: string;
}

export interface CaptureTelemetry {
  active: boolean;
  mode: string;
  serial: string | null;
  codec: string | null;
  width: number;
  height: number;
  fps: number;
  v4l2Device: string | null;
  audioSink: string | null;
  preview: boolean;
  startedAt: number;
  frames: number;
  dropped: number;
}

export interface LanStatusView {
  listening: boolean;
  port: number;
  peers: number;
  previewUrl: string | null;
  v4l2Device: string | null;
  audioSource: string | null;
  downloadUrl: string;
  lanIp: string;
  pairingCode: string;
  captureMode: string | null;
  captureDeviceId: string | null;
  lastError: string | null;
}

export interface SessionView {
  serial: string;
  deviceName: string;
  mode: string;
  videoCodec: string | null;
  audioCodec: string | null;
  width: number;
  height: number;
  previewUrl: string | null;
  v4l2Device: string | null;
  audioSource: string | null;
  frames: number;
}

export type CaptureMode = "mirror" | "webcam" | "mic" | "studio";
