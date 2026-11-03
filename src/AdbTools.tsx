import { useCallback, useEffect, useRef, useState } from "react";
import {
  Camera,
  MonitorSmartphone,
  RefreshCw,
  Square,
  Usb,
} from "lucide-react";
import { api, KEYCODE_BACK, KEYCODE_HOME, KEYCODE_APP_SWITCH } from "./api";
import type { AdbDevice, CaptureMode, SessionView } from "./types";

export default function AdbTools() {
  const [devices, setDevices] = useState<AdbDevice[]>([]);
  const [serial, setSerial] = useState("");
  const [pairAddress, setPairAddress] = useState("");
  const [code, setCode] = useState("");
  const [address, setAddress] = useState("");
  const [mode, setMode] = useState<CaptureMode>("mirror");
  const [session, setSession] = useState<SessionView | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const ownsSession = useRef(false);
  const refresh = useCallback(async () => {
    try {
      const list = await api.listAdbDevices();
      setDevices(list);
      setSerial((current) =>
        list.some((item) => item.serial === current)
          ? current
          : (list.find((item) => item.state === "device")?.serial ?? ""),
      );
    } catch (failure) {
      setDevices([]);
      setError(String(failure));
    }
  }, []);
  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), 5000);
    return () => {
      window.clearInterval(timer);
      if (ownsSession.current)
        void api
          .stopCapture()
          .catch((failure: unknown) =>
            console.error("Could not stop ADB capture:", String(failure)),
          );
    };
  }, [refresh]);
  useEffect(() => {
    if (!session) return;
    const timer = window.setInterval(() => {
      void api
        .captureStatus()
        .then((status) => {
          if (!status.active) {
            setSession(null);
            ownsSession.current = false;
          }
        })
        .catch((failure: unknown) => setError(String(failure)));
    }, 2000);
    return () => window.clearInterval(timer);
  }, [session]);
  const perform = async (action: () => Promise<void>) => {
    if (busy) return;
    setBusy(true);
    setError("");
    setMessage("");
    try {
      await action();
    } catch (failure) {
      setError(String(failure));
    } finally {
      setBusy(false);
    }
  };
  const ready = devices.some(
    (item) => item.serial === serial && item.state === "device",
  );
  return (
    <div className="companion-shell companion-adb">
      <main className="companion-main">
        <header className="companion-header">
          <div>
            <span className="companion-breadcrumb">Advanced connection</span>
            <h1>ADB workspace</h1>
          </div>
          <Usb size={25} />
        </header>
        <div className="companion-content">
          {error && (
            <div className="companion-alert" role="alert">
              {error}
            </div>
          )}
          {message && (
            <div className="companion-notice" role="status">
              {message}
            </div>
          )}
          <div className="companion-adb-grid">
            <section className="companion-panel">
              <h2>Connect with USB or wireless debugging</h2>
              <p>
                For USB, enable USB debugging and accept the authorization
                dialog on your phone. For wireless, enable Developer options and
                Wireless debugging.
              </p>
              <label
                className="companion-field-label"
                htmlFor="adb-pair-address"
              >
                Pairing address and port from “Pair device with pairing code”
              </label>
              <input
                id="adb-pair-address"
                value={pairAddress}
                onChange={(event) => setPairAddress(event.target.value)}
                placeholder="Phone address:pairing port"
              />
              <label className="companion-field-label" htmlFor="adb-code">
                Six-digit pairing code
              </label>
              <input
                id="adb-code"
                value={code}
                inputMode="numeric"
                maxLength={6}
                onChange={(event) =>
                  setCode(event.target.value.replace(/\D/g, ""))
                }
                autoComplete="off"
              />
              <button
                className="companion-button secondary"
                disabled={busy || code.length !== 6 || !pairAddress.trim()}
                onClick={() =>
                  void perform(async () => {
                    setMessage(await api.adbPair(pairAddress.trim(), code));
                    setCode("");
                  })
                }
              >
                Pair wireless debugging
              </button>
              <label className="companion-field-label" htmlFor="adb-address">
                Connection address and port from the main Wireless debugging
                screen
              </label>
              <input
                id="adb-address"
                value={address}
                onChange={(event) => setAddress(event.target.value)}
                placeholder="Phone address:connection port"
              />
              <button
                className="companion-button primary"
                disabled={busy || !address.trim()}
                onClick={() =>
                  void perform(async () => {
                    setMessage(await api.adbConnect(address.trim()));
                    await refresh();
                  })
                }
              >
                Connect
              </button>
              <p className="companion-helper">
                The pairing and connection ports are different. Use the values
                currently displayed by Android.
              </p>
            </section>
            <section className="companion-panel">
              <div className="companion-section-heading">
                <div>
                  <h2>Phone screen and capture</h2>
                  <p>Uses the bundled scrcpy server.</p>
                </div>
                <button
                  className="companion-icon-button"
                  aria-label="Refresh ADB devices"
                  onClick={() => void refresh()}
                >
                  <RefreshCw size={18} />
                </button>
              </div>
              <label className="companion-field-label" htmlFor="adb-device">
                Device
              </label>
              <select
                id="adb-device"
                value={serial}
                disabled={Boolean(session)}
                onChange={(event) => setSerial(event.target.value)}
              >
                <option value="">Select a device</option>
                {devices.map((item) => (
                  <option key={item.serial} value={item.serial}>
                    {item.model ?? item.serial} ({item.state})
                  </option>
                ))}
              </select>
              {!ready && (
                <p className="companion-helper">
                  Connect a phone and accept USB debugging authorization before
                  starting.
                </p>
              )}
              <label className="companion-field-label" htmlFor="adb-mode">
                Capture mode
              </label>
              <select
                id="adb-mode"
                value={mode}
                disabled={Boolean(session)}
                onChange={(event) => setMode(event.target.value as CaptureMode)}
              >
                <option value="mirror">Phone screen</option>
                <option value="studio">Camera and microphone</option>
                <option value="webcam">Virtual webcam</option>
                <option value="mic">Virtual microphone</option>
              </select>
              <button
                className={`companion-button ${session ? "stop" : "primary"}`}
                disabled={busy || !ready}
                onClick={() =>
                  void perform(async () => {
                    if (session) {
                      await api.stopCapture();
                      ownsSession.current = false;
                      setSession(null);
                      setMessage("ADB capture stopped.");
                    } else {
                      const view = await api.startCapture(serial, mode, {
                        maxSize: 1280,
                        maxFps: 24,
                        preview: true,
                      });
                      ownsSession.current = true;
                      setSession(view);
                    }
                  })
                }
              >
                {session ? (
                  <Square size={16} />
                ) : (
                  <MonitorSmartphone size={17} />
                )}{" "}
                {busy
                  ? "Waiting for device"
                  : session
                    ? "Stop capture"
                    : "Start capture"}
              </button>
              <button
                className="companion-button secondary"
                disabled={busy || !ready}
                onClick={() =>
                  void perform(async () =>
                    setMessage(
                      `Screenshot saved to ${await api.screenshot(serial)}`,
                    ),
                  )
                }
              >
                <Camera size={16} />
                Save screenshot
              </button>
              {session?.previewUrl && (
                <img
                  className="companion-adb-preview"
                  src={session.previewUrl}
                  alt="Live Android screen; click to tap"
                  onClick={(event) => {
                    if (session.mode !== "mirror") return;
                    const bounds = event.currentTarget.getBoundingClientRect();
                    const x = Math.round(
                      ((event.clientX - bounds.left) / bounds.width) *
                        session.width,
                    );
                    const y = Math.round(
                      ((event.clientY - bounds.top) / bounds.height) *
                        session.height,
                    );
                    void api
                      .mirrorTap(x, y, session.width, session.height)
                      .catch((failure: unknown) => setError(String(failure)));
                  }}
                />
              )}
              {session?.mode === "mirror" && (
                <div className="companion-clipboard-actions">
                  {[
                    ["Back", KEYCODE_BACK],
                    ["Home", KEYCODE_HOME],
                    ["Recent apps", KEYCODE_APP_SWITCH],
                  ].map(([label, key]) => (
                    <button
                      key={label}
                      className="companion-button secondary"
                      onClick={() =>
                        void api
                          .mirrorKey(Number(key))
                          .catch((failure: unknown) =>
                            setError(String(failure)),
                          )
                      }
                    >
                      {label}
                    </button>
                  ))}
                </div>
              )}
              {session?.audioSource && (
                <p className="companion-helper">
                  Microphone source: {session.audioSource}
                </p>
              )}
              {session?.v4l2Device && (
                <p className="companion-helper">
                  Webcam device: {session.v4l2Device}
                </p>
              )}
            </section>
          </div>
        </div>
      </main>
    </div>
  );
}
