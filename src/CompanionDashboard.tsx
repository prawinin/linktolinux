import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { listen } from "@tauri-apps/api/event";
import {
  Battery,
  Camera,
  Check,
  ChevronRight,
  Clipboard,
  Copy,
  Download,
  Images,
  File as FileIcon,
  LayoutDashboard,
  Mic,
  Monitor,
  RefreshCw,
  Settings2,
  Smartphone,
  Square,
  Usb,
  Wifi,
  X,
} from "lucide-react";
import { api } from "./api";
import FilesPanel from "./FilesPanel";
const AdvancedApp = lazy(() => import("./AdbTools"));
import {
  parseClipboard,
  parsePhotoPage,
  parseThumbnail,
} from "./companion-protocol";
import type { CompanionPhoto } from "./companion-protocol";
import type { DeviceView, LanStatusView } from "./types";
import { generateQrMatrix, generateQrSvgPath } from "./lib/qr";
import { parseClipboardSyncPreference, clipboardRetryDelay, canSendClipboard } from "./clipboard-policy";
import "./companion.css";

type Section = "overview" | "files" | "photos" | "capture" | "clipboard" | "connection";
type Mode = "webcam" | "mic" | "studio";
interface PhotoView extends CompanionPhoto {
  thumbnail?: string;
  failed?: boolean;
}
const sections = [
  { id: "overview", label: "Overview", icon: LayoutDashboard },
  { id: "files", label: "Files", icon: FileIcon },
  { id: "photos", label: "Photos", icon: Images },
  { id: "capture", label: "Camera & mic", icon: Camera },
  { id: "clipboard", label: "Clipboard", icon: Clipboard },
  { id: "connection", label: "Connection", icon: Wifi },
] as const;

export default function CompanionDashboard() {
  const [advanced, setAdvanced] = useState(false);
  const [section, setSection] = useState<Section>("overview");
  const [status, setStatus] = useState<LanStatusView | null>(null);
  const [devices, setDevices] = useState<DeviceView[]>([]);
  const [deviceId, setDeviceId] = useState("");
  const [photos, setPhotos] = useState<PhotoView[]>([]);
  const [hasMore, setHasMore] = useState(false);
  const [photoBusy, setPhotoBusy] = useState(false);
  const [saving, setSaving] = useState<string | null>(null);
  const [captureBusy, setCaptureBusy] = useState(false);
  const [webcamBusy, setWebcamBusy] = useState(false);
  const [mode, setMode] = useState<Mode>("studio");
  const [text, setText] = useState("");
  const [clipboardBusy, setClipboardBusy] = useState(false);
  const [autoSync, setAutoSync] = useState<boolean | null>(null);
  const [clipboardPreferenceBusy, setClipboardPreferenceBusy] = useState(false);
  const [clipboardPreferenceError, setClipboardPreferenceError] = useState("");
  const [clipboardSyncError, setClipboardSyncError] = useState("");
  const [message, setMessage] = useState<{
    text: string;
    error: boolean;
  } | null>(null);
  const [resetPairingOpen, setResetPairingOpen] = useState(false);
  const [resetPairingBusy, setResetPairingBusy] = useState(false);
  const [connectionError, setConnectionError] = useState("");
  const [previewError, setPreviewError] = useState(false);
  const target = useRef("");
  const clipboardLast = useRef<string | null>(null);
  const loadingPhotos = useRef<string | null>(null);
  const photoGeneration = useRef(0);
  const pollBusy = useRef(false);
  const device =
    devices.find((item) => item.id === deviceId) ??
    devices.find((item) => item.connected) ??
    devices[0];
  const connected = device?.connected ?? false;
  const active = status?.captureMode ?? null;
  const otherCapture = Boolean(
    active && status?.captureDeviceId && status.captureDeviceId !== device?.id,
  );
  target.current = connected ? (device?.id ?? "") : "";

  const report = useCallback(
    (value: unknown, error = false) =>
      setMessage({ text: String(value), error }),
    [],
  );
  const refresh = useCallback(async () => {
    if (pollBusy.current) return;
    pollBusy.current = true;
    try {
      const [lan, list] = await Promise.all([
        api.lanStatus(),
        api.listDevices(),
      ]);
      setStatus(lan);
      setDevices(list.filter((item) => item.protocolVersion === 2));
      setConnectionError("");
    } catch (error) {
      setStatus(null);
      setDevices((current) =>
        current.map((item) => ({ ...item, connected: false })),
      );
      setConnectionError(
        typeof error === "string"
          ? error
          : "Desktop connection unavailable. Close and reopen LinkToLinux.",
      );
    } finally {
      pollBusy.current = false;
    }
  }, []);

  useEffect(() => {
    if (advanced) return;
    const poll = () => {
      if (!document.hidden) void refresh();
    };
    poll();
    const timer = window.setInterval(poll, active ? 1500 : 5000);
    document.addEventListener("visibilitychange", poll);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", poll);
    };
  }, [refresh, advanced, active]);
  useEffect(() => {
    if (!message || message.error) return;
    const timer = window.setTimeout(() => setMessage(null), 6500);
    return () => window.clearTimeout(timer);
  }, [message]);
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<{ deviceId: string; text: string }>(
      "companion:clipboard",
      ({ payload }) => {
        if (
          payload.deviceId !== target.current ||
          typeof payload.text !== "string"
        )
          return;
        clipboardLast.current = payload.text;
        setText(payload.text);
      },
    )
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      })
      .catch((error: unknown) => {
        console.warn("Clipboard event bridge unavailable", error instanceof Error ? error.name : "Desktop bridge unavailable");
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);
  useEffect(() => {
    setPreviewError(false);
  }, [status?.previewUrl]);

  const loadPhotos = useCallback(
    async (id: string, offset = 0) => {
      if (!id || loadingPhotos.current === id) return;
      loadingPhotos.current = id;
      const generation = ++photoGeneration.current;
      const isCurrent = () =>
        photoGeneration.current === generation && target.current === id;
      setPhotoBusy(true);
      try {
        const page = parsePhotoPage(
          await api.companionRequest(id, "gallery_list", { offset }),
        );
        if (!isCurrent()) return;
        setPhotos((current) =>
          offset ? [...current, ...page.photos] : page.photos,
        );
        setHasMore(page.hasMore);
        let next = 0;
        const worker = async () => {
          while (next < page.photos.length && isCurrent()) {
            const photo = page.photos[next++];
            if (!photo) return;
            try {
              const thumbnail = parseThumbnail(
                await api.companionRequest(id, "gallery_thumbnail", {
                  photoId: photo.id,
                }),
              );
              if (isCurrent())
                setPhotos((current) =>
                  current.map((item) =>
                    item.id === photo.id ? { ...item, thumbnail } : item,
                  ),
                );
            } catch (error) {
              if (isCurrent())
                setPhotos((current) =>
                  current.map((item) =>
                    item.id === photo.id ? { ...item, failed: true } : item,
                  ),
                );
            }
          }
        };
        await Promise.all([worker(), worker(), worker()]);
      } catch (error) {
        if (isCurrent()) report(error, true);
      } finally {
        if (photoGeneration.current === generation) {
          loadingPhotos.current = null;
          setPhotoBusy(false);
        }
      }
    },
    [report],
  );
  const currentId = connected ? (device?.id ?? "") : "";
  useEffect(() => {
    photoGeneration.current++;
    loadingPhotos.current = null;
    setPhotoBusy(false);
    setPhotos([]);
    setHasMore(false);
    setText("");
    clipboardLast.current = null;
    setClipboardSyncError("");
    if (currentId) void loadPhotos(currentId);
  }, [currentId, loadPhotos]);

  useEffect(() => {
    let disposed = false;
    let failures = 0;
    let timer: number | undefined;
    const restore = async () => {
      try {
        const enabled = parseClipboardSyncPreference(await api.clipboardSync());
        if (!disposed) {
          setAutoSync(enabled);
          setClipboardPreferenceError("");
        }
      } catch (error) {
        if (!disposed) {
          failures = Math.min(failures + 1, 5);
          setClipboardPreferenceError(
            `Could not load clipboard sync preference. Retrying. ${String(error)}`,
          );
          timer = window.setTimeout(() => void restore(), clipboardRetryDelay(failures));
        }
      }
    };
    void restore();
    return () => {
      disposed = true;
      window.clearTimeout(timer);
    };
  }, []);

  async function changeClipboardSync(enabled: boolean) {
    if (clipboardPreferenceBusy || autoSync === null) return;
    setClipboardPreferenceBusy(true);
    setClipboardPreferenceError("");
    try {
      setAutoSync(parseClipboardSyncPreference(await api.setClipboardSync(enabled)));
    } catch (error) {
      setClipboardPreferenceError(`Clipboard sync preference was not saved. Try again. ${String(error)}`);
    } finally {
      setClipboardPreferenceBusy(false);
    }
  }

  useEffect(() => {
    setClipboardSyncError("");
    if (!autoSync || !currentId) return;
    let disposed = false;
    let failures = 0;
    let timer: number | undefined;
    const sync = async () => {
      const snapshot = clipboardLast.current;
      try {
        const content = parseClipboard({ text: await api.readClipboard() });
        if (disposed || target.current !== currentId) return;
        if (canSendClipboard(content, snapshot, clipboardLast.current)) {
          await api.companionRequest(currentId, "clipboard_set", {
            text: content,
            automatic: true,
          });
          if (disposed || target.current !== currentId) return;
          if (snapshot === clipboardLast.current) {
            clipboardLast.current = content;
            setText(content);
          }
        }
        failures = 0;
        setClipboardSyncError("");
      } catch (error) {
        if (!disposed) {
          failures = Math.min(failures + 1, 5);
          setClipboardSyncError(
            `Clipboard sync is retrying. ${typeof error === "string" ? error : "Check the phone connection and clipboard access."}`,
          );
        }
      } finally {
        if (!disposed) timer = window.setTimeout(() => void sync(), clipboardRetryDelay(failures));
      }
    };
    void sync();
    return () => {
      disposed = true;
      window.clearTimeout(timer);
    };
  }, [autoSync, currentId]);

  async function capture() {
    if (!currentId || captureBusy) return;
    setCaptureBusy(true);
    try {
      await api.companionRequest(
        currentId,
        active ? "capture_stop" : "capture_start",
        active ? {} : { mode },
      );
      report(active ? "Phone capture stopped." : "Phone capture started.");
      await refresh();
    } catch (error) {
      report(error, true);
    } finally {
      setCaptureBusy(false);
    }
  }
  async function clipboardAction(action: "send" | "fetch" | "copy" | "linux") {
    setClipboardBusy(true);
    try {
      if (action === "send") {
        await api.companionRequest(currentId, "clipboard_set", { text });
        await api.writeClipboard(text);
        clipboardLast.current = text;
        report("Text copied to the phone clipboard.");
      } else if (action === "fetch") {
        const value = parseClipboard(
          await api.companionRequest(currentId, "clipboard_get"),
        );
        await api.writeClipboard(value);
        clipboardLast.current = value;
        setText(value);
        report("Phone text copied to the Linux clipboard.");
      } else if (action === "copy") {
        await api.writeClipboard(text);
        clipboardLast.current = text;
        report("Copied to Linux clipboard.");
      } else setText(await api.readClipboard());
    } catch (error) {
      report(error, true);
    } finally {
      setClipboardBusy(false);
    }
  }
  async function save(photo: CompanionPhoto) {
    setSaving(photo.id);
    try {
      report(
        `Saved original to ${await api.companionSavePhoto(currentId, photo.id)}`,
      );
    } catch (error) {
      report(error, true);
    } finally {
      setSaving(null);
    }
  }
  async function toggleServer() {
    try {
      if (status?.listening) {
        await api.lanStop();
        setStatus(null);
      } else setStatus(await api.lanStart());
      await refresh();
    } catch (error) {
      report(error, true);
    }
  }

  const qr = useMemo(
    () =>
      status?.downloadUrl
        ? generateQrSvgPath(generateQrMatrix(status.downloadUrl, "M"))
        : null,
    [status?.downloadUrl],
  );

  if (advanced)
    return (
      <>
        <Suspense
          fallback={
            <main className="companion-shell">
              <p>Opening ADB tools</p>
            </main>
          }
        >
          <AdvancedApp />
        </Suspense>
        <button className="companion-back" onClick={() => setAdvanced(false)}>
          Back to phone dashboard
        </button>
      </>
    );

  const show = (id: Section) => section === "overview" || section === id;
  return (
    <div className="companion-shell">
      <aside className="companion-sidebar">
        <a
          className="companion-brand"
          href="#overview"
          onClick={(event) => {
            event.preventDefault();
            setSection("overview");
          }}
        >
          <img src="/icon.svg" width="36" height="36" alt="" />
          LinkToLinux
        </a>
        <div className="companion-device">
          <div className="companion-phone-icon">
            <Smartphone size={30} />
          </div>
          <strong>{device?.name ?? "Your Android phone"}</strong>
          <span className={connected ? "linked" : ""}>
            {connected ? "Connected over Wi-Fi" : "Ready to connect"}
          </span>
          {connected && device && device.battery.charge >= 0 && (
            <small>
              <Battery size={14} />
              {device.battery.charge}%
              {device.battery.charging ? " (charging)" : ""}
            </small>
          )}
        </div>
        <nav aria-label="Phone features">
          {sections.map(({ id, label, icon: Icon }) => (
            <button
              key={id}
              className={section === id ? "selected" : ""}
              onClick={() => setSection(id)}
              aria-current={section === id ? "page" : undefined}
            >
              <Icon size={19} />
              {label}
            </button>
          ))}
        </nav>
        <div className="companion-sidebar-bottom">
          <button onClick={() => setAdvanced(true)}>
            <Usb size={17} />
            Advanced ADB tools
            <ChevronRight size={14} />
          </button>
        </div>
      </aside>
      <main className="companion-main">
        <header className="companion-header">
          <div>
            <span className="companion-breadcrumb">{device?.name ?? "Android companion"}</span>
            <h1>{sections.find((item) => item.id === section)?.label}</h1>
          </div>
          <div className="companion-header-actions">
            {devices.length > 1 && (
              <select
                aria-label="Select phone"
                value={device?.id ?? ""}
                onChange={(event) => setDeviceId(event.target.value)}
              >
                {devices.map((item) => (
                  <option key={item.id} value={item.id}>
                    {item.name}
                    {item.connected ? "" : " (offline)"}
                  </option>
                ))}
              </select>
            )}
            <span className={`companion-status ${connected ? "online" : ""}`}>
                {connected
                ? "Phone connected"
                : status?.listening
                  ? "Waiting for phone"
                  : "Not connected"}
            </span>
            <button
              className="companion-icon-button"
              aria-label="Refresh connection"
              onClick={() => void refresh()}
            >
              <RefreshCw size={18} />
            </button>
          </div>
        </header>
        {connectionError && (
          <div className="companion-alert" role="alert">
            {connectionError}
          </div>
        )}
        {message && (
          <div
            className={`companion-notice ${message.error ? "error" : ""}`}
            role={message.error ? "alert" : "status"}
          >
            <span>
              {message.error ? <X size={17} /> : <Check size={17} />}
              {message.text}
            </span>
            <button
              aria-label="Dismiss message"
              onClick={() => setMessage(null)}
            >
              <X size={16} />
            </button>
          </div>
        )}
        <div className="companion-content">
          {(!connected || section === "connection") && (
            <section className="companion-connect companion-panel">
              <div>
                <div className="companion-section-icon">
                  <Wifi size={22} />
                </div>
                <h2>
                  {connected
                    ? "Connection details"
                    : "Connect your Android phone"}
                </h2>
                <p>
                  Enter these details once in the Android companion and keep
                  Remember this desktop enabled. Reopen the companion to reconnect
                  on the same Wi-Fi.
                </p>
                <dl className="companion-credentials">
                  <div>
                    <dt>Desktop address</dt>
                    <dd>{status?.lanIp ?? "Unavailable"}</dd>
                  </div>
                  <div>
                    <dt>Port</dt>
                    <dd>{status?.port ?? "Unavailable"}</dd>
                  </div>
                  <div>
                    <dt>Pairing code</dt>
                    <dd className="pairing-code">
                      {status?.pairingCode || "Start server first"}
                    </dd>
                  </div>
                </dl>
                <p className="companion-fine-print">
                  Use trusted Wi-Fi. This local connection is authenticated with
                  the pairing code, but is not encrypted.
                </p>
                <div className="companion-connection-actions">
                  <button
                    className="companion-button secondary"
                    disabled={resetPairingBusy || Boolean(connectionError)}
                    onClick={() => void toggleServer()}
                  >
                    {status?.listening ? "Stop connection server" : "Start connection server"}
                  </button>
                  <button
                    className="companion-button secondary"
                    onClick={() => setResetPairingOpen(!resetPairingOpen)}
                    disabled={resetPairingBusy || Boolean(connectionError)}
                  >
                    Reset pairing
                  </button>
                </div>
                {resetPairingOpen && (
                  <div role="group" aria-label="Reset desktop pairing">
                    <p>
                      This disconnects every phone and replaces the saved pairing
                      code. Enter the new code on each phone to reconnect.
                    </p>
                    <button
                      className="companion-button secondary"
                      disabled={resetPairingBusy}
                      onClick={() => {
                        setResetPairingBusy(true);
                        void api.lanResetPairing().then(async (next) => {
                          setStatus(next);
                          setResetPairingOpen(false);
                          report("Pairing reset. Enter the new code on your phone.");
                          await refresh();
                        }).catch((error: unknown) => report(error, true))
                          .finally(() => setResetPairingBusy(false));
                      }}
                    >
                      {resetPairingBusy ? "Resetting pairing" : "Disconnect phones and reset code"}
                    </button>
                  </div>
                )}
              </div>
              <div className="companion-install">
                {qr ? (
                  <svg
                    role="img"
                    aria-label="Scan to download Android companion"
                    shapeRendering="crispEdges"
                    viewBox={`-4 -4 ${qr.size + 8} ${qr.size + 8}`}
                  >
                    <path d={qr.path} />
                  </svg>
                ) : (
                  <Smartphone size={64} />
                )}
                <strong>Get the Android companion</strong>
                <p>Scan with your phone camera to download the updated APK.</p>
                <code>
                  {status?.downloadUrl ?? "Start the desktop connection server"}
                </code>
              </div>
            </section>
          )}
          {section === "files" && <FilesPanel
            deviceId={currentId}
            deviceName={device?.name ?? "your phone"}
            connected={connected}
            supported={(device?.incomingCapabilities ?? []).includes("files-v1")}
          />}
          {section !== "files" && <div
            className={`companion-workspace ${section !== "overview" ? "single-section" : ""}`}
          >
            {show("photos") && (
              <section
                className="companion-panel companion-photos"
                aria-labelledby="photos-title"
              >
                <div className="companion-section-heading">
                  <div>
                    <h2 id="photos-title">Recent photos</h2>
                    <p>Browse your phone and save original photos.</p>
                  </div>
                  <button
                    className="companion-button secondary"
                    disabled={!connected || photoBusy}
                    onClick={() => void loadPhotos(currentId)}
                  >
                    <RefreshCw size={15} />
                    {photoBusy ? "Loading photos" : "Refresh photos"}
                  </button>
                </div>
                {photos.length ? (
                  <>
                    <div className="companion-photo-grid">
                      {photos
                        .slice(0, section === "overview" ? 12 : undefined)
                        .map((photo) => (
                          <figure key={photo.id}>
                            {photo.thumbnail ? (
                              <img src={photo.thumbnail} alt={photo.name} />
                            ) : (
                              <div className="companion-photo-loading">
                                <Images size={24} />
                                <span>
                                  {photo.failed
                                    ? "Preview unavailable"
                                    : "Loading preview"}
                                </span>
                              </div>
                            )}
                            <figcaption>
                              <span title={photo.name}>{photo.name}</span>
                              <button
                                className="companion-icon-button"
                                aria-label={`Save ${photo.name}`}
                                disabled={saving !== null}
                                onClick={() => void save(photo)}
                              >
                                {saving === photo.id ? (
                                  <RefreshCw
                                    size={16}
                                    className="companion-spin"
                                  />
                                ) : (
                                  <Download size={16} />
                                )}
                              </button>
                            </figcaption>
                          </figure>
                        ))}
                    </div>
                    {section === "overview" && photos.length > 12 ? (
                      <button
                        className="companion-button text"
                        onClick={() => setSection("photos")}
                      >
                        Browse all photos
                        <ChevronRight size={16} />
                      </button>
                    ) : (
                      hasMore && (
                        <button
                          className="companion-button secondary"
                          disabled={photoBusy}
                          onClick={() =>
                            void loadPhotos(currentId, photos.length)
                          }
                        >
                          Load more photos
                        </button>
                      )
                    )}
                  </>
                ) : (
                  <div className="companion-empty">
                    <Images size={38} strokeWidth={1.3} />
                    <h3>
                      {photoBusy
                        ? "Reading your camera roll"
                        : connected
                          ? "No photos available"
                          : "Your photos will appear here"}
                    </h3>
                    <p>
                      {connected
                        ? "Allow photo access in the companion, then refresh. Android can limit access to selected photos."
                        : "Connect your phone to browse photos and save originals to Linux."}
                    </p>
                  </div>
                )}
                <footer>
                  Original photos save to Downloads / LinkToLinux. Maximum 20
                  MiB per photo.
                </footer>
              </section>
            )}
            <div className="companion-tools">
              {show("capture") && (
                <section
                  className="companion-panel"
                  aria-labelledby="capture-title"
                >
                  <div className="companion-section-heading">
                    <div>
                      <h2 id="capture-title">Camera & microphone</h2>
                      <p>Use your phone in desktop apps.</p>
                    </div>
                    <Camera size={20} />
                  </div>
                  <div className="companion-preview">
                    {status?.previewUrl && !previewError ? (
                      <img
                        alt="Live phone camera"
                        src={status.previewUrl}
                        onError={() => setPreviewError(true)}
                      />
                    ) : (
                      <>
                        <div>
                          {active === "mic" ? (
                            <Mic size={38} strokeWidth={1.3} />
                          ) : (
                            <Camera size={38} strokeWidth={1.3} />
                          )}
                        </div>
                        <span>
                          {previewError
                            ? "Preview interrupted. Stop capture and retry."
                            : active === "mic"
                              ? "Phone microphone is active"
                              : active
                                ? "Waiting for camera frames"
                                : connected
                                  ? "Start capture to preview your phone camera"
                                  : "Connect your phone to use its camera"}
                        </span>
                      </>
                    )}
                  </div>
                  <label
                    className="companion-field-label"
                    htmlFor="capture-mode"
                  >
                    Share from phone
                  </label>
                  <select
                    id="capture-mode"
                    value={mode}
                    disabled={Boolean(active) || captureBusy}
                    onChange={(event) => setMode(event.target.value as Mode)}
                  >
                    <option value="studio">Camera and microphone</option>
                    <option value="webcam">Camera only</option>
                    <option value="mic">Microphone only</option>
                  </select>
                  <button
                    className={`companion-button ${active ? "stop" : "primary"}`}
                    disabled={!connected || captureBusy || otherCapture}
                    onClick={() => void capture()}
                  >
                    {active ? (
                      <Square size={16} />
                    ) : mode === "mic" ? (
                      <Mic size={17} />
                    ) : (
                      <Camera size={17} />
                    )}
                    {captureBusy
                      ? "Waiting for phone"
                      : active
                        ? "Stop capture"
                        : "Start capture"}
                  </button>
                  <p className="companion-helper">
                    Keep the companion visible when starting. Android will
                    request camera and microphone access. Switch lenses or rotate
                    the video using the controls on your phone.
                  </p>
                  {active && (
                    <div className="companion-routing">
                      {status?.audioSource && (
                        <p>
                          <Mic size={14} />
                          Choose <strong>{status.audioSource}</strong> in your
                          audio app.
                        </p>
                      )}
                      {status?.v4l2Device ? (
                        <p>
                          <Camera size={14} />
                          Virtual webcam: <strong>{status.v4l2Device}</strong>
                        </p>
                      ) : (
                        active !== "mic" && (
                          <p>
                            Preview works without a virtual webcam. Load
                            v4l2loopback to expose the camera to other apps.
                          </p>
                        )
                      )}
                    </div>
                  )}
                  {status?.lastError && (
                    <p className="companion-inline-error" role="alert">
                      {status.lastError}
                    </p>
                  )}
                </section>
              )}
              {show("clipboard") && (
                <section
                  className="companion-panel"
                  aria-labelledby="clipboard-title"
                >
                  <div className="companion-section-heading">
                    <div>
                      <h2 id="clipboard-title">Shared clipboard</h2>
                      <p>Share text between your phone and Linux.</p>
                    </div>
                    <Clipboard size={20} />
                  </div>
                  <label className="companion-sr-only" htmlFor="clipboard-text">
                    Clipboard text
                  </label>
                  <textarea
                    id="clipboard-text"
                    value={text}
                    onChange={(event) => setText(event.target.value)}
                    maxLength={1024 * 1024}
                    placeholder="Paste text to send to your phone"
                    rows={4}
                  />
                  <div className="companion-clipboard-actions">
                    <button
                      className="companion-button primary"
                      disabled={!connected || clipboardBusy}
                      onClick={() => void clipboardAction("send")}
                    >
                      <Copy size={15} />
                      Send to phone
                    </button>
                    <button
                      className="companion-button secondary"
                      disabled={!connected || clipboardBusy}
                      onClick={() => void clipboardAction("fetch")}
                    >
                      Get from phone
                    </button>
                  </div>
                  <div className="companion-clipboard-actions">
                    <button
                      className="companion-button text"
                      disabled={clipboardBusy || Boolean(connectionError)}
                      onClick={() => void clipboardAction("linux")}
                    >
                      Use Linux clipboard
                    </button>
                    <button
                      className="companion-button text"
                      disabled={clipboardBusy || Boolean(connectionError)}
                      onClick={() => void clipboardAction("copy")}
                    >
                      Copy to Linux
                    </button>
                  </div>
                  <label className="companion-checkbox">
                    <input
                      type="checkbox"
                      checked={autoSync ?? false}
                      disabled={autoSync === null || clipboardPreferenceBusy}
                      onChange={(event) => void changeClipboardSync(event.target.checked)}
                    />
                    {autoSync === null ? "Loading clipboard preference" : clipboardPreferenceBusy ? "Saving clipboard preference" : "Automatic clipboard sync"}
                  </label>
                  {clipboardPreferenceError && (
                    <p className="companion-inline-error" role="alert">
                      {clipboardPreferenceError}
                    </p>
                  )}
                  {clipboardSyncError && (
                    <p className="companion-inline-error" role="status">
                      {clipboardSyncError}
                    </p>
                  )}
                  <p className="companion-helper">
                    Linux text syncs while connected, including when the companion
                    is in the background. To share phone text, keep the companion
                    visible or tap Send clipboard in its notification.
                  </p>
                </section>
              )}
            </div>
          </div>}
          {section === "connection" && (
            <section className="companion-panel companion-connection-help">
              <Settings2 size={24} />
              <div>
                <h2>Connection troubleshooting</h2>
                <p>
                  To use the phone camera in Zoom, OBS or a browser, enable the
                  virtual webcam driver. Linux will ask for administrator
                  authorization.
                </p>
                <button
                  className="companion-button secondary"
                  disabled={webcamBusy || Boolean(connectionError)}
                  onClick={() => {
                    setWebcamBusy(true);
                    void api
                      .enableWebcam()
                      .then((result) => report(result))
                      .catch((error: unknown) => report(error, true))
                      .finally(() => setWebcamBusy(false));
                  }}
                >
                  {webcamBusy
                    ? "Waiting for administrator authorization"
                    : "Enable virtual webcam"}
                </button>
                <p>
                  Allow the displayed TCP port through the Linux firewall. Guest
                  Wi-Fi may isolate devices. If a VPN selects the wrong address,
                  set LINKTOLINUX_LAN_IP in the launch environment.
                </p>
                <p>
                  Screen mirroring and USB workflows are available in Advanced
                  ADB tools.
                </p>
              </div>
              <Monitor size={26} />
            </section>
          )}
        </div>
      </main>
    </div>
  );
}
