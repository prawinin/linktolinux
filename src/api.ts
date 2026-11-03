import { invoke } from "@tauri-apps/api/core";
import type {
  AdbDevice,
  CaptureMode,
  CaptureTelemetry,
  DeviceView,
  LanStatusView,
  SessionView,
} from "./types";
export const api = {
  filesChoose: () => invoke<unknown>("files_choose"),
  filesSend: (deviceId: string, paths: string[]) =>
    invoke<unknown>("files_send", { deviceId, paths }),
  filesList: () => invoke<unknown>("files_list"),
  filesCancel: (id: string) => invoke<void>("files_cancel", { id }),
  filesOpenFolder: () => invoke<void>("files_open_folder"),
  listDevices: () => invoke<DeviceView[]>("list_devices"),
  lanStart: () => invoke<LanStatusView>("lan_start"),
  lanResetPairing: () => invoke<LanStatusView>("lan_reset_pairing"),
  lanStop: () => invoke<void>("lan_stop"),
  lanStatus: () => invoke<LanStatusView>("lan_status"),
  companionRequest: (
    deviceId: string,
    command: string,
    args: Record<string, unknown> = {},
  ) => invoke<unknown>("companion_request", { deviceId, command, args }),
  companionSavePhoto: (deviceId: string, photoId: string) =>
    invoke<string>("companion_save_photo", { deviceId, photoId }),
  enableWebcam: () => invoke<string>("companion_enable_webcam"),
  clipboardSync: () => invoke<unknown>("companion_clipboard_sync"),
  setClipboardSync: (enabled: boolean) =>
    invoke<unknown>("companion_set_clipboard_sync", { enabled }),
  readClipboard: () => invoke<string>("companion_read_clipboard"),
  writeClipboard: (text: string) =>
    invoke<void>("companion_write_clipboard", { text }),
  listAdbDevices: () => invoke<AdbDevice[]>("list_adb_devices"),
  adbPair: (address: string, code: string) =>
    invoke<string>("adb_pair", { address, code }),
  adbConnect: (address: string) => invoke<string>("adb_connect", { address }),
  startCapture: (
    serial: string,
    mode: CaptureMode,
    options?: { maxSize?: number; maxFps?: number; preview?: boolean },
  ) =>
    invoke<SessionView>("start_capture", {
      serial,
      mode,
      maxSize: options?.maxSize ?? null,
      maxFps: options?.maxFps ?? null,
      videoDevice: null,
      preview: options?.preview ?? null,
    }),
  stopCapture: () => invoke<void>("stop_capture"),
  captureStatus: () => invoke<CaptureTelemetry>("capture_status"),
  mirrorTap: (x: number, y: number, width: number, height: number) =>
    invoke<void>("mirror_tap", { x, y, width, height }),
  mirrorKey: (keycode: number) => invoke<void>("mirror_key", { keycode }),
  screenshot: (serial: string) => invoke<string>("screenshot", { serial }),
};
export const KEYCODE_BACK = 4;
export const KEYCODE_HOME = 3;
export const KEYCODE_APP_SWITCH = 187;
