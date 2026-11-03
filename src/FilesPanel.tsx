import {useCallback, useEffect, useRef, useState} from "react";
import {getCurrentWebviewWindow} from "@tauri-apps/api/webviewWindow";
import {File, FolderOpen, Upload, Download, X, Check, RefreshCw} from "lucide-react";
import {api} from "./api";
import {activeTransfer, filePollDelay, withFileDeadline, formatTransferBytes, parseTransfers, transferLabel, transferProgress, validateFileSelection} from "./file-transfer-policy";
import type {FileTransferView} from "./file-transfer-policy";
import "./file-transfers.css";

interface Props {
  deviceId: string;
  deviceName: string;
  connected: boolean;
  supported: boolean;
}
function errorText(value: unknown): string {
  return typeof value === "string" ? value : value instanceof Error ? value.message : "File transfer is unavailable. Reconnect and try again.";
}
export default function FilesPanel({deviceId, deviceName, connected, supported}: Props) {
  const [transfers, setTransfers] = useState<FileTransferView[]>([]);
  const [error, setError] = useState("");
  const [loadError, setLoadError] = useState("");
  const [choosing, setChoosing] = useState(false);
  const [sending, setSending] = useState(false);
  const [cancelling, setCancelling] = useState<string | null>(null);
  const [dragging, setDragging] = useState(false);
  const [nativeDrop, setNativeDrop] = useState(true);
  const busy = useRef(false);
  const polling = useRef(false);
  const failures = useRef(0);
  const mounted = useRef(true);
  const ready = connected && supported && Boolean(deviceId);
  const reload = useCallback(async () => {
    if (polling.current) return;
    polling.current = true;
    try {
      const items = parseTransfers(await withFileDeadline(api.filesList()));
      failures.current = 0;
      if (mounted.current) { setTransfers(items); setLoadError(""); }
    } catch (failure) {
      failures.current = Math.min(failures.current + 1, 4);
      if (mounted.current) setLoadError(errorText(failure));
    } finally { polling.current = false; }
  }, []);
  useEffect(() => {
    mounted.current = true;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const poll = async () => {
      await reload();
      if (!disposed) timer = setTimeout(() => void poll(), filePollDelay(failures.current));
    };
    void poll();
    return () => { disposed = true; mounted.current = false; clearTimeout(timer); };
  }, [reload]);
  const sendPaths = useCallback(async (paths: string[]) => {
    if (!ready) { setError(connected ? "Update the Android companion to transfer files." : "Connect your phone before sending files."); return; }
    if (busy.current) return;
    busy.current = true;
    setSending(true);
    setError("");
    try {
      const selected = validateFileSelection(paths);
      if (selected.length) {
        parseTransfers(await withFileDeadline(api.filesSend(deviceId, selected)));
        await reload();
      }
    } catch (failure) { if (mounted.current) setError(errorText(failure)); }
    finally { busy.current = false; if (mounted.current) setSending(false); }
  }, [ready, connected, deviceId, reload]);
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const stopListening = (stop: () => void) => {
      void Promise.resolve().then(stop).catch((failure: unknown) => {
        console.warn("File drop listener cleanup failed", failure instanceof Error ? failure.name : "Desktop bridge unavailable");
      });
    };
    try {
      void getCurrentWebviewWindow().onDragDropEvent(({payload}) => {
        if (disposed) return;
        if (payload.type === "drop") { setDragging(false); void sendPaths(payload.paths); }
        else setDragging(payload.type === "enter" || payload.type === "over");
      }).then((stop) => { if (disposed) stopListening(stop); else unlisten = stop; })
        .catch((failure: unknown) => {
          if (!disposed) setNativeDrop(false);
          console.warn("File drop listener unavailable", failure instanceof Error ? failure.name : "Desktop bridge unavailable");
        });
    } catch (failure) {
      if (!disposed) setNativeDrop(false);
      console.warn("File drop listener unavailable", failure instanceof Error ? failure.name : "Desktop bridge unavailable");
    }
    return () => { disposed = true; if (unlisten) stopListening(unlisten); };
  }, [sendPaths]);
  async function chooseFiles() {
    if (!ready || choosing) return;
    setChoosing(true); setError("");
    try {
      const paths = validateFileSelection(await withFileDeadline(api.filesChoose(), 315000));
      if (mounted.current && paths.length) await sendPaths(paths);
    } catch (failure) { if (mounted.current) setError(errorText(failure)); }
    finally { if (mounted.current) setChoosing(false); }
  }
  async function cancel(item: FileTransferView) {
    setCancelling(item.id); setError("");
    try { await withFileDeadline(api.filesCancel(item.id)); await reload(); }
    catch (failure) { if (mounted.current) setError(errorText(failure)); }
    finally { if (mounted.current) setCancelling(null); }
  }
  async function openFolder() {
    try { await withFileDeadline(api.filesOpenFolder()); }
    catch (failure) { if (mounted.current) setError(errorText(failure)); }
  }
  const ordered = transfers.filter((item) => item.deviceId === deviceId).sort((a, b) => Number(activeTransfer(b)) - Number(activeTransfer(a)) || b.startedAt - a.startedAt);
  return <section className="companion-panel files-panel" aria-labelledby="files-title">
    <div className="files-heading">
      <div><h2 id="files-title">File transfers</h2><p>Send files to your phone or receive them directly from Android.</p></div>
      <button className="companion-button secondary" onClick={() => void openFolder()}>
        <FolderOpen size={16}/> Open received files
      </button>
    </div>
    <div className={"files-dropzone" + (dragging && ready ? " dragging" : "")}>
      <Upload size={28} strokeWidth={1.5}/>
      <div>
        <strong>{ready ? "Send files to " + deviceName : connected ? "Update the Android companion" : "Connect your phone to send files"}</strong>
        <p>{ready ? (nativeDrop ? "Drop files anywhere on this page, or choose them below." : "Choose files to send from your desktop.") : connected ? "File sharing requires the current companion app on your phone." : "Pair once from Connection. File sharing uses the same Wi-Fi connection."}</p>
      </div>
      <button className="companion-button primary" disabled={!ready || choosing || sending} onClick={() => void chooseFiles()}>
        <Upload size={16}/>{choosing ? "Choosing files" : sending ? "Queuing files" : "Send files"}
      </button>
      <small>Up to 16 files at a time, 8 GiB per file. Zip folders before sending.</small>
    </div>
    {(error || loadError) && <p className="companion-inline-error" role="alert">{error || loadError}</p>}
    <div className="files-list-heading"><h3>Transfers this session</h3><button className="companion-button text" onClick={() => void reload()} aria-label="Refresh transfers"><RefreshCw size={15}/></button></div>
    {ordered.length === 0 ? <div className="files-empty"><File size={30} strokeWidth={1.5}/><strong>No transfers yet</strong><p>On your phone, choose Send files in LinkToLinux or share a file to LinkToLinux from another app.</p></div> :
      <ul className="files-list">{ordered.map((item) => {
        const progress = transferProgress(item);
        const active = activeTransfer(item);
        return <li key={item.id} className={"files-transfer " + item.status}>
          <div className="files-direction" aria-label={item.direction === "send" ? "Sending to phone" : "Receiving from phone"}>{item.status === "complete" ? <Check size={19}/> : item.direction === "send" ? <Upload size={19}/> : <Download size={19}/>}</div>
          <div className="files-transfer-detail">
            <strong title={item.name}>{item.name}</strong>
            <div className="files-transfer-meta"><span>{transferLabel(item)}</span><span>{formatTransferBytes(item.transferred)}{item.size === null ? " transferred" : " of " + formatTransferBytes(item.size)}</span></div>
            {active && <progress aria-label={"Transfer progress for " + item.name} value={progress ?? undefined} max={100}/>}
            {item.error && <p className="files-transfer-error">{item.error}</p>}
          </div>
          {active && <button className="companion-button text" disabled={cancelling !== null} onClick={() => void cancel(item)} aria-label={"Cancel transfer of " + item.name}><X size={17}/></button>}
        </li>;
      })}</ul>}
    <p className="files-destination">Received files are saved to Downloads/LinkToLinux. Existing files are kept, and incomplete transfers are cleaned up.</p>
  </section>;
}
