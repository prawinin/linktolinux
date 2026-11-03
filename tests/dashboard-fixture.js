// Test-only desktop bridge; never loaded by the application.
window.__testCalls = [];
window.__testCapture = null;
window.__testClipboard = "Linux clipboard text";
window.__testClipboardSync = localStorage.getItem("linktolinux.test.clipboardSync") !== "false";
window.__testClipboardPreferenceDelay = 0;
window.__testClipboardPreferenceFailure = false;
window.__testConnected = true;
window.__testPairingCode = "ABCDEF123456";
window.__testPhotoFailure = false;
window.__testFilesSupported = true;
window.__testFiles = [];
window.__testChosenFiles = ["/test-downloads/report.pdf"];
window.__testFileFailure = "";
window.__testCallbacks = new Map();
window.__testListeners = new Map();
let callbackId = 0;
window.__testEmit = (event, payload) => {
  for (const [id, listener] of window.__testListeners) {
    if (listener.event === event) window.__testCallbacks.get(listener.handler)?.({event, id, payload});
  }
};
window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
  unregisterListener(_event, id) { window.__testListeners.delete(id); },
};
window.__TAURI_INTERNALS__ = {
  metadata: {currentWindow: {label: "main"}, currentWebview: {label: "main"}},
  transformCallback(callback) {
    const id = ++callbackId;
    window.__testCallbacks.set(id, callback);
    return id;
  },
  unregisterCallback(id) { window.__testCallbacks.delete(id); },
  async invoke(command, args = {}) {
    window.__testCalls.push({ command, args });
    if (command === "lan_reset_pairing") {
      window.__testConnected = false;
      window.__testPairingCode = "FEDCBA654321";
      return window.__TAURI_INTERNALS__.invoke("lan_status");
    }
    if (command === "lan_status" || command === "lan_start")
      return {
        listening: true,
        port: 1718,
        peers: window.__testConnected ? 1 : 0,
        previewUrl: null,
        v4l2Device: null,
        audioSource: window.__testCapture === "mic" ? "linktolinux_mic" : null,
        downloadUrl: "http://192.0.2.10:1718/download/companion.apk",
        lanIp: "192.0.2.10",
        pairingCode: window.__testPairingCode,
        captureMode: window.__testCapture,
        captureDeviceId: window.__testCapture ? "test-phone" : null,
        lastError: null,
      };
    if (command === "list_devices")
      return [
        {
          id: "test-phone",
          name: "Android test device",
          model: "Android 16",
          protocolVersion: 2,
          incomingCapabilities: window.__testFilesSupported ? ["files-v1"] : [],
          connected: window.__testConnected,
          battery: { charge: 76, charging: false },
        },
      ];
    if (command === "list_adb_devices") return [];
    if (command === "plugin:event|listen") {
      const id = ++callbackId;
      window.__testListeners.set(id, {event: args.event, handler: args.handler});
      return id;
    }
    if (command === "plugin:event|unlisten") {
      window.__testListeners.delete(args.eventId);
      return;
    }
    if (command.startsWith("files_") && window.__testFileFailure) throw window.__testFileFailure;
    if (command === "files_choose") return window.__testChosenFiles;
    if (command === "files_list") return window.__testFiles;
    if (command === "files_open_folder") return;
    if (command === "files_send") {
      if (!window.__testConnected || !window.__testFilesSupported) throw "Connect an updated companion before sending files";
      const items = args.paths.map((path) => ({
        id: crypto.randomUUID(), deviceId: args.deviceId, name: path.split("/").at(-1),
        direction: "send", size: 4096, transferred: 0, status: "queued",
        error: null, savedPath: null, startedAt: Date.now(), finishedAt: null,
      }));
      window.__testFiles.push(...items);
      return items;
    }
    if (command === "files_cancel") {
      const item = window.__testFiles.find((entry) => entry.id === args.id);
      if (!item) throw "Unknown transfer";
      item.status = "cancelled";
      item.finishedAt = Date.now();
      return;
    }
    if (command === "companion_clipboard_sync" || command === "companion_set_clipboard_sync") {
      if (window.__testClipboardPreferenceDelay)
        await new Promise((resolve) => setTimeout(resolve, window.__testClipboardPreferenceDelay));
      if (window.__testClipboardPreferenceFailure) throw "Clipboard preference storage unavailable";
      if (command === "companion_set_clipboard_sync") {
        if (typeof args.enabled !== "boolean") throw "Invalid clipboard preference";
        window.__testClipboardSync = args.enabled;
        localStorage.setItem("linktolinux.test.clipboardSync", String(args.enabled));
      }
      return window.__testClipboardSync;
    }
    if (command === "companion_read_clipboard") return window.__testClipboard;
    if (command === "companion_write_clipboard") {
      window.__testClipboard = args.text;
      return;
    }
    if (command === "companion_save_photo")
      return "/test-downloads/LinkToLinux/camera.jpg";
    if (command === "companion_request") {
      switch (args.command) {
        case "gallery_list":
          if (window.__testPhotoFailure)
            throw "Photo access denied. Open Manage permissions on the phone and allow photos.";
          return {
            photos: Array.from({ length: 6 }, (_, i) => ({
              id: String(i + 1),
              name: `Camera_${i + 1}.jpg`,
              size: 1024,
              modified: 1700000000000,
            })),
            hasMore: false,
          };
        case "gallery_thumbnail":
          return { dataUrl: "data:image/jpeg;base64,/9j/4AAQSkZJRgABAgAAAQABAAD//gAPTGF2YzYzLjEuMTAyAP/bAEMACAQEBAQEBQUFBQUFBgYGBgYGBgYGBgYGBgcHBwgICAcHBwYGBwcICAgICQkJCAgICAkJCgoKDAwLCw4ODhERFP/EAE0AAQEAAAAAAAAAAAAAAAAAAAAGAQEBAQAAAAAAAAAAAAAAAAAABQYQAQAAAAAAAAAAAAAAAAAAAAARAQAAAAAAAAAAAAAAAAAAAAD/wAARCAAwAEADASIAAhEAAxEA/9oADAMBAAIRAxEAPwCUAU2LAAAAAAAAAAAAAAAAAAAAAAf/2Q==" };
        case "capture_start":
          window.__testCapture = args.args.mode;
          return { mode: args.args.mode };
        case "capture_stop":
          window.__testCapture = null;
          return {};
        case "clipboard_get":
          return { text: "Phone clipboard text" };
        case "clipboard_set":
          return {};
        default:
          throw "Unexpected companion command";
      }
    }
    if (command === "lan_stop") {
      window.__testConnected = false;
      return;
    }
    throw `Unexpected desktop command: ${command}`;
  },
};
