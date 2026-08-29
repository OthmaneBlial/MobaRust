import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import {
  Activity,
  ArrowDownToLine,
  ArrowUpFromLine,
  CheckCircle2,
  ChevronDown,
  CircleHelp,
  CircleX,
  Command,
  Copy,
  Download,
  ExternalLink,
  Folder,
  FolderPlus,
  LoaderCircle,
  LayoutDashboard,
  MoreHorizontal,
  Network,
  PanelLeftClose,
  Pencil,
  Plus,
  Radio,
  RefreshCw,
  Search,
  Server,
  Settings2,
  ShieldCheck,
  Star,
  Terminal as TerminalIcon,
  Trash2,
  Upload,
  X,
  type LucideIcon,
} from "lucide-react";

type View = "terminal" | "files" | "tunnels" | "diagnostics";

type AppSettings = {
  general: {
    theme: "dark" | "light" | "system";
    confirmMultilinePaste: boolean;
  };
  appearance: {
    fontSize: number;
  };
  terminal: {
    scrollbackLines: number;
    cursorBlink: boolean;
  };
  ssh: {
    reconnectEnabled: boolean;
    reconnectAttempts: number;
    connectTimeoutMs: number;
  };
  network: {
    diagnosticTimeoutMs: number;
    scanConcurrency: number;
  };
};

const defaultSettings: AppSettings = {
  general: { theme: "dark", confirmMultilinePaste: true },
  appearance: { fontSize: 13 },
  terminal: { scrollbackLines: 5000, cursorBlink: true },
  ssh: { reconnectEnabled: true, reconnectAttempts: 3, connectTimeoutMs: 12000 },
  network: { diagnosticTimeoutMs: 1500, scanConcurrency: 32 },
};

type TerminalOutputEvent = {
  terminalId: string;
  data: string;
};

type TerminalClosedEvent = {
  terminalId: string;
  reason?: string;
};

type TerminalStatus = "starting" | "connected" | "reconnecting" | "closed" | "error";

type SshSessionEvent = {
  terminalId: string;
  state: "reconnecting" | "connected" | "failed" | "disconnected";
  attempt: number;
  error?: string | null;
};

type TelnetSessionEvent = {
  terminalId: string;
  state: "connected" | "disconnected" | "failed";
  error?: string | null;
};

type SerialSessionEvent = {
  terminalId: string;
  state: "connected" | "disconnected" | "failed";
  error?: string | null;
};

type TerminalViewportProps = {
  instanceKey: number;
  remoteSessionId: string | null;
  remoteProtocol: "ssh" | "telnet" | "serial" | null;
  fontSize: number;
  scrollbackLines: number;
  cursorBlink: boolean;
  confirmMultilinePaste: boolean;
  onStatusChange: (status: TerminalStatus) => void;
};

const IS_TAURI = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

type SessionListItem = {
  id?: string;
  name: string;
  detail: string;
  type: string;
  folder: string;
  active: boolean;
  favorite: boolean;
  tags: string[];
};

type SavedSession = {
  id: string;
  name: string;
  protocol: string;
  hostname: string;
  port: number;
  username?: string | null;
  known_hosts_path?: string | null;
  pinned_fingerprint?: string | null;
  folder: string | null;
  jump_hosts: string[];
  tags: string[];
  favorite: boolean;
  startup_directory: string | null;
  startup_command: string | null;
  environment: Array<[string, string]>;
  notes: string | null;
  auth:
    | { kind: "none" }
    | { kind: "agent" }
    | { kind: "password"; credentialRef: string }
    | { kind: "privateKey"; keyRef: string; credentialRef?: string | null };
};

type OpenSshImportReport = {
  source: string;
  imported: SavedSession[];
  skippedHosts: string[];
  unsupportedDirectives: string[];
};

type SessionImportReport = {
  importedCount: number;
  skipped: string[];
};

type SshConnectRequest = {
  host: string;
  port: number;
  username: string;
  auth: { method: "agent" } | { method: "privateKey"; path: string; passphraseCredentialId?: string } | { method: "password"; credentialId: string };
  knownHostsPath?: string;
  pinnedFingerprint?: string;
  jumpHosts?: SshJumpHostRequest[];
  cols: number;
  rows: number;
};

type SshJumpHostRequest = {
  host: string;
  port: number;
  username: string;
  auth: { method: "agent" };
  knownHostsPath?: string;
  pinnedFingerprint?: string;
};

type SshConnectResponse = {
  terminalId: string;
  host: string;
};

type TelnetConnectRequest = {
  host: string;
  port: number;
  terminal: string;
  encoding: "utf-8" | "windows-1252";
  columns: number;
  rows: number;
};

type TelnetConnectResponse = {
  terminalId: string;
  host: string;
};

type SerialConnectRequest = {
  device: string;
  baudRate: number;
  dataBits: "five" | "six" | "seven" | "eight";
  stopBits: "one" | "two";
  parity: "none" | "odd" | "even";
  flowControl: "none" | "software" | "hardware";
  lineEnding: "none" | "cr-lf" | "cr" | "lf";
};

type SerialConnectResponse = {
  terminalId: string;
  device: string;
};

type RemoteEntry = {
  name: string;
  path: string;
  size: number;
  isDirectory: boolean;
  modifiedUnixSeconds?: number | null;
};

type TransferState = "queued" | "preparing" | "running" | "paused" | "cancelling" | "cancelled" | "completed" | "failed";

type SshTransferEvent = {
  transferId: string;
  terminalId: string;
  direction: "download" | "upload";
  source: string;
  destination: string;
  bytesTransferred: number;
  totalBytes?: number | null;
  state: TransferState;
  error?: string | null;
};

type TunnelState = "listening" | "running" | "stopping" | "stopped" | "failed";

type SshTunnelEvent = {
  tunnelId: string;
  terminalId: string;
  localHost: string;
  localPort: number;
  targetHost: string;
  targetPort: number;
  kind: "local" | "dynamic" | "remote";
  state: TunnelState;
  connections: number;
  bytesForwarded: number;
  error?: string | null;
};

type TcpCheckResult = {
  host: string;
  port: number;
  status: "open" | "closed" | "timed-out";
};

type NetworkScanEvent = {
  scanId: string;
  state: "running" | "completed" | "cancelled" | "failed";
  scanned: number;
  total: number;
  result?: TcpCheckResult | null;
  error?: string | null;
};

const previewSessions: SessionListItem[] = [
  { name: "Local workstation", detail: "zsh · localhost", type: "LOCAL", folder: "Local terminals", active: true, favorite: true, tags: ["local"] },
  { name: "Production bastion", detail: "ops@bastion.example", type: "SSH", folder: "Production", active: false, favorite: true, tags: ["production"] },
  { name: "Staging cluster", detail: "dev@staging.example", type: "SSH", folder: "Staging", active: false, favorite: false, tags: ["staging"] },
];

const quickActions: Array<{ label: string; hint: string; icon: LucideIcon }> = [
  { label: "New local terminal", hint: "⌘ N", icon: TerminalIcon },
  { label: "Quick connect", hint: "⌘ K", icon: Network },
  { label: "Settings", hint: "", icon: Settings2 },
  { label: "Command palette", hint: "⌘ ⇧ P", icon: Command },
];

function TerminalViewport({ instanceKey, remoteSessionId, remoteProtocol, fontSize, scrollbackLines, cursorBlink, confirmMultilinePaste, onStatusChange }: TerminalViewportProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const terminalIdRef = useRef<string | null>(null);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    let disposed = false;
    let unlistenOutput: UnlistenFn | undefined;
    let unlistenClosed: UnlistenFn | undefined;
    let unlistenState: UnlistenFn | undefined;
    const terminal = new Terminal({
      allowProposedApi: true,
      convertEol: false,
      cursorBlink,
      cursorStyle: "bar",
      fontFamily: '"JetBrains Mono", "SFMono-Regular", Consolas, monospace',
      fontSize,
      lineHeight: 1.35,
      scrollback: scrollbackLines,
      theme: {
        background: "#101514",
        foreground: "#dce8dc",
        cursor: "#e8b45c",
        cursorAccent: "#101514",
        selectionBackground: "#3b5148",
        black: "#101514",
        red: "#ee8d78",
        green: "#9bc48a",
        yellow: "#e8b45c",
        blue: "#86a9cc",
        magenta: "#c9a3c7",
        cyan: "#77c4bb",
        white: "#dce8dc",
        brightBlack: "#63746b",
        brightRed: "#f09f89",
        brightGreen: "#b9dc9d",
        brightYellow: "#f3ca78",
        brightBlue: "#a7c6e2",
        brightMagenta: "#e0bedf",
        brightCyan: "#99e0d5",
        brightWhite: "#f2f5eb",
      },
    });
    const fitAddon = new FitAddon();
    terminal.loadAddon(fitAddon);
    terminal.open(host);

    const fit = () => {
      if (host.clientWidth > 0 && host.clientHeight > 0) fitAddon.fit();
      const terminalId = terminalIdRef.current;
      if (IS_TAURI && terminalId && remoteProtocol !== "serial") {
        const resizeCommand = remoteProtocol === "ssh" ? "ssh_resize" : remoteProtocol === "telnet" ? "telnet_resize" : "terminal_resize";
        void invoke(resizeCommand, {
          terminalId,
          cols: terminal.cols,
          rows: terminal.rows,
        }).catch(() => undefined);
      }
    };

    const resizeObserver = new ResizeObserver(fit);
    resizeObserver.observe(host);
    requestAnimationFrame(fit);

    const sendTerminalInput = (data: string) => {
      const terminalId = terminalIdRef.current;
      if (!IS_TAURI || !terminalId) {
        terminal.write(data.replace(/\r/g, "\r\n"));
        return;
      }
      const command = remoteProtocol === "ssh" ? "ssh_write" : remoteProtocol === "telnet" ? "telnet_write" : remoteProtocol === "serial" ? "serial_write" : "terminal_write";
      void invoke(command, { terminalId, data }).catch(() => onStatusChange("error"));
    };

    const input = terminal.onData((data) => {
      sendTerminalInput(data);
    });

    const onPaste = (event: ClipboardEvent) => {
      const data = event.clipboardData?.getData("text/plain") ?? "";
      if (!confirmMultilinePaste || (!data.includes("\n") && !data.includes("\r"))) return;
      event.preventDefault();
      event.stopPropagation();
      const accepted = window.confirm("This paste contains multiple lines. Send it to the terminal? Nothing will be executed automatically by MobaRust.");
      if (accepted) sendTerminalInput(data);
    };
    host.addEventListener("paste", onPaste, true);

    const boot = async () => {
      onStatusChange("starting");
      if (!IS_TAURI) {
        terminal.writeln("MobaRust browser preview");
        terminal.writeln("The real PTY is enabled in the desktop runtime.");
        terminal.writeln("\x1b[38;5;179m$\x1b[0m preview --ready");
        onStatusChange("connected");
        return;
      }

      try {
        const outputEvent = remoteProtocol === "ssh" ? "ssh://output" : remoteProtocol === "telnet" ? "telnet://output" : remoteProtocol === "serial" ? "serial://output" : "terminal://output";
        const closedEvent = remoteProtocol === "ssh" ? "ssh://closed" : remoteProtocol === "telnet" ? "telnet://closed" : remoteProtocol === "serial" ? "serial://closed" : "terminal://closed";
        unlistenOutput = await listen<TerminalOutputEvent>(outputEvent, (event) => {
          if (event.payload.terminalId === terminalIdRef.current) terminal.write(event.payload.data);
        });
        unlistenClosed = await listen<TerminalClosedEvent>(closedEvent, (event) => {
          if (event.payload.terminalId === terminalIdRef.current) onStatusChange("closed");
        });
        if (remoteProtocol === "ssh") {
          unlistenState = await listen<SshSessionEvent>("ssh://state", (event) => {
            if (event.payload.terminalId !== terminalIdRef.current) return;
            if (event.payload.state === "connected") onStatusChange("connected");
            else if (event.payload.state === "reconnecting") onStatusChange("reconnecting");
            else if (event.payload.state === "failed") onStatusChange("error");
          });
        }
        if (remoteProtocol === "telnet") {
          unlistenState = await listen<TelnetSessionEvent>("telnet://state", (event) => {
            if (event.payload.terminalId !== terminalIdRef.current) return;
            if (event.payload.state === "connected") onStatusChange("connected");
            else if (event.payload.state === "failed") onStatusChange("error");
            else if (event.payload.state === "disconnected") onStatusChange("closed");
          });
        }
        if (remoteProtocol === "serial") {
          unlistenState = await listen<SerialSessionEvent>("serial://state", (event) => {
            if (event.payload.terminalId !== terminalIdRef.current) return;
            if (event.payload.state === "connected") onStatusChange("connected");
            else if (event.payload.state === "failed") onStatusChange("error");
            else if (event.payload.state === "disconnected") onStatusChange("closed");
          });
        }
        if (remoteSessionId) {
          terminalIdRef.current = remoteSessionId;
          const attachCommand = remoteProtocol === "ssh" ? "ssh_attach" : remoteProtocol === "telnet" ? "telnet_attach" : "serial_attach";
          const closeCommand = remoteProtocol === "ssh" ? "ssh_close" : remoteProtocol === "telnet" ? "telnet_close" : "serial_close";
          const pendingOutput = await invoke<string[]>(attachCommand, { terminalId: remoteSessionId });
          if (disposed) {
            void invoke(closeCommand, { terminalId: remoteSessionId });
            return;
          }
          pendingOutput.forEach((data) => terminal.write(data));
          onStatusChange("connected");
          fit();
          return;
        }
        const terminalId = await invoke<string>("terminal_spawn", {
          cols: terminal.cols,
          rows: terminal.rows,
        });
        if (disposed) {
          void invoke("terminal_close", { terminalId });
          return;
        }
        terminalIdRef.current = terminalId;
        onStatusChange("connected");
        fit();
      } catch {
        onStatusChange("error");
        terminal.writeln("\r\n\x1b[38;5;203mUnable to start the local PTY.\x1b[0m");
      }
    };
    void boot();

    return () => {
      disposed = true;
      input.dispose();
      host.removeEventListener("paste", onPaste, true);
      resizeObserver.disconnect();
      unlistenOutput?.();
      unlistenClosed?.();
      unlistenState?.();
      const terminalId = terminalIdRef.current;
      if (IS_TAURI && terminalId) {
        const closeCommand = remoteProtocol === "ssh" ? "ssh_close" : remoteProtocol === "telnet" ? "telnet_close" : remoteProtocol === "serial" ? "serial_close" : "terminal_close";
        void invoke(closeCommand, { terminalId });
      }
      terminalIdRef.current = null;
      terminal.dispose();
    };
  }, [confirmMultilinePaste, cursorBlink, fontSize, instanceKey, onStatusChange, remoteProtocol, remoteSessionId, scrollbackLines]);

  return <div className="terminal-host" ref={hostRef} aria-label="Local terminal" />;
}

function App() {
  const [activeView, setActiveView] = useState<View>("terminal");
  const [terminalKey, setTerminalKey] = useState(0);
  const [terminalOpen, setTerminalOpen] = useState(true);
  const [terminalStatus, setTerminalStatus] = useState<TerminalStatus>("starting");
  const [search, setSearch] = useState("");
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [quickConnectOpen, setQuickConnectOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settings, setSettings] = useState<AppSettings>(defaultSettings);
  const [connectionError, setConnectionError] = useState<string | null>(null);
  const [sessionNotice, setSessionNotice] = useState<string | null>(null);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [favoritesOnly, setFavoritesOnly] = useState(false);
  const [now, setNow] = useState(() => new Date());
  const [sessionRows, setSessionRows] = useState<SessionListItem[]>(IS_TAURI ? [] : previewSessions);
  const [savedSessions, setSavedSessions] = useState<SavedSession[]>([]);
  const [editingSession, setEditingSession] = useState<SavedSession | null>(null);
  const [remoteSessionId, setRemoteSessionId] = useState<string | null>(null);
  const [remoteProtocol, setRemoteProtocol] = useState<"ssh" | "telnet" | "serial" | null>(null);
  const [remoteHost, setRemoteHost] = useState<string | null>(null);
  const [remotePath, setRemotePath] = useState(".");
  const [remoteEntries, setRemoteEntries] = useState<RemoteEntry[]>([]);
  const [sftpStatus, setSftpStatus] = useState<"idle" | "loading" | "ready" | "error">("idle");
  const [transfers, setTransfers] = useState<SshTransferEvent[]>([]);
  const [tunnels, setTunnels] = useState<SshTunnelEvent[]>([]);
  const [networkHost, setNetworkHost] = useState("");
  const [networkPort, setNetworkPort] = useState("22");
  const [networkTimeout, setNetworkTimeout] = useState("1500");
  const [networkStatus, setNetworkStatus] = useState<"idle" | "running" | "ready" | "error">("idle");
  const [networkAddresses, setNetworkAddresses] = useState<string[]>([]);
  const [networkResult, setNetworkResult] = useState<TcpCheckResult | null>(null);
  const [networkError, setNetworkError] = useState<string | null>(null);
  const [networkScanId, setNetworkScanId] = useState<string | null>(null);
  const [networkScanStatus, setNetworkScanStatus] = useState<"idle" | "running" | "completed" | "cancelled" | "failed">("idle");
  const [networkScanStart, setNetworkScanStart] = useState("1");
  const [networkScanEnd, setNetworkScanEnd] = useState("1024");
  const [networkScanConcurrency, setNetworkScanConcurrency] = useState("32");
  const [networkScanScanned, setNetworkScanScanned] = useState(0);
  const [networkScanTotal, setNetworkScanTotal] = useState(0);
  const [networkScanResults, setNetworkScanResults] = useState<TcpCheckResult[]>([]);
  const networkScanIdRef = useRef<string | null>(null);

  const startNewTerminal = useCallback(() => {
    setRemoteSessionId(null);
    setRemoteProtocol(null);
    setRemoteHost(null);
    setConnectionError(null);
    setSessionNotice(null);
    setTerminalStatus("starting");
    setTerminalOpen(true);
    setTerminalKey((key) => key + 1);
    setActiveView("terminal");
  }, []);

  const handleTerminalStatus = useCallback((status: TerminalStatus) => {
    setTerminalStatus(status);
  }, []);

  const refreshSavedSessions = useCallback(() => {
    if (!IS_TAURI) return;
    void invoke<SavedSession[]>("session_list")
      .then((sessions) => {
        setSavedSessions(sessions);
        setSessionRows(sessions.map(toSessionListItem));
      })
      .catch(() => undefined);
  }, []);

  const refreshSettings = useCallback(() => {
    if (!IS_TAURI) return;
    void invoke<AppSettings>("settings_get")
      .then(setSettings)
      .catch((error) => setConnectionError(`Settings could not be loaded: ${String(error)}`));
  }, []);

  const saveSettings = useCallback(async (next: AppSettings) => {
    try {
      const saved = IS_TAURI ? await invoke<AppSettings>("settings_save", { settings: next }) : next;
      setSettings(saved);
      setSettingsOpen(false);
      setSessionNotice("Settings saved. New terminal instances use the updated terminal profile.");
      setConnectionError(null);
    } catch (error) {
      setConnectionError(`Settings could not be saved: ${String(error)}`);
    }
  }, []);

  const resetSettings = useCallback(async () => {
    if (!window.confirm("Reset MobaRust settings to their safe defaults?")) return;
    try {
      const reset = IS_TAURI ? await invoke<AppSettings>("settings_reset") : defaultSettings;
      setSettings(reset);
      setSettingsOpen(false);
      setSessionNotice("Settings reset to defaults.");
      setConnectionError(null);
    } catch (error) {
      setConnectionError(`Settings could not be reset: ${String(error)}`);
    }
  }, []);

  const connectSsh = useCallback(async (request: SshConnectRequest, offerSave = true) => {
    setConnectionError(null);
    setSessionNotice(null);
    if (!IS_TAURI) {
      setConnectionError("SSH connections require the desktop runtime.");
      return;
    }
    try {
      const response = await invoke<SshConnectResponse>("ssh_connect", { request });
      setRemoteSessionId(response.terminalId);
      setRemoteProtocol("ssh");
      setRemoteHost(response.host);
      setTerminalOpen(true);
      setTerminalStatus("starting");
      setTerminalKey((key) => key + 1);
      setActiveView("terminal");
      setQuickConnectOpen(false);
      if (offerSave) {
        const suggestedName = `${request.username}@${response.host}`;
        const name = window.prompt("Save this SSH session as", suggestedName);
        if (name?.trim()) {
          try {
            await invoke("session_save_ssh", { payload: { name: name.trim(), request } });
            refreshSavedSessions();
          } catch (error) {
            setConnectionError(`Connected, but the session could not be saved: ${String(error)}`);
          }
        }
      }
    } catch (error) {
      setConnectionError(String(error));
    }
  }, [refreshSavedSessions]);

  const connectTelnet = useCallback(async (request: TelnetConnectRequest) => {
    setConnectionError(null);
    setSessionNotice(null);
    if (!IS_TAURI) {
      setConnectionError("Telnet connections require the desktop runtime.");
      return;
    }
    try {
      const response = await invoke<TelnetConnectResponse>("telnet_connect", { request });
      setRemoteSessionId(response.terminalId);
      setRemoteProtocol("telnet");
      setRemoteHost(response.host);
      setTerminalOpen(true);
      setTerminalStatus("starting");
      setTerminalKey((key) => key + 1);
      setActiveView("terminal");
      setQuickConnectOpen(false);
      setSessionNotice("Connected over Telnet. This connection is unencrypted.");
    } catch (error) {
      setConnectionError(String(error));
    }
  }, []);

  const connectSerial = useCallback(async (request: SerialConnectRequest) => {
    setConnectionError(null);
    setSessionNotice(null);
    if (!IS_TAURI) {
      setConnectionError("Serial connections require the desktop runtime.");
      return;
    }
    try {
      const response = await invoke<SerialConnectResponse>("serial_connect", { request });
      setRemoteSessionId(response.terminalId);
      setRemoteProtocol("serial");
      setRemoteHost(response.device);
      setTerminalOpen(true);
      setTerminalStatus("starting");
      setTerminalKey((key) => key + 1);
      setActiveView("terminal");
      setQuickConnectOpen(false);
      setSessionNotice(`Connected to ${response.device}. Serial traffic is not encrypted by MobaRust.`);
    } catch (error) {
      setConnectionError(String(error));
    }
  }, []);

  const importOpenSshConfig = useCallback(async () => {
    if (!IS_TAURI) return;
    const requestedPath = window.prompt("OpenSSH config path", "~/.ssh/config");
    if (requestedPath === null) return;
    try {
      const report = await invoke<OpenSshImportReport>("session_import_openssh", {
        payload: { path: requestedPath.trim() || "~/.ssh/config" },
      });
      refreshSavedSessions();
      const warnings = [
        report.skippedHosts.length > 0 ? `${report.skippedHosts.length} skipped` : "",
        report.unsupportedDirectives.length > 0 ? `${report.unsupportedDirectives.length} unsupported directive${report.unsupportedDirectives.length === 1 ? "" : "s"}` : "",
      ].filter(Boolean);
      setSessionNotice(`Imported ${report.imported.length} OpenSSH profile${report.imported.length === 1 ? "" : "s"}${warnings.length > 0 ? ` · ${warnings.join(" · ")}` : ""}.`);
    } catch (error) {
      setSessionNotice(null);
      setConnectionError(String(error));
    }
  }, [refreshSavedSessions]);

  const exportSessions = useCallback(async () => {
    if (!IS_TAURI) return;
    try {
      const json = await invoke<string>("session_export");
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(json);
        setSessionNotice("Secret-free session definitions copied to the clipboard; credential material is not included.");
      } else {
        window.prompt("Copy this secret-free MobaRust session export", json);
      }
    } catch (error) {
      setConnectionError(`Session export failed: ${String(error)}`);
    }
  }, []);

  const importSessions = useCallback(async () => {
    if (!IS_TAURI) return;
    const json = window.prompt("Paste a secret-free MobaRust session export JSON");
    if (!json?.trim()) return;
    try {
      const report = await invoke<SessionImportReport>("session_import", { payload: { json } });
      refreshSavedSessions();
      setSessionNotice(`Imported ${report.importedCount} session${report.importedCount === 1 ? "" : "s"}${report.skipped.length > 0 ? ` · ${report.skipped.length} skipped` : ""}.`);
    } catch (error) {
      setConnectionError(`Session import failed: ${String(error)}`);
    }
  }, [refreshSavedSessions]);

  const toggleFavorite = useCallback(async (session: SessionListItem) => {
    if (!IS_TAURI || !session.id) return;
    try {
      await invoke("session_set_favorite", { sessionId: session.id, favorite: !session.favorite });
      refreshSavedSessions();
    } catch (error) {
      setConnectionError(`Favorite update failed: ${String(error)}`);
    }
  }, [refreshSavedSessions]);

  const saveEditedSession = useCallback(async (session: SavedSession) => {
    if (!IS_TAURI) return;
    try {
      await invoke<SavedSession>("session_save", { session });
      setEditingSession(null);
      refreshSavedSessions();
      setSessionNotice(`Saved ${session.name}. Secret references were left unchanged.`);
    } catch (error) {
      setConnectionError(`Session update failed: ${String(error)}`);
    }
  }, [refreshSavedSessions]);

  const deleteSavedSession = useCallback(async (session: SavedSession) => {
    if (!IS_TAURI || !window.confirm(`Delete saved session “${session.name}”?`)) return;
    try {
      await invoke<boolean>("session_delete", { sessionId: session.id });
      if (editingSession?.id === session.id) setEditingSession(null);
      refreshSavedSessions();
      setSessionNotice(`Deleted ${session.name}.`);
    } catch (error) {
      setConnectionError(`Session deletion failed: ${String(error)}`);
    }
  }, [editingSession?.id, refreshSavedSessions]);

  const connectSavedSession = useCallback((session: SavedSession) => {
    if (session.jump_hosts.length > 0) {
      setConnectionError("This profile uses ProxyJump. Jump-host connections are imported but not implemented yet.");
      return;
    }
    const request = requestFromSavedSession(session);
    if (!request) {
      setConnectionError("This saved session uses an authentication method that is not available yet.");
      return;
    }
    void connectSsh(request, false);
  }, [connectSsh]);

  const loadRemoteDirectory = useCallback(async (path: string) => {
    if (!remoteSessionId) return;
    setSftpStatus("loading");
    try {
      const entries = await invoke<RemoteEntry[]>("ssh_list_directory", {
        terminalId: remoteSessionId,
        path,
      });
      setRemoteEntries(entries);
      setRemotePath(path);
      setSftpStatus("ready");
    } catch (error) {
      setSftpStatus("error");
      setConnectionError(String(error));
    }
  }, [remoteSessionId]);

  const navigateRemote = useCallback((path: string) => {
    setRemotePath(path);
    void loadRemoteDirectory(path);
  }, [loadRemoteDirectory]);

  const startDownload = useCallback(async (entry: RemoteEntry) => {
    if (!remoteSessionId) return;
    const localPath = window.prompt(entry.isDirectory ? "Local destination directory" : "Local destination path", entry.name);
    if (!localPath?.trim()) return;
    const overwrite = window.confirm(entry.isDirectory ? "Allow replacing existing files inside this directory?" : "Allow replacing an existing local file?");
    try {
      await invoke("ssh_download", {
        terminalId: remoteSessionId,
        request: { remotePath: entry.path, localPath: localPath.trim(), overwrite, recursive: entry.isDirectory },
      });
      setConnectionError(null);
    } catch (error) {
      setConnectionError(String(error));
    }
  }, [remoteSessionId]);

  const startUpload = useCallback(async () => {
    if (!remoteSessionId) return;
    const localPath = window.prompt("Local file or directory to upload", "");
    if (!localPath?.trim()) return;
    const fallbackName = localPath.trim().split(/[\\/]/).pop() || "upload.bin";
    const defaultRemotePath = remotePath === "." ? `./${fallbackName}` : `${remotePath.replace(/\/$/, "")}/${fallbackName}`;
    const destination = window.prompt("Remote destination path", defaultRemotePath);
    if (!destination?.trim()) return;
    const overwrite = window.confirm("Allow replacing an existing remote file?");
    try {
      await invoke("ssh_upload", {
        terminalId: remoteSessionId,
        request: { remotePath: destination.trim(), localPath: localPath.trim(), overwrite, recursive: true },
      });
      setConnectionError(null);
    } catch (error) {
      setConnectionError(String(error));
    }
  }, [remotePath, remoteSessionId]);

  const createRemoteDirectory = useCallback(async () => {
    if (!remoteSessionId) return;
    const defaultPath = remotePath === "." ? "./new-folder" : `${remotePath.replace(/\/$/, "")}/new-folder`;
    const path = window.prompt("Remote folder path", defaultPath);
    if (!path?.trim()) return;
    try {
      await invoke("ssh_create_remote_directory", { terminalId: remoteSessionId, path: path.trim() });
      setConnectionError(null);
      await loadRemoteDirectory(remotePath);
    } catch (error) {
      setConnectionError(String(error));
    }
  }, [loadRemoteDirectory, remotePath, remoteSessionId]);

  const renameRemote = useCallback(async (entry: RemoteEntry) => {
    if (!remoteSessionId) return;
    const nextName = window.prompt("New remote name or path", entry.name);
    if (!nextName?.trim()) return;
    const parent = entry.path.split("/").slice(0, -1).join("/") || ".";
    const target = nextName.trim().includes("/") ? nextName.trim() : `${parent}/${nextName.trim()}`;
    try {
      await invoke("ssh_rename_remote", { terminalId: remoteSessionId, from: entry.path, to: target });
      setConnectionError(null);
      await loadRemoteDirectory(remotePath);
    } catch (error) {
      setConnectionError(String(error));
    }
  }, [loadRemoteDirectory, remotePath, remoteSessionId]);

  const deleteRemote = useCallback(async (entry: RemoteEntry) => {
    if (!remoteSessionId || !window.confirm(`Delete remote ${entry.isDirectory ? "directory" : "file"} “${entry.name}”?`)) return;
    try {
      await invoke("ssh_delete_remote", { terminalId: remoteSessionId, path: entry.path });
      setConnectionError(null);
      await loadRemoteDirectory(remotePath);
    } catch (error) {
      setConnectionError(String(error));
    }
  }, [loadRemoteDirectory, remotePath, remoteSessionId]);

  const cancelTransfer = useCallback(async (transferId: string) => {
    try {
      await invoke("ssh_cancel_transfer", { transferId });
    } catch (error) {
      setConnectionError(String(error));
    }
  }, []);

  const startLocalForward = useCallback(async () => {
    if (!remoteSessionId) return;
    const targetHost = window.prompt("Remote target host", "127.0.0.1");
    if (!targetHost?.trim()) return;
    const targetPortValue = window.prompt("Remote target port", "5432");
    const targetPort = Number(targetPortValue);
    if (!Number.isInteger(targetPort) || targetPort < 1 || targetPort > 65535) {
      setConnectionError("Target port must be an integer between 1 and 65535.");
      return;
    }
    const bindHost = window.prompt("Local bind host", "127.0.0.1");
    if (!bindHost?.trim()) return;
    const bindPortValue = window.prompt("Local bind port (0 chooses a free port)", "0");
    const bindPort = Number(bindPortValue);
    if (!Number.isInteger(bindPort) || bindPort < 0 || bindPort > 65535) {
      setConnectionError("Local bind port must be an integer between 0 and 65535.");
      return;
    }
    try {
      await invoke("ssh_start_local_forward", {
        terminalId: remoteSessionId,
        request: { bindHost: bindHost.trim(), bindPort, targetHost: targetHost.trim(), targetPort },
      });
      setConnectionError(null);
      setActiveView("tunnels");
    } catch (error) {
      setConnectionError(String(error));
    }
  }, [remoteSessionId]);

  const startRemoteForward = useCallback(async () => {
    if (!remoteSessionId) return;
    const bindHost = window.prompt("Remote bind host (on the SSH server)", "127.0.0.1");
    if (!bindHost?.trim()) return;
    const bindPortValue = window.prompt("Remote bind port (0 chooses a free port)", "0");
    const bindPort = Number(bindPortValue);
    if (!Number.isInteger(bindPort) || bindPort < 0 || bindPort > 65535) {
      setConnectionError("Remote bind port must be an integer between 0 and 65535.");
      return;
    }
    const targetHost = window.prompt("Local target host (from this Mac)", "127.0.0.1");
    if (!targetHost?.trim()) return;
    const targetPortValue = window.prompt("Local target port", "3000");
    const targetPort = Number(targetPortValue);
    if (!Number.isInteger(targetPort) || targetPort < 1 || targetPort > 65535) {
      setConnectionError("Local target port must be an integer between 1 and 65535.");
      return;
    }
    try {
      await invoke("ssh_start_remote_forward", {
        terminalId: remoteSessionId,
        request: { bindHost: bindHost.trim(), bindPort, targetHost: targetHost.trim(), targetPort },
      });
      setConnectionError(null);
      setActiveView("tunnels");
    } catch (error) {
      setConnectionError(String(error));
    }
  }, [remoteSessionId]);

  const startDynamicForward = useCallback(async () => {
    if (!remoteSessionId) return;
    const bindHost = window.prompt("SOCKS5 bind host", "127.0.0.1");
    if (!bindHost?.trim()) return;
    const bindPortValue = window.prompt("SOCKS5 local bind port (0 chooses a free port)", "0");
    const bindPort = Number(bindPortValue);
    if (!Number.isInteger(bindPort) || bindPort < 0 || bindPort > 65535) {
      setConnectionError("SOCKS bind port must be an integer between 0 and 65535.");
      return;
    }
    try {
      await invoke("ssh_start_dynamic_forward", {
        terminalId: remoteSessionId,
        request: { bindHost: bindHost.trim(), bindPort },
      });
      setConnectionError(null);
      setActiveView("tunnels");
    } catch (error) {
      setConnectionError(String(error));
    }
  }, [remoteSessionId]);

  const cancelTunnel = useCallback(async (tunnelId: string) => {
    try {
      await invoke("ssh_cancel_tunnel", { tunnelId });
    } catch (error) {
      setConnectionError(String(error));
    }
  }, []);

  const resolveNetworkHost = useCallback(async () => {
    const host = networkHost.trim();
    const timeoutMs = Number(networkTimeout);
    if (!host) {
      setNetworkError("Enter an explicit hostname or IP address.");
      setNetworkStatus("error");
      return;
    }
    if (!Number.isInteger(timeoutMs) || timeoutMs < 50 || timeoutMs > 60_000) {
      setNetworkError("Timeout must be an integer between 50 and 60000 milliseconds.");
      setNetworkStatus("error");
      return;
    }
    if (!IS_TAURI) {
      setNetworkError("Network diagnostics require the desktop runtime.");
      setNetworkStatus("error");
      return;
    }
    setNetworkStatus("running");
    setNetworkError(null);
    try {
      const addresses = await invoke<string[]>("network_resolve_host", { request: { host, timeoutMs } });
      setNetworkAddresses(addresses);
      setNetworkStatus("ready");
    } catch (error) {
      setNetworkAddresses([]);
      setNetworkError(String(error));
      setNetworkStatus("error");
    }
  }, [networkHost, networkTimeout]);

  const checkNetworkTcp = useCallback(async () => {
    const host = networkHost.trim();
    const port = Number(networkPort);
    const timeoutMs = Number(networkTimeout);
    if (!host) {
      setNetworkError("Enter an explicit hostname or IP address.");
      setNetworkStatus("error");
      return;
    }
    if (!Number.isInteger(port) || port < 1 || port > 65_535) {
      setNetworkError("Port must be an integer between 1 and 65535.");
      setNetworkStatus("error");
      return;
    }
    if (!Number.isInteger(timeoutMs) || timeoutMs < 50 || timeoutMs > 60_000) {
      setNetworkError("Timeout must be an integer between 50 and 60000 milliseconds.");
      setNetworkStatus("error");
      return;
    }
    if (!IS_TAURI) {
      setNetworkError("Network diagnostics require the desktop runtime.");
      setNetworkStatus("error");
      return;
    }
    setNetworkStatus("running");
    setNetworkError(null);
    try {
      const result = await invoke<TcpCheckResult>("network_check_tcp", { request: { host, port, timeoutMs } });
      setNetworkResult(result);
      setNetworkStatus("ready");
    } catch (error) {
      setNetworkResult(null);
      setNetworkError(String(error));
      setNetworkStatus("error");
    }
  }, [networkHost, networkPort, networkTimeout]);

  const startNetworkScan = useCallback(async () => {
    const host = networkHost.trim();
    const startPort = Number(networkScanStart);
    const endPort = Number(networkScanEnd);
    const concurrency = Number(networkScanConcurrency);
    const timeoutMs = Number(networkTimeout);
    if (!host) {
      setNetworkError("Enter an explicit hostname or IP address.");
      setNetworkStatus("error");
      return;
    }
    if (!Number.isInteger(startPort) || !Number.isInteger(endPort) || startPort < 1 || endPort < startPort || endPort > 65_535 || endPort - startPort + 1 > 4_096) {
      setNetworkError("Port range must be explicit, valid, and no larger than 4096 ports.");
      setNetworkStatus("error");
      return;
    }
    if (!Number.isInteger(concurrency) || concurrency < 1 || concurrency > 128) {
      setNetworkError("Concurrency must be an integer between 1 and 128.");
      setNetworkStatus("error");
      return;
    }
    if (!Number.isInteger(timeoutMs) || timeoutMs < 50 || timeoutMs > 60_000) {
      setNetworkError("Timeout must be an integer between 50 and 60000 milliseconds.");
      setNetworkStatus("error");
      return;
    }
    if (!IS_TAURI) {
      setNetworkError("Network diagnostics require the desktop runtime.");
      setNetworkStatus("error");
      return;
    }
    networkScanIdRef.current = null;
    setNetworkScanId(null);
    setNetworkScanResults([]);
    setNetworkScanScanned(0);
    setNetworkScanTotal(endPort - startPort + 1);
    setNetworkScanStatus("running");
    setNetworkError(null);
    try {
      const response = await invoke<{ scanId: string }>("network_scan_start", {
        request: { host, startPort, endPort, concurrency, timeoutMs },
      });
      if (!networkScanIdRef.current) {
        networkScanIdRef.current = response.scanId;
        setNetworkScanId(response.scanId);
      }
    } catch (error) {
      networkScanIdRef.current = null;
      setNetworkScanId(null);
      setNetworkScanStatus("failed");
      setNetworkError(String(error));
    }
  }, [networkHost, networkScanConcurrency, networkScanEnd, networkScanStart, networkTimeout]);

  const cancelNetworkScan = useCallback(async () => {
    const scanId = networkScanIdRef.current ?? networkScanId;
    if (!scanId || !IS_TAURI) return;
    try {
      await invoke("network_scan_cancel", { scanId });
    } catch (error) {
      setNetworkError(`Scan cancellation failed: ${String(error)}`);
    }
  }, [networkScanId]);

  useEffect(() => {
    const timer = window.setInterval(() => setNow(new Date()), 30_000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    if (!IS_TAURI) return;
    let disposed = false;
    let unlisten: UnlistenFn | undefined;
    void listen<SshTunnelEvent>("ssh://tunnel", (event) => {
      setTunnels((current) => {
        const next = current.filter((tunnel) => tunnel.tunnelId !== event.payload.tunnelId);
        return [...next, event.payload].slice(-20);
      });
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    if (!IS_TAURI) return;
    let disposed = false;
    let unlisten: UnlistenFn | undefined;
    void listen<NetworkScanEvent>("network://scan", (event) => {
      const payload = event.payload;
      if (networkScanIdRef.current && payload.scanId !== networkScanIdRef.current) return;
      if (!networkScanIdRef.current) {
        networkScanIdRef.current = payload.scanId;
        setNetworkScanId(payload.scanId);
      }
      setNetworkScanScanned(payload.scanned);
      setNetworkScanTotal(payload.total);
      if (payload.result) {
        setNetworkScanResults((current) => current.some((result) => result.port === payload.result?.port) ? current : [...current, payload.result!].sort((a, b) => a.port - b.port));
      }
      if (payload.state === "running") setNetworkScanStatus("running");
      if (payload.state === "completed") setNetworkScanStatus("completed");
      if (payload.state === "cancelled") setNetworkScanStatus("cancelled");
      if (payload.state === "failed") {
        setNetworkScanStatus("failed");
        setNetworkError(payload.error ?? "Network scan failed.");
      }
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    refreshSavedSessions();
  }, [refreshSavedSessions]);

  useEffect(() => {
    refreshSettings();
  }, [refreshSettings]);

  useEffect(() => {
    if (activeView === "files" && remoteSessionId && remoteProtocol === "ssh") void loadRemoteDirectory(".");
  }, [activeView, loadRemoteDirectory, remoteProtocol, remoteSessionId]);

  useEffect(() => {
    if (!IS_TAURI) return;
    let disposed = false;
    let unlisten: UnlistenFn | undefined;
    void listen<SshTransferEvent>("sftp://transfer", (event) => {
      setTransfers((current) => {
        const next = current.filter((transfer) => transfer.transferId !== event.payload.transferId);
        return [...next, event.payload].slice(-40);
      });
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const command = event.metaKey || event.ctrlKey;
      if (command && event.key.toLowerCase() === "n") {
        event.preventDefault();
        startNewTerminal();
      }
      if (command && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setQuickConnectOpen(true);
      }
      if (command && event.shiftKey && event.key.toLowerCase() === "p") {
        event.preventDefault();
        setPaletteOpen((open) => !open);
      }
      if (event.key === "Escape") setPaletteOpen(false);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [startNewTerminal]);

  const filteredSessions = sessionRows.filter((session) => {
    const matchesSearch = `${session.name} ${session.detail} ${session.type} ${session.tags.join(" ")}`.toLowerCase().includes(search.toLowerCase());
    return matchesSearch && (!favoritesOnly || session.favorite);
  });
  const localSessionCount = filteredSessions.filter((session) => session.type === "LOCAL").length;
  const remoteSessionCount = filteredSessions.filter((session) => session.type !== "LOCAL").length;
  const activeTransferCount = transfers.filter((transfer) => !["completed", "cancelled", "failed"].includes(transfer.state)).length;
  const activeTunnelCount = tunnels.filter((tunnel) => !["stopped", "failed"].includes(tunnel.state)).length;

  return (
    <main className={`app-shell ${sidebarOpen ? "" : "sidebar-collapsed"} theme-${settings.general.theme}`}>
      <header className="topbar">
        <div className="brand-lockup">
          <div className="brand-mark" aria-hidden="true">
            <span />
            <span />
            <span />
          </div>
          <div>
            <div className="brand-name">MobaRust</div>
            <div className="brand-kicker">operations deck</div>
          </div>
        </div>
        <div className="topbar-center">
          <span className="live-dot" />
          <span>LOCAL CORE ONLINE</span>
          <span className="topbar-divider" />
          <span className="muted">{IS_TAURI ? "desktop runtime" : "browser preview"}</span>
        </div>
        <div className="topbar-actions">
          <button className="icon-button" aria-label="Help" title="Help">
            <CircleHelp size={17} strokeWidth={1.7} />
          </button>
          <button className="icon-button" aria-label="Settings" title="Settings" onClick={() => setSettingsOpen(true)}>
            <Settings2 size={17} strokeWidth={1.7} />
          </button>
          <div className="avatar">OB</div>
        </div>
      </header>

      <div className="app-body">
        <aside className="sidebar">
          <div className="sidebar-toolbar">
            <button className="sidebar-toggle" onClick={() => setSidebarOpen(false)} aria-label="Collapse sidebar">
              <PanelLeftClose size={16} />
            </button>
            <button className="new-button" onClick={startNewTerminal}>
              <Plus size={15} strokeWidth={2.4} />
              <span>New terminal</span>
              <span className="shortcut">⌘ N</span>
            </button>
          </div>

          <label className="search-box">
            <Search size={15} />
            <input value={search} onChange={(event) => setSearch(event.target.value)} placeholder="Search sessions" />
            <span className="search-key">⌘ K</span>
          </label>

          <nav className="sidebar-nav" aria-label="Workspace navigation">
            <div className="nav-section-label">Workspace</div>
            <button className={`nav-item ${!favoritesOnly ? "active" : ""}`} onClick={() => setFavoritesOnly(false)}><LayoutDashboard size={15} /> Overview <span className="nav-count">{sessionRows.length}</span></button>
            <button className={`nav-item ${favoritesOnly ? "active" : ""}`} onClick={() => setFavoritesOnly(true)}><Star size={15} /> Favorites <span className="nav-count">{sessionRows.filter((session) => session.favorite).length}</span></button>
            <button className="nav-item"><Activity size={15} /> Recent</button>
          </nav>

          <div className="session-list">
            <div className="list-heading"><span>{favoritesOnly ? "Favorite sessions" : "Sessions"}</span><span className="list-actions"><button aria-label="Import OpenSSH config" title="Import OpenSSH config" onClick={importOpenSshConfig}><Upload size={14} /></button><button aria-label="Import MobaRust session export" title="Import MobaRust session export" onClick={importSessions}><ArrowDownToLine size={14} /></button><button aria-label="Export MobaRust sessions" title="Export secret-free session definitions" onClick={exportSessions}><ArrowUpFromLine size={14} /></button></span></div>
            <div className="folder-heading"><ChevronDown size={13} /> Local terminals <span>{localSessionCount}</span></div>
            {filteredSessions.filter((session) => session.type === "LOCAL").map((session) => (
              <SessionRow key={session.id ?? session.name} {...session} onSelect={startNewTerminal} onToggleFavorite={() => void toggleFavorite(session)} />
            ))}
            <div className="folder-heading muted-folder"><ChevronDown size={13} /> Remote sessions <span>{remoteSessionCount}</span></div>
            {groupSessionsByFolder(filteredSessions.filter((session) => session.type === "SSH")).map(([folder, sessions]) => (
              <div key={folder} className="session-folder-group">
                <div className="folder-heading nested-folder"><Folder size={12} /> {folder} <span>{sessions.length}</span></div>
                {sessions.map((session) => (
                  <SessionRow key={session.id ?? session.name} {...session} onSelect={() => {
                    const saved = savedSessions.find((item) => item.id === session.id);
                    if (saved) connectSavedSession(saved);
                  }} onEdit={session.id ? () => {
                    const saved = savedSessions.find((item) => item.id === session.id);
                    if (saved) setEditingSession(saved);
                  } : undefined} onDelete={session.id ? () => {
                    const saved = savedSessions.find((item) => item.id === session.id);
                    if (saved) void deleteSavedSession(saved);
                  } : undefined} onToggleFavorite={() => void toggleFavorite(session)} />
                ))}
              </div>
            ))}
            {filteredSessions.length === 0 && <div className="empty-search">No matching sessions</div>}
          </div>

          <div className="sidebar-footer">
            <div className="security-note"><ShieldCheck size={15} /><span><strong>Secrets stay native</strong><small>Vault boundary is Rust-owned</small></span></div>
            <button className={`nav-item ${activeView === "diagnostics" ? "active" : ""}`} onClick={() => setActiveView("diagnostics")}><Activity size={15} /> Network diagnostics</button>
            <button className="nav-item" onClick={() => setActiveView("tunnels")}><Network size={15} /> Tunnel manager <span className="nav-count">{activeTunnelCount}</span></button>
            <button className="nav-item"><ArrowDownToLine size={15} /> Transfers <span className="nav-count">{activeTransferCount}</span></button>
          </div>
        </aside>

        <section className="workspace">
          {!sidebarOpen && <button className="floating-sidebar-button" onClick={() => setSidebarOpen(true)} aria-label="Expand sidebar"><PanelLeftClose size={16} /></button>}
          <div className="workspace-heading">
            <div>
              <div className="eyebrow"><span>WORKSPACE / 01</span><span className="eyebrow-slash">/</span><span className="muted">{remoteProtocol ? remoteProtocol.toUpperCase() : "LOCAL"}</span></div>
              <h1>{remoteHost ?? "Local workstation"}</h1>
              <p className="workspace-subtitle">{remoteProtocol === "ssh" ? "Interactive SSH shell with native host-key verification." : remoteProtocol === "telnet" ? "Legacy Telnet terminal. Traffic is unencrypted." : remoteProtocol === "serial" ? "Serial terminal with explicit device parameters." : "A quiet command surface for the machine in front of you."}</p>
            </div>
            <div className="heading-actions">
              <button className="outline-button" onClick={() => setPaletteOpen(true)}><Command size={15} /> Command palette <span>⌘ ⇧ P</span></button>
              <button className="outline-button" onClick={() => setQuickConnectOpen(true)}><Network size={15} /> Quick connect <span>⌘ K</span></button>
              <button className="primary-button" onClick={startNewTerminal}><Plus size={15} /> New terminal</button>
            </div>
          </div>
          {sessionNotice && <div className="workspace-notice" role="status"><CheckCircle2 size={14} /><span>{sessionNotice}</span></div>}

          <div className="workspace-grid">
            <div className="main-column">
              <div className="context-strip">
                <div className="context-title"><span className="status-pulse" /> {remoteHost ?? "localhost"} <span className="context-separator">/</span> <span className="muted">{terminalStatus === "connected" ? "shell ready" : terminalStatus}</span></div>
                <div className="context-metrics"><span><TerminalIcon size={13} /> PTY</span><span><ArrowUpFromLine size={13} /> bidirectional</span><span><Radio size={13} /> 32 KB batches</span></div>
              </div>

              <div className="view-tabs" role="tablist" aria-label="Workspace views">
                <button className={activeView === "terminal" ? "selected" : ""} onClick={() => setActiveView("terminal")} role="tab" aria-selected={activeView === "terminal"}><TerminalIcon size={15} /> Terminal</button>
                <button className={activeView === "files" ? "selected" : ""} onClick={() => setActiveView("files")} role="tab" aria-selected={activeView === "files"}><Folder size={15} /> Files <span className="tab-badge">SSH</span></button>
                <button className={activeView === "tunnels" ? "selected" : ""} onClick={() => setActiveView("tunnels")} role="tab" aria-selected={activeView === "tunnels"}><Network size={15} /> Tunnels <span className="tab-badge">{activeTunnelCount}</span></button>
                <button className={activeView === "diagnostics" ? "selected" : ""} onClick={() => setActiveView("diagnostics")} role="tab" aria-selected={activeView === "diagnostics"}><Activity size={15} /> Diagnostics</button>
              </div>

              {activeView === "terminal" ? (
                <section className="terminal-card" aria-label="Terminal workspace">
                  <div className="terminal-toolbar">
                    <div className="terminal-tab"><span className="terminal-tab-dot" /><span>{remoteHost ? "remote shell" : "local shell"}</span><span className="terminal-tab-meta">{terminalStatus === "connected" ? (remoteHost ? remoteProtocol : "zsh") : terminalStatus}</span><button aria-label="Close terminal" onClick={() => { setTerminalOpen(false); setTerminalStatus("closed"); setRemoteSessionId(null); setRemoteProtocol(null); setRemoteHost(null); }}><X size={14} /></button></div>
                    <div className="terminal-toolbar-actions"><span className="terminal-chip">UTF-8</span><span className="terminal-chip">256 colors</span><button aria-label="Copy terminal output"><Copy size={14} /></button><button aria-label="Terminal options"><MoreHorizontal size={16} /></button></div>
                  </div>
                  <div className={`terminal-frame ${terminalOpen ? "" : "terminal-frame-closed"}`}>{terminalOpen ? <TerminalViewport key={terminalKey} instanceKey={terminalKey} remoteSessionId={remoteSessionId} remoteProtocol={remoteProtocol} fontSize={settings.appearance.fontSize} scrollbackLines={settings.terminal.scrollbackLines} cursorBlink={settings.terminal.cursorBlink} confirmMultilinePaste={settings.general.confirmMultilinePaste} onStatusChange={handleTerminalStatus} /> : <div className="terminal-closed"><div className="empty-protocol-art"><TerminalIcon size={21} /></div><strong>Terminal closed</strong><span>Start a fresh local or remote shell when you are ready.</span><button className="primary-button" onClick={startNewTerminal}><Plus size={14} /> New terminal</button></div>}</div>
                  <div className="terminal-statusbar"><span><span className="status-square" /> {terminalStatus === "connected" ? "connected" : terminalStatus}</span><span>{remoteProtocol ? `${remoteProtocol} transport` : "local process"}</span><span>scrollback 5,000</span><span className="terminal-status-spacer" /><span>⌘K for quick connect</span></div>
                </section>
              ) : activeView === "files" && remoteSessionId && remoteProtocol === "ssh" ? (
                <RemoteFilesView entries={remoteEntries} path={remotePath} status={sftpStatus} error={connectionError} transfers={transfers.filter((transfer) => transfer.terminalId === remoteSessionId)} onNavigate={navigateRemote} onDownload={startDownload} onUpload={startUpload} onCreateDirectory={createRemoteDirectory} onRename={renameRemote} onDelete={deleteRemote} onCancelTransfer={cancelTransfer} />
              ) : activeView === "tunnels" && remoteSessionId && remoteProtocol === "ssh" ? (
                <TunnelView tunnels={tunnels} onNewTunnel={startLocalForward} onNewDynamicForward={startDynamicForward} onNewRemoteForward={startRemoteForward} onCancelTunnel={cancelTunnel} />
              ) : activeView === "diagnostics" ? (
                <NetworkDiagnosticsView host={networkHost} port={networkPort} timeout={networkTimeout} status={networkStatus} addresses={networkAddresses} result={networkResult} error={networkError} scanId={networkScanId} scanStatus={networkScanStatus} scanStart={networkScanStart} scanEnd={networkScanEnd} scanConcurrency={networkScanConcurrency} scanScanned={networkScanScanned} scanTotal={networkScanTotal} scanResults={networkScanResults} onHostChange={setNetworkHost} onPortChange={setNetworkPort} onTimeoutChange={setNetworkTimeout} onResolve={resolveNetworkHost} onCheckTcp={checkNetworkTcp} onScanStartChange={setNetworkScanStart} onScanEndChange={setNetworkScanEnd} onScanConcurrencyChange={setNetworkScanConcurrency} onStartScan={startNetworkScan} onCancelScan={cancelNetworkScan} />
              ) : (
                <EmptyProtocolView view={activeView} onAction={activeView === "tunnels" ? () => setQuickConnectOpen(true) : undefined} />
              )}

              <div className="lower-grid">
                <InfoCard icon={ShieldCheck} label="Security boundary" title="Credentials never cross into React" detail="Session records carry references. Secret material stays in the native layer." action="Read threat model" />
                <InfoCard icon={ArrowUpFromLine} label="Transport" title="Backpressure is explicit" detail="PTY output is bounded before it reaches the renderer, keeping noisy jobs responsive." action="View architecture" />
              </div>
            </div>

            <aside className="right-rail">
              <div className="rail-heading"><span>Session brief</span><button aria-label="Session options"><MoreHorizontal size={15} /></button></div>
              <div className="machine-card">
                <div className="machine-icon"><Server size={18} /></div>
                <div><div className="machine-name">{remoteHost ?? "This Mac"}</div><div className="machine-detail">{remoteHost ? (remoteProtocol === "telnet" ? "Telnet · unencrypted" : remoteProtocol === "serial" ? "Serial · device" : "SSH · verified transport") : "Apple Silicon · local"}</div></div>
                <span className="machine-live">LIVE</span>
              </div>
              <div className="rail-group"><div className="rail-label">Runtime</div><Metric label="Shell" value={remoteHost ? "remote" : "zsh"} /><Metric label="Terminal" value="xterm-256color" /><Metric label="Process" value={terminalStatus === "connected" ? "running" : "idle"} /></div>
              <div className="rail-group"><div className="rail-label">Workspace notes</div><p className="rail-copy">The local terminal is the first real vertical slice. SSH and SFTP slots are visible so the workspace can grow without hiding unfinished protocol claims.</p></div>
              <div className="rail-callout"><div className="callout-icon"><Network size={15} /></div><div><strong>{remoteProtocol === "telnet" ? "Telnet transport active" : remoteProtocol === "serial" ? "Serial transport active" : remoteHost ? "SSH transport active" : "Connect securely"}</strong><p>{remoteProtocol === "telnet" ? "This legacy terminal is unencrypted; use SSH for protected administration." : remoteProtocol === "serial" ? "Serial traffic depends on the connected hardware; MobaRust does not add encryption." : remoteHost ? "Host-key verification and native PTY negotiation are active for this shell." : "Known-host verification and PTY negotiation are ready for a real SSH connection."}</p><button onClick={() => setQuickConnectOpen(true)}>{remoteHost ? "Open another session" : "Quick connect"} <ExternalLink size={12} /></button></div></div>
            </aside>
          </div>

          <footer className="workspace-footer"><span><span className="footer-led" /> MobaRust core · v0.1.0</span><span>Rust PTY bridge</span><span>{navigator.platform.includes("Mac") ? "macOS" : navigator.platform.includes("Win") ? "Windows" : "Linux"} · local mode</span><span className="footer-spacer" /><span>{now.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })} CET</span></footer>
        </section>
      </div>

      {paletteOpen && <CommandPalette onClose={() => setPaletteOpen(false)} onNewTerminal={startNewTerminal} onQuickConnect={() => { setQuickConnectOpen(true); setPaletteOpen(false); }} onOpenSettings={() => { setSettingsOpen(true); setPaletteOpen(false); }} onToggleSidebar={() => { setSidebarOpen((open) => !open); setPaletteOpen(false); }} />}
      {quickConnectOpen && <QuickConnectDialog error={connectionError} onClose={() => { setQuickConnectOpen(false); setConnectionError(null); }} onConnectSsh={connectSsh} onConnectTelnet={connectTelnet} onConnectSerial={connectSerial} />}
      {editingSession && <SessionEditor session={editingSession} onClose={() => setEditingSession(null)} onSave={saveEditedSession} />}
      {settingsOpen && <SettingsModal settings={settings} onClose={() => setSettingsOpen(false)} onSave={saveSettings} onReset={resetSettings} />}
    </main>
  );
}

function requestFromSavedSession(session: SavedSession): SshConnectRequest | null {
  const username = session.username?.trim();
  if (!username || session.port === 0) return null;
  const auth = session.auth.kind === "agent"
    ? { method: "agent" as const }
    : session.auth.kind === "password" && session.auth.credentialRef.trim()
      ? { method: "password" as const, credentialId: session.auth.credentialRef }
      : session.auth.kind === "privateKey" && session.auth.keyRef.trim()
        ? { method: "privateKey" as const, path: session.auth.keyRef, passphraseCredentialId: session.auth.credentialRef ?? undefined }
        : null;
  if (!auth) return null;
  return {
    host: session.hostname,
    port: session.port,
    username,
    auth,
    knownHostsPath: session.known_hosts_path ?? undefined,
    pinnedFingerprint: session.pinned_fingerprint ?? undefined,
    cols: 120,
    rows: 32,
  };
}

function toSessionListItem(session: SavedSession): SessionListItem {
  if (session.protocol === "LOCAL") {
    return { id: session.id, name: session.name, detail: "zsh · localhost", type: "LOCAL", folder: session.folder ?? "Local terminals", active: true, favorite: session.favorite, tags: session.tags };
  }
  const user = session.username ? `${session.username}@` : "";
  const port = session.port && session.port !== 22 ? `:${session.port}` : "";
  return { id: session.id, name: session.name, detail: `${user}${session.hostname}${port}`, type: session.protocol, folder: session.folder ?? "Unfiled", active: false, favorite: session.favorite, tags: session.tags };
}

function groupSessionsByFolder(sessions: SessionListItem[]): Array<[string, SessionListItem[]]> {
  const groups = new Map<string, SessionListItem[]>();
  sessions.forEach((session) => {
    const folder = session.folder.trim() || "Unfiled";
    groups.set(folder, [...(groups.get(folder) ?? []), session]);
  });
  return [...groups.entries()].sort(([first], [second]) => first.localeCompare(second));
}

function SessionRow({ name, detail, type, active, favorite, onSelect, onEdit, onDelete, onToggleFavorite }: SessionListItem & { onSelect: () => void; onEdit?: () => void; onDelete?: () => void; onToggleFavorite: () => void }) {
  return <div className={`session-row ${active ? "active" : ""}`}><button className="session-row-main" onClick={onSelect}><span className={`session-icon ${type === "LOCAL" ? "local" : "remote"}`}>{type === "LOCAL" ? <TerminalIcon size={14} /> : <Server size={14} />}</span><span className="session-copy"><strong>{name}</strong><small>{detail}</small></span><span className={`session-type ${type === "LOCAL" ? "local-type" : ""}`}>{type}</span></button><div className="session-row-actions">{onEdit && <button className="session-action" onClick={onEdit} aria-label={`Edit ${name}`} title="Edit session"><Pencil size={12} /></button>}{onDelete && <button className="session-action danger" onClick={onDelete} aria-label={`Delete ${name}`} title="Delete session"><Trash2 size={12} /></button>}<button className={`session-favorite ${favorite ? "selected" : ""}`} onClick={onToggleFavorite} aria-label={`${favorite ? "Remove" : "Add"} ${name} ${favorite ? "from" : "to"} favorites`} title={favorite ? "Remove from favorites" : "Add to favorites"}><Star size={13} fill={favorite ? "currentColor" : "none"} /></button></div></div>;
}

function RemoteFilesView({ entries, path, status, error, transfers, onNavigate, onDownload, onUpload, onCreateDirectory, onRename, onDelete, onCancelTransfer }: {
  entries: RemoteEntry[];
  path: string;
  status: "idle" | "loading" | "ready" | "error";
  error: string | null;
  transfers: SshTransferEvent[];
  onNavigate: (path: string) => void;
  onDownload: (entry: RemoteEntry) => void;
  onUpload: () => void;
  onCreateDirectory: () => void;
  onRename: (entry: RemoteEntry) => void;
  onDelete: (entry: RemoteEntry) => void;
  onCancelTransfer: (transferId: string) => void;
}) {
  const parentPath = path === "." || path === "/" ? path : path.split("/").slice(0, -1).join("/") || ".";
  return <section className="remote-files" aria-label="Remote files">
      <div className="remote-files-toolbar">
      <div><span className="eyebrow">SFTP / BROWSER</span><strong>{path}</strong></div>
      <div className="remote-files-toolbar-actions">
        <button className="outline-button" onClick={onCreateDirectory}><FolderPlus size={14} /> New folder</button>
        <button className="outline-button" onClick={onUpload}><Upload size={14} /> Upload</button>
        <button className="outline-button" onClick={() => onNavigate(path)} disabled={status === "loading"}><RefreshCw size={14} /> {status === "loading" ? "Refreshing" : "Refresh"}</button>
      </div>
    </div>
    <div className="remote-files-meta"><span>{status === "ready" ? `${entries.length} entries` : status === "error" ? "Unable to list directory" : "Streaming directory listing"}</span><span className="remote-files-safe"><ShieldCheck size={13} /> Native transport · bounded transfers</span></div>
    {error && <div className="remote-files-error" role="alert"><CircleX size={14} /><span>{error}</span></div>}
    <div className="remote-files-list">
      <div className="remote-file-row parent"><button className="remote-file-main" onClick={() => onNavigate(parentPath)}><span className="remote-file-icon"><Folder size={15} /></span><span>..</span><small>parent directory</small></button></div>
      {entries.map((entry) => <div className={`remote-file-row ${entry.isDirectory ? "directory" : ""}`} key={entry.path}>
        <button className="remote-file-main" onClick={() => entry.isDirectory ? onNavigate(entry.path) : undefined} aria-label={entry.isDirectory ? `Open ${entry.name}` : entry.name}>
          <span className="remote-file-icon">{entry.isDirectory ? <Folder size={15} /> : <ArrowDownToLine size={15} />}</span><span>{entry.name}</span><small>{entry.isDirectory ? "directory" : formatBytes(entry.size)}</small>
        </button>
        <button className="remote-file-action" onClick={() => onDownload(entry)} title={`${entry.isDirectory ? "Download directory" : "Download"} ${entry.name}`} aria-label={`${entry.isDirectory ? "Download directory" : "Download"} ${entry.name}`}><Download size={14} /></button>
        <button className="remote-file-action" onClick={() => onRename(entry)} title={`Rename ${entry.name}`} aria-label={`Rename ${entry.name}`}><Pencil size={14} /></button>
        <button className="remote-file-action danger" onClick={() => onDelete(entry)} title={`Delete ${entry.name}`} aria-label={`Delete ${entry.name}`}><Trash2 size={14} /></button>
      </div>)}
      {status === "ready" && entries.length === 0 && <div className="remote-files-empty">This directory is empty.</div>}
    </div>
    {transfers.length > 0 && <TransferPanel transfers={transfers} onCancelTransfer={onCancelTransfer} />}
    <div className="remote-files-note">Files and bounded recursive transfers run through native Rust SFTP. Individual files commit from temporary local or remote paths; cancellation never presents a partial file as complete. Directory transfers refuse symlink traversal and cap the walk at 100,000 entries.</div>
  </section>;
}

function TransferPanel({ transfers, onCancelTransfer }: { transfers: SshTransferEvent[]; onCancelTransfer: (transferId: string) => void }) {
  return <section className="transfer-panel" aria-label="Transfers"><div className="transfer-panel-heading"><span className="eyebrow">TRANSFER MANAGER</span><span>{transfers.length} recent</span></div>{transfers.slice().reverse().map((transfer) => {
    const percent = transfer.totalBytes && transfer.totalBytes > 0 ? Math.min(100, Math.round((transfer.bytesTransferred / transfer.totalBytes) * 100)) : null;
    const active = !["completed", "cancelled", "failed"].includes(transfer.state);
    return <div className="transfer-row" key={transfer.transferId}><div className="transfer-row-icon">{transfer.state === "completed" ? <CheckCircle2 size={15} /> : transfer.state === "failed" ? <CircleX size={15} /> : <LoaderCircle className={active ? "spin" : ""} size={15} />}</div><div className="transfer-row-copy"><strong>{transfer.direction === "download" ? "↓" : "↑"} {transfer.destination.split(/[\\/]/).pop() || transfer.destination}</strong><small>{transfer.state} · {formatBytes(transfer.bytesTransferred)}{transfer.totalBytes ? ` / ${formatBytes(transfer.totalBytes)}` : ""}{percent === null ? "" : ` · ${percent}%`}</small>{transfer.error && <small className="transfer-error">{transfer.error}</small>}<div className="transfer-progress"><span style={{ width: `${percent ?? (active ? 8 : 100)}%` }} /></div></div>{active && <button className="transfer-cancel" onClick={() => onCancelTransfer(transfer.transferId)} aria-label="Cancel transfer" title="Cancel transfer"><CircleX size={14} /></button>}</div>;
  })}</section>;
}

function TunnelView({ tunnels, onNewTunnel, onNewDynamicForward, onNewRemoteForward, onCancelTunnel }: { tunnels: SshTunnelEvent[]; onNewTunnel: () => void; onNewDynamicForward: () => void; onNewRemoteForward: () => void; onCancelTunnel: (tunnelId: string) => void }) {
  return <section className="tunnel-view" aria-label="SSH tunnel manager">
    <div className="tunnel-view-toolbar">
      <div><span className="eyebrow">SSH / PORT FORWARDING</span><strong>Port forwarding</strong><p>Choose the direction explicitly. Listeners stay bounded and every tunnel can be stopped.</p></div>
      <div className="tunnel-view-actions"><button className="outline-button" onClick={onNewDynamicForward}><Network size={14} /> New SOCKS5 proxy</button><button className="outline-button" onClick={onNewRemoteForward}><ArrowUpFromLine size={14} /> New remote forward</button><button className="primary-button" onClick={onNewTunnel}><Plus size={14} /> New local forward</button></div>
    </div>
    <div className="tunnel-view-meta"><span>{tunnels.length} recent tunnel{tunnels.length === 1 ? "" : "s"}</span><span className="remote-files-safe"><ShieldCheck size={13} /> Native direct-tcpip · max 16 clients</span></div>
    {tunnels.length === 0 ? <div className="tunnel-empty"><Network size={19} /><strong>No tunnels yet</strong><span>Create a local, remote, or SOCKS5 tunnel through this SSH session.</span><div className="tunnel-empty-actions"><button className="outline-button" onClick={onNewTunnel}><Plus size={14} /> Create local forward</button><button className="outline-button" onClick={onNewRemoteForward}><ArrowUpFromLine size={14} /> Create remote forward</button><button className="outline-button" onClick={onNewDynamicForward}><Network size={14} /> Create SOCKS5 proxy</button></div></div> : <div className="tunnel-list">{tunnels.slice().reverse().map((tunnel) => {
      const active = !["stopped", "failed"].includes(tunnel.state);
      const stateIcon = tunnel.state === "failed" ? <CircleX size={15} /> : tunnel.state === "stopped" ? <CheckCircle2 size={15} /> : <LoaderCircle className={active ? "spin" : ""} size={15} />;
      const remote = tunnel.kind === "remote";
      const dynamic = tunnel.kind === "dynamic";
      return <article className={`tunnel-row tunnel-${tunnel.state}`} key={tunnel.tunnelId}>
        <div className="tunnel-row-icon">{stateIcon}</div>
        <div className="tunnel-row-copy"><div className="tunnel-endpoints"><strong>{tunnel.localHost}:{tunnel.localPort}</strong><span>{remote ? "⇢" : "→"}</span><strong>{dynamic ? "SOCKS5" : `${tunnel.targetHost}:${tunnel.targetPort}`}</strong></div><small>{remote ? "remote listener → local target" : dynamic ? "local SOCKS5 proxy" : "local listener → remote target"} · {tunnel.state} · {tunnel.connections} connection{tunnel.connections === 1 ? "" : "s"} · {formatBytes(tunnel.bytesForwarded)} forwarded</small>{tunnel.error && <small className="tunnel-error">{tunnel.error}</small>}</div>
        {active && <button className="transfer-cancel" onClick={() => onCancelTunnel(tunnel.tunnelId)} aria-label="Stop tunnel" title="Stop tunnel"><CircleX size={14} /></button>}
      </article>;
    })}</div>}
    <div className="tunnel-view-note">Local forwarding listens only on the bind address you choose. The remote target is resolved from the SSH server, and tunnel output is not interpreted as terminal HTML.</div>
  </section>;
}

function NetworkDiagnosticsView({ host, port, timeout, status, addresses, result, error, scanId, scanStatus, scanStart, scanEnd, scanConcurrency, scanScanned, scanTotal, scanResults, onHostChange, onPortChange, onTimeoutChange, onResolve, onCheckTcp, onScanStartChange, onScanEndChange, onScanConcurrencyChange, onStartScan, onCancelScan }: {
  host: string;
  port: string;
  timeout: string;
  status: "idle" | "running" | "ready" | "error";
  addresses: string[];
  result: TcpCheckResult | null;
  error: string | null;
  scanId: string | null;
  scanStatus: "idle" | "running" | "completed" | "cancelled" | "failed";
  scanStart: string;
  scanEnd: string;
  scanConcurrency: string;
  scanScanned: number;
  scanTotal: number;
  scanResults: TcpCheckResult[];
  onHostChange: (value: string) => void;
  onPortChange: (value: string) => void;
  onTimeoutChange: (value: string) => void;
  onResolve: () => void;
  onCheckTcp: () => void;
  onScanStartChange: (value: string) => void;
  onScanEndChange: (value: string) => void;
  onScanConcurrencyChange: (value: string) => void;
  onStartScan: () => void;
  onCancelScan: () => void;
}) {
  const statusLabel = status === "running" ? "Running" : status === "ready" ? "Ready" : status === "error" ? "Needs attention" : "Idle";
  const scanActive = scanStatus === "running";
  const scanProgress = scanTotal > 0 ? Math.min(100, Math.round((scanScanned / scanTotal) * 100)) : 0;
  return <section className="diagnostics-view" aria-label="Network diagnostics">
    <div className="diagnostics-toolbar">
      <div><span className="eyebrow">NETWORK / DIAGNOSTICS</span><strong>Inspect one explicit target</strong><p>Resolve a hostname, check one TCP port, or run a bounded range from the desktop runtime. Nothing starts automatically.</p></div>
      <div className={`diagnostics-status diagnostics-${status}`}><span /> {statusLabel}</div>
    </div>
    <div className="diagnostics-target">
      <label className="diagnostics-host">Target host or IP<input value={host} onChange={(event) => onHostChange(event.target.value)} placeholder="127.0.0.1 or host.example" /></label>
      <label>TCP port<input inputMode="numeric" pattern="[0-9]+" value={port} onChange={(event) => onPortChange(event.target.value)} /></label>
      <label>Timeout ms<input inputMode="numeric" pattern="[0-9]+" value={timeout} onChange={(event) => onTimeoutChange(event.target.value)} /></label>
    </div>
    <div className="diagnostics-actions"><button className="outline-button" onClick={onResolve} disabled={status === "running"}><Search size={14} /> Resolve DNS</button><button className="primary-button" onClick={onCheckTcp} disabled={status === "running"}><Network size={14} /> Check TCP port</button></div>
    {error && <div className="connect-error diagnostics-error" role="alert"><CircleX size={14} /><span>{error}</span></div>}
    <div className="diagnostics-results">
      <article className="diagnostic-card"><div className="diagnostic-card-heading"><span className="eyebrow">DNS / ADDRESSES</span><Search size={15} /></div><h3>{addresses.length > 0 ? `${addresses.length} address${addresses.length === 1 ? "" : "es"}` : "No lookup yet"}</h3>{addresses.length > 0 ? <div className="diagnostic-addresses">{addresses.map((address) => <code key={address}>{address}</code>)}</div> : <p>Enter a target and run an explicit lookup. Results are kept in this view only.</p>}</article>
      <article className="diagnostic-card"><div className="diagnostic-card-heading"><span className="eyebrow">TCP / REACHABILITY</span><Network size={15} /></div>{result ? <><h3>{result.host}:{result.port}</h3><div className={`diagnostic-result diagnostic-result-${result.status}`}><span /> {result.status === "open" ? "Open" : result.status === "closed" ? "Closed" : "Timed out"}</div><p>The result describes TCP reachability only; it does not authenticate or identify the service.</p></> : <><h3>No TCP check yet</h3><p>Choose a port explicitly, then run a bounded connection check.</p></>}</article>
    </div>
    <section className="diagnostics-scan" aria-label="Bounded TCP port scan">
      <div className="diagnostics-scan-heading"><div><span className="eyebrow">TCP / BOUNDED SCAN</span><h3>Scan an explicit range</h3><p>Maximum 4096 ports, maximum 128 concurrent checks, and a visible cancellation control.</p></div><span className={`diagnostics-scan-state scan-${scanStatus}`}>{scanStatus === "idle" ? "Ready" : scanStatus}</span></div>
      <div className="diagnostics-scan-fields"><label>Start port<input inputMode="numeric" pattern="[0-9]+" value={scanStart} onChange={(event) => onScanStartChange(event.target.value)} disabled={scanActive} /></label><label>End port<input inputMode="numeric" pattern="[0-9]+" value={scanEnd} onChange={(event) => onScanEndChange(event.target.value)} disabled={scanActive} /></label><label>Concurrency<input inputMode="numeric" pattern="[0-9]+" value={scanConcurrency} onChange={(event) => onScanConcurrencyChange(event.target.value)} disabled={scanActive} /></label><div className="diagnostics-scan-action">{scanActive ? <button className="outline-button" onClick={onCancelScan}><CircleX size={14} /> Cancel scan</button> : <button className="primary-button" onClick={onStartScan}><Search size={14} /> Start bounded scan</button>}</div></div>
      {(scanStatus !== "idle" || scanResults.length > 0) && <div className="diagnostics-scan-progress"><div className="diagnostics-progress-label"><span>{scanId ? `Scan ${scanId.slice(0, 8)}` : "Scan"}</span><strong>{scanScanned}/{scanTotal || "—"} · {scanProgress}%</strong></div><div className="diagnostics-progress-track"><span style={{ width: `${scanProgress}%` }} /></div><div className="diagnostics-open-results">{scanResults.length > 0 ? scanResults.filter((item) => item.status === "open").map((item) => <code key={item.port}>{item.port} open</code>) : <span>No open ports reported yet.</span>}</div></div>}
    </section>
    <div className="diagnostics-note"><ShieldCheck size={14} /><span>Safety boundary: target, range, concurrency, timeout, and action are explicit. Results describe TCP reachability only and are not a security audit.</span></div>
  </section>;
}

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function EmptyProtocolView({ view, onAction }: { view: "files" | "tunnels"; onAction?: () => void }) {
  const isFiles = view === "files";
  return <section className="empty-protocol"><div className="empty-protocol-art"><div className="empty-ring ring-one" /><div className="empty-ring ring-two" />{isFiles ? <Folder size={24} /> : <Network size={24} />}</div><span className="eyebrow">{isFiles ? "REMOTE FILES" : "NETWORK FABRIC"}</span><h2>{isFiles ? "SFTP browser is staged for the SSH slice" : "No tunnels are active"}</h2><p>{isFiles ? "This surface will only appear as usable once streaming transfers, cancellation, and path safety are implemented." : "Create a tunnel from a connected SSH session. The manager will expose endpoints, ownership, state, and byte counts."}</p>{onAction ? <button className="outline-button" onClick={onAction}><Network size={14} /> Quick connect</button> : <button className="outline-button" disabled><Settings2 size={14} /> Delivery map</button>}</section>;
}

function InfoCard({ icon: Icon, label, title, detail, action }: { icon: LucideIcon; label: string; title: string; detail: string; action: string }) {
  return <article className="info-card"><div className="info-card-top"><span className="info-icon"><Icon size={15} /></span><span>{label}</span><button aria-label="More information"><MoreHorizontal size={15} /></button></div><h3>{title}</h3><p>{detail}</p><button className="text-button">{action} <ExternalLink size={12} /></button></article>;
}

function Metric({ label, value }: { label: string; value: string }) {
  return <div className="metric"><span>{label}</span><strong>{value}</strong></div>;
}

function CommandPalette({ onClose, onNewTerminal, onQuickConnect, onOpenSettings, onToggleSidebar }: { onClose: () => void; onNewTerminal: () => void; onQuickConnect: () => void; onOpenSettings: () => void; onToggleSidebar: () => void }) {
  const [query, setQuery] = useState("");
  const commands = quickActions.filter((action) => action.label.toLowerCase().includes(query.toLowerCase()));
  return <div className="palette-backdrop" role="presentation" onMouseDown={onClose}><section className="command-palette" role="dialog" aria-modal="true" aria-label="Command palette" onMouseDown={(event) => event.stopPropagation()}><div className="palette-search"><Search size={17} /><input autoFocus value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search commands" /><kbd>ESC</kbd></div><div className="palette-section-label">Actions</div>{commands.map((action) => { const ActionIcon = action.icon; const run = action.label === "New local terminal" ? onNewTerminal : action.label === "Quick connect" ? onQuickConnect : action.label === "Settings" ? onOpenSettings : onClose; return <button key={action.label} className="palette-item" onClick={() => { run(); onClose(); }}><ActionIcon size={16} /><span>{action.label}</span><kbd>{action.hint}</kbd></button>; })}<button className="palette-item" onClick={onToggleSidebar}><PanelLeftClose size={16} /><span>Toggle sidebar</span><kbd>⌘ B</kbd></button><div className="palette-footer"><span>Navigate <b>↑ ↓</b></span><span>Run <b>↵</b></span><span>Close <b>esc</b></span></div></section></div>;
}

function SessionEditor({ session, onClose, onSave }: { session: SavedSession; onClose: () => void; onSave: (session: SavedSession) => void }) {
  const [name, setName] = useState(session.name);
  const [folder, setFolder] = useState(session.folder ?? "");
  const [tags, setTags] = useState(session.tags.join(", "));
  const [startupDirectory, setStartupDirectory] = useState(session.startup_directory ?? "");
  const [startupCommand, setStartupCommand] = useState(session.startup_command ?? "");
  const [notes, setNotes] = useState(session.notes ?? "");
  const [favorite, setFavorite] = useState(session.favorite);

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const normalizedTags = [...new Set(tags.split(",").map((tag) => tag.trim()).filter(Boolean))];
    onSave({
      ...session,
      name: name.trim(),
      folder: folder.trim() || null,
      tags: normalizedTags,
      favorite,
      startup_directory: startupDirectory.trim() || null,
      startup_command: startupCommand.trim() || null,
      notes: notes.trim() || null,
      environment: session.environment ?? [],
    });
  };

  const endpoint = session.username ? `${session.username}@${session.hostname}:${session.port}` : `${session.hostname}:${session.port}`;
  const authLabel = session.auth.kind === "agent" ? "SSH agent" : session.auth.kind === "password" ? "Vault credential reference" : session.auth.kind === "privateKey" ? "Private key reference" : "No authentication";

  return (
    <div className="palette-backdrop" role="presentation" onMouseDown={onClose}>
      <form className="session-editor" role="dialog" aria-modal="true" aria-label={`Edit ${session.name}`} onMouseDown={(event) => event.stopPropagation()} onSubmit={submit}>
        <div className="session-editor-heading">
          <div>
            <span className="eyebrow">SESSION / METADATA</span>
            <h2>Edit session</h2>
            <p>Organize this profile without exposing or changing its credential material.</p>
          </div>
          <button type="button" className="icon-button" aria-label="Close session editor" onClick={onClose}><X size={17} /></button>
        </div>

        <div className="session-editor-summary">
          <div><span>Protocol</span><strong>{session.protocol}</strong></div>
          <div><span>Endpoint</span><strong>{endpoint}</strong></div>
          <div><span>Authentication</span><strong>{authLabel}</strong></div>
        </div>

        <div className="session-editor-grid">
          <label className="quick-connect-wide">
            Session name
            <input autoFocus required value={name} onChange={(event) => setName(event.target.value)} />
          </label>
          <label>
            Folder
            <input value={folder} onChange={(event) => setFolder(event.target.value)} placeholder="Production" />
          </label>
          <label>
            Tags
            <input value={tags} onChange={(event) => setTags(event.target.value)} placeholder="prod, bastion" />
            <small>Separate tags with commas.</small>
          </label>
          <label>
            Startup directory <span className="optional">optional</span>
            <input value={startupDirectory} onChange={(event) => setStartupDirectory(event.target.value)} placeholder="/srv/app" />
          </label>
          <label>
            Startup command <span className="optional">optional</span>
            <input value={startupCommand} onChange={(event) => setStartupCommand(event.target.value)} placeholder="htop" />
          </label>
          <label className="quick-connect-wide">
            Notes <span className="optional">optional</span>
            <textarea value={notes} onChange={(event) => setNotes(event.target.value)} placeholder="Operational notes" rows={3} />
          </label>
        </div>

        <div className="session-editor-footer">
          <button type="button" className={`favorite-toggle ${favorite ? "selected" : ""}`} onClick={() => setFavorite((value) => !value)}><Star size={14} fill={favorite ? "currentColor" : "none"} /> {favorite ? "Favorite" : "Add to favorites"}</button>
          <div><button type="button" className="outline-button" onClick={onClose}>Cancel</button><button type="submit" className="primary-button"><CheckCircle2 size={14} /> Save changes</button></div>
        </div>
      </form>
    </div>
  );
}

function SettingsModal({ settings, onClose, onSave, onReset }: { settings: AppSettings; onClose: () => void; onSave: (settings: AppSettings) => void; onReset: () => void }) {
  const [theme, setTheme] = useState(settings.general.theme);
  const [confirmMultilinePaste, setConfirmMultilinePaste] = useState(settings.general.confirmMultilinePaste);
  const [fontSize, setFontSize] = useState(String(settings.appearance.fontSize));
  const [scrollbackLines, setScrollbackLines] = useState(String(settings.terminal.scrollbackLines));
  const [cursorBlink, setCursorBlink] = useState(settings.terminal.cursorBlink);
  const [reconnectEnabled, setReconnectEnabled] = useState(settings.ssh.reconnectEnabled);
  const [reconnectAttempts, setReconnectAttempts] = useState(String(settings.ssh.reconnectAttempts));
  const [connectTimeoutMs, setConnectTimeoutMs] = useState(String(settings.ssh.connectTimeoutMs));
  const [diagnosticTimeoutMs, setDiagnosticTimeoutMs] = useState(String(settings.network.diagnosticTimeoutMs));
  const [scanConcurrency, setScanConcurrency] = useState(String(settings.network.scanConcurrency));

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    onSave({
      general: { theme, confirmMultilinePaste },
      appearance: { fontSize: Number(fontSize) },
      terminal: { scrollbackLines: Number(scrollbackLines), cursorBlink },
      ssh: { reconnectEnabled, reconnectAttempts: Number(reconnectAttempts), connectTimeoutMs: Number(connectTimeoutMs) },
      network: { diagnosticTimeoutMs: Number(diagnosticTimeoutMs), scanConcurrency: Number(scanConcurrency) },
    });
  };

  return (
    <div className="palette-backdrop" role="presentation" onMouseDown={onClose}>
      <form className="settings-modal" role="dialog" aria-modal="true" aria-label="Settings" onMouseDown={(event) => event.stopPropagation()} onSubmit={submit}>
        <div className="session-editor-heading">
          <div>
            <span className="eyebrow">MOBA / SETTINGS</span>
            <h2>Workspace settings</h2>
            <p>Typed, validated preferences. Secrets and credential material are not stored here.</p>
          </div>
          <button type="button" className="icon-button" aria-label="Close settings" onClick={onClose}><X size={17} /></button>
        </div>

        <div className="settings-section">
          <span className="settings-section-label">General & appearance</span>
          <div className="settings-grid">
            <label>Theme<select value={theme} onChange={(event) => setTheme(event.target.value as AppSettings["general"]["theme"])}><option value="dark">Dark</option><option value="light">Light</option><option value="system">System</option></select></label>
            <label>Font size<input type="number" min="8" max="32" value={fontSize} onChange={(event) => setFontSize(event.target.value)} /><small>8–32 px; new terminal instances.</small></label>
            <label>Scrollback lines<input type="number" min="100" max="100000" value={scrollbackLines} onChange={(event) => setScrollbackLines(event.target.value)} /><small>100–100,000 lines.</small></label>
            <label className="settings-check"><input type="checkbox" checked={cursorBlink} onChange={(event) => setCursorBlink(event.target.checked)} /> Cursor blink</label>
            <label className="settings-check"><input type="checkbox" checked={confirmMultilinePaste} onChange={(event) => setConfirmMultilinePaste(event.target.checked)} /> Confirm multiline paste</label>
          </div>
        </div>

        <div className="settings-section">
          <span className="settings-section-label">SSH resilience</span>
          <div className="settings-grid">
            <label className="settings-check"><input type="checkbox" checked={reconnectEnabled} onChange={(event) => setReconnectEnabled(event.target.checked)} /> Enable bounded reconnect</label>
            <label>Reconnect attempts<input type="number" min="0" max="10" value={reconnectAttempts} onChange={(event) => setReconnectAttempts(event.target.value)} /><small>0–10 attempts.</small></label>
            <label>Connect timeout ms<input type="number" min="100" max="60000" value={connectTimeoutMs} onChange={(event) => setConnectTimeoutMs(event.target.value)} /></label>
          </div>
        </div>

        <div className="settings-section">
          <span className="settings-section-label">Network diagnostics</span>
          <div className="settings-grid">
            <label>Default timeout ms<input type="number" min="50" max="60000" value={diagnosticTimeoutMs} onChange={(event) => setDiagnosticTimeoutMs(event.target.value)} /></label>
            <label>Default scan concurrency<input type="number" min="1" max="128" value={scanConcurrency} onChange={(event) => setScanConcurrency(event.target.value)} /><small>1–128 bounded checks.</small></label>
          </div>
        </div>

        <div className="session-editor-footer">
          <button type="button" className="outline-button" onClick={onReset}>Reset defaults</button>
          <div><button type="button" className="outline-button" onClick={onClose}>Cancel</button><button type="submit" className="primary-button"><CheckCircle2 size={14} /> Save settings</button></div>
        </div>
      </form>
    </div>
  );
}

function QuickConnectDialog({ error, onClose, onConnectSsh, onConnectTelnet, onConnectSerial }: { error: string | null; onClose: () => void; onConnectSsh: (request: SshConnectRequest) => void; onConnectTelnet: (request: TelnetConnectRequest) => void; onConnectSerial: (request: SerialConnectRequest) => void }) {
  const [host, setHost] = useState("");
  const [port, setPort] = useState("22");
  const [username, setUsername] = useState("");
  const [protocol, setProtocol] = useState<"ssh" | "telnet" | "serial">("ssh");
  const [method, setMethod] = useState<"agent" | "privateKey" | "password">("agent");
  const [keyPath, setKeyPath] = useState("");
  const [passphraseCredentialId, setPassphraseCredentialId] = useState("");
  const [credentialId, setCredentialId] = useState("");
  const [knownHostsPath, setKnownHostsPath] = useState("");
  const [pinnedFingerprint, setPinnedFingerprint] = useState("");
  const [jumpHost, setJumpHost] = useState("");
  const [jumpPort, setJumpPort] = useState("22");
  const [jumpUsername, setJumpUsername] = useState("");
  const [terminal, setTerminal] = useState("xterm-256color");
  const [encoding, setEncoding] = useState<"utf-8" | "windows-1252">("utf-8");
  const [serialDevice, setSerialDevice] = useState("");
  const [baudRate, setBaudRate] = useState("115200");
  const [dataBits, setDataBits] = useState<SerialConnectRequest["dataBits"]>("eight");
  const [stopBits, setStopBits] = useState<SerialConnectRequest["stopBits"]>("one");
  const [parity, setParity] = useState<SerialConnectRequest["parity"]>("none");
  const [flowControl, setFlowControl] = useState<SerialConnectRequest["flowControl"]>("none");
  const [lineEnding, setLineEnding] = useState<SerialConnectRequest["lineEnding"]>("cr-lf");

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (protocol === "telnet") {
      onConnectTelnet({
        host: host.trim(),
        port: Number(port),
        terminal: terminal.trim(),
        encoding,
        columns: 120,
        rows: 32,
      });
      return;
    }
    if (protocol === "serial") {
      onConnectSerial({
        device: serialDevice.trim(),
        baudRate: Number(baudRate),
        dataBits,
        stopBits,
        parity,
        flowControl,
        lineEnding,
      });
      return;
    }
    const auth = method === "agent"
      ? { method: "agent" as const }
      : method === "privateKey"
        ? { method: "privateKey" as const, path: keyPath, passphraseCredentialId: passphraseCredentialId.trim() || undefined }
        : { method: "password" as const, credentialId };
    const parsedJumpPort = Number(jumpPort);
    const jumpHosts = jumpHost.trim() && Number.isInteger(parsedJumpPort) && parsedJumpPort > 0 && parsedJumpPort <= 65535
      ? [{ host: jumpHost.trim(), port: parsedJumpPort, username: jumpUsername.trim() || username.trim(), auth: { method: "agent" as const } }]
      : undefined;
    onConnectSsh({
      host: host.trim(),
      port: Number(port),
      username: username.trim(),
      auth,
      knownHostsPath: knownHostsPath.trim() || undefined,
      pinnedFingerprint: pinnedFingerprint.trim() || undefined,
      jumpHosts,
      cols: 120,
      rows: 32,
    });
  };

  return (
    <div className="palette-backdrop" role="presentation" onMouseDown={onClose}>
      <form
        className="quick-connect"
        role="dialog"
        aria-modal="true"
        aria-label="Quick connect"
        onMouseDown={(event) => event.stopPropagation()}
        onSubmit={submit}
      >
        <div className="quick-connect-heading">
          <div>
            <span className="eyebrow">NEW REMOTE SESSION</span>
            <h2>Quick connect</h2>
            <p>
              {protocol === "ssh"
                ? "Open a real native SSH shell in seconds."
                : protocol === "telnet"
                  ? "Open a legacy Telnet terminal with a visible plaintext warning."
                  : "Open a serial terminal with explicit hardware parameters."}
            </p>
          </div>
          <button type="button" className="icon-button" aria-label="Close quick connect" onClick={onClose}>
            <X size={17} />
          </button>
        </div>

        <div className="quick-connect-grid">
          <label className="quick-connect-wide">
            Protocol
            <select
              value={protocol}
              onChange={(event) => {
                const next = event.target.value as "ssh" | "telnet" | "serial";
                setProtocol(next);
                setPort(next === "ssh" ? "22" : "23");
              }}
            >
              <option value="ssh">SSH</option>
              <option value="telnet">Telnet · unencrypted</option>
              <option value="serial">Serial device</option>
            </select>
          </label>

          {protocol === "serial" ? (
            <>
              <label className="quick-connect-wide">
                Device path
                <input autoFocus required value={serialDevice} onChange={(event) => setSerialDevice(event.target.value)} placeholder="/dev/tty.usbserial-… or COM3" />
                <small>No device enumeration is performed automatically.</small>
              </label>
              <label>
                Baud rate
                <input required inputMode="numeric" pattern="[0-9]+" value={baudRate} onChange={(event) => setBaudRate(event.target.value)} />
              </label>
              <label>
                Data bits
                <select value={dataBits} onChange={(event) => setDataBits(event.target.value as SerialConnectRequest["dataBits"])}>
                  <option value="five">5</option>
                  <option value="six">6</option>
                  <option value="seven">7</option>
                  <option value="eight">8</option>
                </select>
              </label>
              <label>
                Stop bits
                <select value={stopBits} onChange={(event) => setStopBits(event.target.value as SerialConnectRequest["stopBits"])}>
                  <option value="one">1</option>
                  <option value="two">2</option>
                </select>
              </label>
              <label>
                Parity
                <select value={parity} onChange={(event) => setParity(event.target.value as SerialConnectRequest["parity"])}>
                  <option value="none">None</option>
                  <option value="odd">Odd</option>
                  <option value="even">Even</option>
                </select>
              </label>
              <label>
                Flow control
                <select value={flowControl} onChange={(event) => setFlowControl(event.target.value as SerialConnectRequest["flowControl"])}>
                  <option value="none">None</option>
                  <option value="software">Software</option>
                  <option value="hardware">Hardware</option>
                </select>
              </label>
              <label className="quick-connect-wide">
                Line ending
                <select value={lineEnding} onChange={(event) => setLineEnding(event.target.value as SerialConnectRequest["lineEnding"])}>
                  <option value="cr-lf">CRLF</option>
                  <option value="cr">CR</option>
                  <option value="lf">LF</option>
                  <option value="none">None</option>
                </select>
              </label>
            </>
          ) : (
            <>
              <label>
                Host
                <input autoFocus required value={host} onChange={(event) => setHost(event.target.value)} placeholder="bastion.example.com" />
              </label>
              <label>
                Port
                <input required inputMode="numeric" pattern="[0-9]+" value={port} onChange={(event) => setPort(event.target.value)} />
              </label>

              {protocol === "ssh" ? (
                <>
                  <label className="quick-connect-wide">
                    Username
                    <input required value={username} onChange={(event) => setUsername(event.target.value)} placeholder="ops" />
                  </label>
                  <label className="quick-connect-wide">
                    Authentication
                    <select value={method} onChange={(event) => setMethod(event.target.value as "agent" | "privateKey" | "password")}>
                      <option value="agent">Local SSH agent</option>
                      <option value="privateKey">Private key path</option>
                      <option value="password">Existing vault credential reference</option>
                    </select>
                  </label>
                  {method === "privateKey" ? (
                    <>
                      <label className="quick-connect-wide">
                        Private key path
                        <input required value={keyPath} onChange={(event) => setKeyPath(event.target.value)} placeholder="~/.ssh/id_ed25519" />
                        <small>The key stays on disk; only its path crosses IPC.</small>
                      </label>
                      <label className="quick-connect-wide">
                        Passphrase credential reference <span className="optional">optional</span>
                        <input value={passphraseCredentialId} onChange={(event) => setPassphraseCredentialId(event.target.value)} placeholder="prod-key-passphrase" />
                        <small>Encrypted-key passphrases are retrieved natively from the vault.</small>
                      </label>
                    </>
                  ) : method === "password" ? (
                    <label className="quick-connect-wide">
                      Credential reference
                      <input required value={credentialId} onChange={(event) => setCredentialId(event.target.value)} placeholder="prod-bastion-password" />
                      <small>Only an opaque vault reference crosses IPC, never the password.</small>
                    </label>
                  ) : (
                    <div className="quick-connect-wide quick-connect-hint">
                      <ShieldCheck size={14} />
                      <span>The native SSH agent signs authentication; private key material stays with the agent.</span>
                    </div>
                  )}
                  <label className="quick-connect-wide">
                    Jump host <span className="optional">optional · SSH agent</span>
                    <input value={jumpHost} onChange={(event) => setJumpHost(event.target.value)} placeholder="bastion.internal.example" />
                  </label>
                  {jumpHost.trim() && (
                    <>
                      <label>
                        Jump port
                        <input required inputMode="numeric" pattern="[0-9]+" value={jumpPort} onChange={(event) => setJumpPort(event.target.value)} />
                      </label>
                      <label>
                        Jump username
                        <input required value={jumpUsername} onChange={(event) => setJumpUsername(event.target.value)} placeholder={username || "ops"} />
                      </label>
                    </>
                  )}
                  <label className="quick-connect-wide">
                    Known hosts path <span className="optional">optional</span>
                    <input value={knownHostsPath} onChange={(event) => setKnownHostsPath(event.target.value)} placeholder="Default: ~/.ssh/known_hosts" />
                  </label>
                  <label className="quick-connect-wide">
                    Pinned SHA-256 fingerprint <span className="optional">optional</span>
                    <input value={pinnedFingerprint} onChange={(event) => setPinnedFingerprint(event.target.value)} placeholder="SHA256:... (for deliberate first trust)" />
                  </label>
                </>
              ) : (
                <>
                  <label className="quick-connect-wide">
                    Terminal type
                    <input required value={terminal} onChange={(event) => setTerminal(event.target.value)} placeholder="xterm-256color" />
                  </label>
                  <label className="quick-connect-wide">
                    Encoding
                    <select value={encoding} onChange={(event) => setEncoding(event.target.value as "utf-8" | "windows-1252")}>
                      <option value="utf-8">UTF-8</option>
                      <option value="windows-1252">Windows-1252</option>
                    </select>
                  </label>
                </>
              )}
            </>
          )}
        </div>

        {error && (
          <div className="connect-error" role="alert">
            <strong>Connection failed</strong>
            <span>{error}</span>
          </div>
        )}

        <div className="quick-connect-footer">
          <span>
            <ShieldCheck size={14} />
            {protocol === "ssh"
              ? "Unknown host keys are rejected."
              : protocol === "telnet"
                ? "Telnet traffic is plaintext and unauthenticated."
                : "Serial device traffic is not encrypted by MobaRust."}
          </span>
          <div>
            <button type="button" className="outline-button" onClick={onClose}>Cancel</button>
            <button className="primary-button" type="submit">
              <Network size={14} />
              Connect {protocol === "ssh" ? "SSH" : protocol === "telnet" ? "Telnet" : "Serial"}
            </button>
          </div>
        </div>
      </form>
    </div>
  );
}

export default App;
