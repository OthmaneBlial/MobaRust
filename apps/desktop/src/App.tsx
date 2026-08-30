import { useCallback, useEffect, useRef, useState, type ClipboardEvent as ReactClipboardEvent, type FormEvent, type KeyboardEvent as ReactKeyboardEvent, type MouseEvent as ReactMouseEvent, type ReactNode, type UIEvent as ReactUIEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { Terminal } from "@xterm/xterm";
import {
  Activity,
  ArrowDownToLine,
  ArrowUpFromLine,
  BookOpen,
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
  Gauge,
  KeyRound,
  LoaderCircle,
  LayoutDashboard,
  Maximize2,
  MoreHorizontal,
  Minimize2,
  Network,
  PanelBottom,
  PanelLeftClose,
  PanelRight,
  Pencil,
  Plus,
  Play,
  Radio,
  RefreshCw,
  Search,
  Server,
  Settings2,
  ShieldAlert,
  ShieldCheck,
  Star,
  Square,
  Terminal as TerminalIcon,
  Trash2,
  Upload,
  X,
  type LucideIcon,
} from "lucide-react";

type View = "terminal" | "files" | "tunnels" | "monitor" | "diagnostics" | "transfers";

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

type PortableVaultStatus = {
  enabled: boolean;
  unlocked: boolean;
  exists: boolean;
  path: string;
};

type VaultBackend = "platform" | "portable";

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

type DesktopProtocol = "rdp" | "vnc";

type RemoteDesktopConnectRequest = {
  protocol: DesktopProtocol;
  host: string;
  port: number;
  username: string;
  domain?: string;
  credentialId?: string;
  width: number;
  height: number;
  colorDepth: number;
  audioEnabled: boolean;
};

type RemoteDesktopConnectResponse = {
  sessionId: string;
  protocol: DesktopProtocol;
  host: string;
};

type RemoteDesktopEvent = {
  sessionId: string;
  event:
    | { event: "hello"; payload: { version: number } }
    | { event: "state"; payload: { state: "created" | "starting" | "ready" | "active" | "reconnecting" | "stopping" | "stopped" | "crashed" | "failed" } }
    | { event: "framebuffer"; payload: { width: number; height: number; pixels: number[] } }
    | { event: "clipboard"; payload: { text: string } }
    | { event: "diagnostic"; payload: { level: string; message: string } };
};

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
  workspaceId: string;
  instanceKey: number;
  remoteSessionId: string | null;
  remoteProtocol: "ssh" | "telnet" | "serial" | DesktopProtocol | null;
  remoteDesktopRequest?: RemoteDesktopConnectRequest | null;
  fontSize: number;
  scrollbackLines: number;
  cursorBlink: boolean;
  confirmMultilinePaste: boolean;
  onStatusChange: (workspaceId: string, status: TerminalStatus) => void;
  onNativeTerminalId: (workspaceId: string, terminalId: string | null) => void;
  onInput: (workspaceId: string, terminalId: string, data: string) => void;
  onTerminalReady: (workspaceId: string, terminal: Terminal, searchAddon: SearchAddon) => void;
  onTerminalDisposed: (workspaceId: string) => void;
  onSearchResults: (workspaceId: string, resultIndex: number, resultCount: number) => void;
};

type WorkspaceTerminal = {
  id: string;
  instanceKey: number;
  label: string;
  remoteSessionId: string | null;
  remoteProtocol: "ssh" | "telnet" | "serial" | DesktopProtocol | null;
  remoteDesktopRequest?: RemoteDesktopConnectRequest | null;
  remoteHost: string | null;
  status: TerminalStatus;
};

type SplitDirection = "none" | "right" | "down";

type TerminalLayoutNode =
  | { kind: "pane"; terminalId: string }
  | { kind: "split"; direction: Exclude<SplitDirection, "none">; ratio: number; first: TerminalLayoutNode; second: TerminalLayoutNode };

type SplitPathPart = "first" | "second";
type SplitPath = SplitPathPart[];

function layoutTerminalIds(node: TerminalLayoutNode): string[] {
  return node.kind === "pane" ? [node.terminalId] : [...layoutTerminalIds(node.first), ...layoutTerminalIds(node.second)];
}

function findLayoutPath(node: TerminalLayoutNode, terminalId: string, path: SplitPath = []): SplitPath | null {
  if (node.kind === "pane") return node.terminalId === terminalId ? path : null;
  return findLayoutPath(node.first, terminalId, [...path, "first"]) ?? findLayoutPath(node.second, terminalId, [...path, "second"]);
}

function replaceLayoutNode(node: TerminalLayoutNode, path: SplitPath, replacement: TerminalLayoutNode): TerminalLayoutNode {
  if (path.length === 0) return replacement;
  if (node.kind === "pane") return node;
  const [head, ...rest] = path;
  return head === "first"
    ? { ...node, first: replaceLayoutNode(node.first, rest, replacement) }
    : { ...node, second: replaceLayoutNode(node.second, rest, replacement) };
}

function updateLayoutRatio(node: TerminalLayoutNode, path: SplitPath, ratio: number): TerminalLayoutNode {
  if (path.length === 0) return node.kind === "split" ? { ...node, ratio } : node;
  if (node.kind === "pane") return node;
  const [head, ...rest] = path;
  return head === "first"
    ? { ...node, first: updateLayoutRatio(node.first, rest, ratio) }
    : { ...node, second: updateLayoutRatio(node.second, rest, ratio) };
}

function removeLayoutNode(node: TerminalLayoutNode, terminalId: string): TerminalLayoutNode | null {
  if (node.kind === "pane") return node.terminalId === terminalId ? null : node;
  const first = removeLayoutNode(node.first, terminalId);
  const second = removeLayoutNode(node.second, terminalId);
  if (!first) return second;
  if (!second) return first;
  return { ...node, first, second };
}

let terminalInstanceCounter = 0;

function createWorkspaceTerminal(config: Partial<Omit<WorkspaceTerminal, "id" | "instanceKey" | "status">> = {}): WorkspaceTerminal {
  terminalInstanceCounter += 1;
  return {
    id: crypto.randomUUID(),
    instanceKey: terminalInstanceCounter,
    label: "local shell",
    remoteSessionId: null,
    remoteProtocol: null,
    remoteHost: null,
    status: "starting",
    ...config,
  };
}

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
  lastUsedAt?: number | null;
};

type SavedSession = {
  id: string;
  name: string;
  protocol: string;
  hostname: string;
  port: number;
  username?: string | null;
  last_used_at?: number | null;
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
  serial_profile?: {
    device: string;
    baud_rate: number;
    data_bits: "five" | "six" | "seven" | "eight";
    stop_bits: "one" | "two";
    parity: "none" | "odd" | "even";
    flow_control: "none" | "software" | "hardware";
    line_ending: "none" | "cr-lf" | "cr" | "lf";
  } | null;
  jump_host_profiles?: SavedJumpHost[];
  remote_desktop_profile?: {
    domain?: string | null;
    width: number;
    height: number;
    color_depth: number;
    audio_enabled: boolean;
  } | null;
  auth: SavedAuth;
};

type SavedAuth =
  | { kind: "none" }
  | { kind: "agent" }
  | { kind: "password"; credentialRef: string }
  | { kind: "privateKey"; keyRef: string; credentialRef?: string | null }
  | { kind: "keyboardInteractive"; credentialRef: string };

type SavedJumpHost = {
  host: string;
  port: number;
  username: string;
  known_hosts_path?: string | null;
  pinned_fingerprint?: string | null;
  auth: SavedAuth;
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

type SnippetRecord = {
  id: string;
  title: string;
  description: string;
  command: string;
  tags: string[];
  variables: string[];
};

type MacroKey = "enter" | "escape" | "tab" | "backspace" | "ctrlC" | "ctrlD" | "arrowUp" | "arrowDown" | "arrowLeft" | "arrowRight";
type MacroApprovalPolicy = "beforeRun" | "eachAction";

type MacroAction =
  | { kind: "sendText"; text: string }
  | { kind: "wait"; milliseconds: number }
  | { kind: "sendKey"; key: MacroKey }
  | { kind: "executeCommand"; command: string }
  | { kind: "openSession"; sessionId: string }
  | { kind: "switchWorkspace"; workspaceId: string };

type MacroRecord = {
  id: string;
  title: string;
  description: string;
  tags: string[];
  actions: MacroAction[];
  approval: MacroApprovalPolicy;
};

type MacroRecordingState = {
  terminalId: string;
  terminalLabel: string;
  actions: MacroAction[];
  textBytes: number;
};

const MAX_RECORDED_MACRO_ACTIONS = 64;
const MAX_RECORDED_MACRO_TEXT_BYTES = 64 * 1024;

function normalizeMacroRecord(record: MacroRecord): MacroRecord {
  return { ...record, approval: record.approval ?? "beforeRun" };
}

function recordedMacroActions(data: string): MacroAction[] {
  const controlKeys: Array<[string, MacroKey]> = [
    ["\x1b[A", "arrowUp"],
    ["\x1b[B", "arrowDown"],
    ["\x1b[D", "arrowLeft"],
    ["\x1b[C", "arrowRight"],
    ["\r\n", "enter"],
    ["\x03", "ctrlC"],
    ["\x04", "ctrlD"],
    ["\x7f", "backspace"],
    ["\x1b", "escape"],
    ["\t", "tab"],
    ["\r", "enter"],
    ["\n", "enter"],
  ];
  const actions: MacroAction[] = [];
  let text = "";
  const flushText = () => {
    if (text) actions.push({ kind: "sendText", text });
    text = "";
  };
  let index = 0;
  while (index < data.length) {
    const control = controlKeys.find(([sequence]) => data.startsWith(sequence, index));
    if (control) {
      flushText();
      actions.push({ kind: "sendKey", key: control[1] });
      index += control[0].length;
      continue;
    }
    const code = data.charCodeAt(index);
    if (code < 0x20 || code === 0x7f) {
      flushText();
      index += 1;
      continue;
    }
    text += data[index];
    index += 1;
  }
  flushText();
  return actions;
}

type SshAuthRequest =
  | { method: "agent" }
  | { method: "privateKey"; path: string; passphraseCredentialId?: string }
  | { method: "password"; credentialId: string }
  | { method: "keyboardInteractive"; credentialId: string };

type SshConnectRequest = {
  host: string;
  port: number;
  username: string;
  auth: SshAuthRequest;
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
  auth: SshAuthRequest;
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

type SerialDeviceInfo = {
  device: string;
  kind: string;
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
  uid?: number | null;
  owner?: string | null;
  gid?: number | null;
  group?: string | null;
  permissions?: number | null;
};

type RemoteTextDocument = {
  path: string;
  content: string;
  revision: string;
  size: number;
  modifiedUnixSeconds?: number | null;
  permissions?: number | null;
  encoding: "utf-8" | "windows-1252";
};

type RemoteMonitorSnapshot = {
  hostname?: string | null;
  kernel?: string | null;
  uptimeSeconds?: number | null;
  loadAverage?: [number, number, number] | null;
  memoryTotalBytes?: number | null;
  memoryAvailableBytes?: number | null;
  rootDiskUsedPercent?: number | null;
  processCount?: number | null;
  supportedMetrics: string[];
};

type TransferProtocol = "sftp" | "scp";
type RemoteFileSort = "name" | "type" | "size" | "modified";

type TransferState = "queued" | "preparing" | "running" | "paused" | "cancelling" | "cancelled" | "completed" | "failed";

type SshTransferEvent = {
  transferId: string;
  terminalId: string;
  direction: "download" | "upload";
  protocol: TransferProtocol;
  source: string;
  destination: string;
  recursive: boolean;
  bytesTransferred: number;
  totalBytes?: number | null;
  bytesPerSecond?: number | null;
  etaSeconds?: number | null;
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

type SshHostKeyInspection = {
  host: string;
  port: number;
  fingerprint: string;
};

type NetworkScanEvent = {
  scanId: string;
  state: "running" | "completed" | "cancelled" | "failed";
  scanned: number;
  total: number;
  result?: TcpCheckResult | null;
  error?: string | null;
};

type PingResult = {
  host: string;
  reachable: boolean;
  elapsedMs: number;
  last_used_at?: number | null;
};

type TracerouteResult = {
  host: string;
  reached: boolean;
  hops: string[];
  elapsedMs: number;
};

type NetworkDiagnosticEvent = {
  operationId: string;
  kind: "ping" | "traceroute";
  state: "running" | "completed" | "cancelled" | "failed";
  ping?: PingResult | null;
  traceroute?: TracerouteResult | null;
  error?: string | null;
};

const previewSessions: SessionListItem[] = [
  { name: "Local workstation", detail: "zsh · localhost", type: "LOCAL", folder: "Local terminals", active: true, favorite: true, tags: ["local"], lastUsedAt: 3 },
  { name: "Production bastion", detail: "ops@bastion.example", type: "SSH", folder: "Production", active: false, favorite: true, tags: ["production"], lastUsedAt: 2 },
  { name: "Staging cluster", detail: "dev@staging.example", type: "SSH", folder: "Staging", active: false, favorite: false, tags: ["staging"], lastUsedAt: 1 },
];

const quickActions: Array<{ label: string; hint: string; icon: LucideIcon }> = [
  { label: "New local terminal", hint: "⌘ N", icon: TerminalIcon },
  { label: "Quick connect", hint: "⌘ K", icon: Network },
  { label: "Open SFTP", hint: "⌘ ⇧ F", icon: Folder },
  { label: "Settings", hint: "", icon: Settings2 },
  { label: "Credential vault", hint: "", icon: KeyRound },
  { label: "Snippets", hint: "", icon: BookOpen },
  { label: "Macros", hint: "", icon: Play },
  { label: "Command palette", hint: "⌘ ⇧ P", icon: Command },
];

function TerminalViewport({ workspaceId, instanceKey, remoteSessionId, remoteProtocol, fontSize, scrollbackLines, cursorBlink, confirmMultilinePaste, onStatusChange, onNativeTerminalId, onInput, onTerminalReady, onTerminalDisposed, onSearchResults }: TerminalViewportProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const terminalIdRef = useRef<string | null>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const terminalOptionsRef = useRef({ fontSize, scrollbackLines, cursorBlink });
  const confirmMultilinePasteRef = useRef(confirmMultilinePaste);

  useEffect(() => {
    terminalOptionsRef.current = { fontSize, scrollbackLines, cursorBlink };
    if (terminalRef.current) {
      terminalRef.current.options.fontSize = fontSize;
      terminalRef.current.options.cursorBlink = cursorBlink;
    }
  }, [cursorBlink, fontSize, scrollbackLines]);

  useEffect(() => {
    confirmMultilinePasteRef.current = confirmMultilinePaste;
  }, [confirmMultilinePaste]);

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
      cursorBlink: terminalOptionsRef.current.cursorBlink,
      cursorStyle: "bar",
      fontFamily: '"JetBrains Mono", "SFMono-Regular", Consolas, monospace',
      fontSize: terminalOptionsRef.current.fontSize,
      lineHeight: 1.35,
      scrollback: terminalOptionsRef.current.scrollbackLines,
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
    terminalRef.current = terminal;
    const fitAddon = new FitAddon();
    terminal.loadAddon(fitAddon);
    const searchAddon = new SearchAddon({ highlightLimit: 1000 });
    terminal.loadAddon(searchAddon);
    terminal.open(host);
    const searchResults = searchAddon.onDidChangeResults((event) => onSearchResults(workspaceId, event.resultIndex, event.resultCount));
    onTerminalReady(workspaceId, terminal, searchAddon);

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
      onInput(workspaceId, terminalId, data);
    };

    const input = terminal.onData((data) => {
      sendTerminalInput(data);
    });

    const onPaste = (event: ClipboardEvent) => {
      const data = event.clipboardData?.getData("text/plain") ?? "";
      if (!confirmMultilinePasteRef.current || (!data.includes("\n") && !data.includes("\r"))) return;
      event.preventDefault();
      event.stopPropagation();
      const accepted = window.confirm("This paste contains multiple lines. Send it to the terminal? Nothing will be executed automatically by MobaRust.");
      if (accepted) sendTerminalInput(data);
    };
    host.addEventListener("paste", onPaste, true);

    const boot = async () => {
      onStatusChange(workspaceId, "starting");
      if (!IS_TAURI) {
        terminal.writeln("MobaRust browser preview");
        terminal.writeln("The real PTY is enabled in the desktop runtime.");
        terminal.writeln("\x1b[38;5;179m$\x1b[0m preview --ready");
        onStatusChange(workspaceId, "connected");
        return;
      }

      try {
        const outputEvent = remoteProtocol === "ssh" ? "ssh://output" : remoteProtocol === "telnet" ? "telnet://output" : remoteProtocol === "serial" ? "serial://output" : "terminal://output";
        const closedEvent = remoteProtocol === "ssh" ? "ssh://closed" : remoteProtocol === "telnet" ? "telnet://closed" : remoteProtocol === "serial" ? "serial://closed" : "terminal://closed";
        unlistenOutput = await listen<TerminalOutputEvent>(outputEvent, (event) => {
          if (event.payload.terminalId === terminalIdRef.current) terminal.write(event.payload.data);
        });
        unlistenClosed = await listen<TerminalClosedEvent>(closedEvent, (event) => {
          if (event.payload.terminalId === terminalIdRef.current) onStatusChange(workspaceId, "closed");
        });
        if (remoteProtocol === "ssh") {
          unlistenState = await listen<SshSessionEvent>("ssh://state", (event) => {
            if (event.payload.terminalId !== terminalIdRef.current) return;
            if (event.payload.state === "connected") onStatusChange(workspaceId, "connected");
            else if (event.payload.state === "reconnecting") onStatusChange(workspaceId, "reconnecting");
            else if (event.payload.state === "failed") onStatusChange(workspaceId, "error");
          });
        }
        if (remoteProtocol === "telnet") {
          unlistenState = await listen<TelnetSessionEvent>("telnet://state", (event) => {
            if (event.payload.terminalId !== terminalIdRef.current) return;
            if (event.payload.state === "connected") onStatusChange(workspaceId, "connected");
            else if (event.payload.state === "failed") onStatusChange(workspaceId, "error");
            else if (event.payload.state === "disconnected") onStatusChange(workspaceId, "closed");
          });
        }
        if (remoteProtocol === "serial") {
          unlistenState = await listen<SerialSessionEvent>("serial://state", (event) => {
            if (event.payload.terminalId !== terminalIdRef.current) return;
            if (event.payload.state === "connected") onStatusChange(workspaceId, "connected");
            else if (event.payload.state === "failed") onStatusChange(workspaceId, "error");
            else if (event.payload.state === "disconnected") onStatusChange(workspaceId, "closed");
          });
        }
        if (remoteSessionId) {
          terminalIdRef.current = remoteSessionId;
          onNativeTerminalId(workspaceId, remoteSessionId);
          const attachCommand = remoteProtocol === "ssh" ? "ssh_attach" : remoteProtocol === "telnet" ? "telnet_attach" : "serial_attach";
          const closeCommand = remoteProtocol === "ssh" ? "ssh_close" : remoteProtocol === "telnet" ? "telnet_close" : "serial_close";
          const pendingOutput = await invoke<string[]>(attachCommand, { terminalId: remoteSessionId });
          if (disposed) {
            void invoke(closeCommand, { terminalId: remoteSessionId });
            return;
          }
          pendingOutput.forEach((data) => terminal.write(data));
          onStatusChange(workspaceId, "connected");
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
        onNativeTerminalId(workspaceId, terminalId);
        onStatusChange(workspaceId, "connected");
        fit();
      } catch {
        onStatusChange(workspaceId, "error");
        terminal.writeln("\r\n\x1b[38;5;203mUnable to start the local PTY.\x1b[0m");
      }
    };
    void boot();

    return () => {
      disposed = true;
      input.dispose();
      searchResults.dispose();
      host.removeEventListener("paste", onPaste, true);
      resizeObserver.disconnect();
      unlistenOutput?.();
      unlistenClosed?.();
      unlistenState?.();
      const terminalId = terminalIdRef.current;
      onNativeTerminalId(workspaceId, null);
      if (IS_TAURI && terminalId) {
        const closeCommand = remoteProtocol === "ssh" ? "ssh_close" : remoteProtocol === "telnet" ? "telnet_close" : remoteProtocol === "serial" ? "serial_close" : "terminal_close";
        void invoke(closeCommand, { terminalId });
      }
      terminalIdRef.current = null;
      terminalRef.current = null;
      onTerminalDisposed(workspaceId);
      terminal.dispose();
    };
  }, [instanceKey, onInput, onNativeTerminalId, onSearchResults, onStatusChange, onTerminalDisposed, onTerminalReady, remoteProtocol, remoteSessionId, workspaceId]);

  return <div className="terminal-host" ref={hostRef} aria-label="Local terminal" />;
}

function RemoteDesktopViewport({ workspaceId, instanceKey, request, onStatusChange, onNativeTerminalId }: {
  workspaceId: string;
  instanceKey: number;
  request: RemoteDesktopConnectRequest;
  onStatusChange: (workspaceId: string, status: TerminalStatus) => void;
  onNativeTerminalId: (workspaceId: string, terminalId: string | null) => void;
}) {
  const hostRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const sessionIdRef = useRef<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [dimensions, setDimensions] = useState({ width: request.width, height: request.height });
  const [remoteClipboard, setRemoteClipboard] = useState<string | null>(null);
  const [clipboardCopied, setClipboardCopied] = useState(false);
  const [connectAttempt, setConnectAttempt] = useState(0);
  const [isFullscreen, setIsFullscreen] = useState(false);
  const [fullscreenError, setFullscreenError] = useState<string | null>(null);

  useEffect(() => {
    const host = hostRef.current;
    const canvas = canvasRef.current;
    if (!host || !canvas) return;
    let disposed = false;
    let unlisten: UnlistenFn | undefined;

    const setErrorAndFail = (message: string) => {
      if (disposed) return;
      setError(message);
      onStatusChange(workspaceId, "error");
    };

    const sendResize = () => {
      if (request.protocol === "vnc") return;
      const width = Math.max(320, Math.min(4096, Math.round(host.clientWidth)));
      const height = Math.max(200, Math.min(4096, Math.round(host.clientHeight)));
      setDimensions({ width, height });
      const sessionId = sessionIdRef.current;
      if (!IS_TAURI || !sessionId || width === 0 || height === 0) return;
      void invoke("remote_desktop_resize", { sessionId, width, height }).catch(() => undefined);
    };

    const renderFramebuffer = (width: number, height: number, pixels: number[]) => {
      if (pixels.length !== width * height * 4) {
        setErrorAndFail("The remote desktop sent an invalid framebuffer.");
        return;
      }
      canvas.width = width;
      canvas.height = height;
      const context = canvas.getContext("2d");
      if (!context) {
        setErrorAndFail("The remote desktop renderer is unavailable.");
        return;
      }
      context.putImageData(new ImageData(new Uint8ClampedArray(pixels), width, height), 0, 0);
      setDimensions({ width, height });
    };

    const boot = async () => {
      setError(null);
      setRemoteClipboard(null);
      setClipboardCopied(false);
      setFullscreenError(null);
      onStatusChange(workspaceId, "starting");
      if (!IS_TAURI) {
        setError(`${request.protocol.toUpperCase()} requires the desktop runtime; browser preview does not open remote hosts.`);
        onStatusChange(workspaceId, "error");
        return;
      }
      try {
        unlisten = await listen<RemoteDesktopEvent>("remote-desktop://event", (event) => {
          if (event.payload.sessionId !== sessionIdRef.current) return;
          const helperEvent = event.payload.event;
          if (helperEvent.event === "state") {
            if (helperEvent.payload.state === "ready" || helperEvent.payload.state === "active") onStatusChange(workspaceId, "connected");
            if (helperEvent.payload.state === "reconnecting") onStatusChange(workspaceId, "reconnecting");
            if (helperEvent.payload.state === "failed" || helperEvent.payload.state === "crashed") setErrorAndFail("The remote desktop helper stopped unexpectedly.");
            if (helperEvent.payload.state === "stopped") onStatusChange(workspaceId, "closed");
          }
          if (helperEvent.event === "framebuffer") renderFramebuffer(helperEvent.payload.width, helperEvent.payload.height, helperEvent.payload.pixels);
          if (helperEvent.event === "clipboard") {
            setRemoteClipboard(helperEvent.payload.text);
            setClipboardCopied(false);
          }
          if (helperEvent.event === "diagnostic") setError(helperEvent.payload.message);
        });
        const response = await invoke<RemoteDesktopConnectResponse>("remote_desktop_start", { request });
        if (disposed) {
          void invoke("remote_desktop_stop", { sessionId: response.sessionId });
          return;
        }
        sessionIdRef.current = response.sessionId;
        onNativeTerminalId(workspaceId, response.sessionId);
        sendResize();
      } catch (startError) {
        setErrorAndFail(String(startError));
      }
    };

    const syncFullscreen = () => setIsFullscreen(document.fullscreenElement === host);
    document.addEventListener("fullscreenchange", syncFullscreen);

    const observer = new ResizeObserver(sendResize);
    observer.observe(host);
    void boot();

    return () => {
      disposed = true;
      observer.disconnect();
      document.removeEventListener("fullscreenchange", syncFullscreen);
      unlisten?.();
      const sessionId = sessionIdRef.current;
      sessionIdRef.current = null;
      onNativeTerminalId(workspaceId, null);
      if (IS_TAURI && sessionId) void invoke("remote_desktop_stop", { sessionId });
    };
  }, [connectAttempt, instanceKey, onNativeTerminalId, onStatusChange, request, workspaceId]);

  const copyRemoteClipboard = async () => {
    if (remoteClipboard === null) return;
    if (!navigator.clipboard?.writeText) {
      setError("The desktop runtime does not expose a safe clipboard writer.");
      return;
    }
    try {
      await navigator.clipboard.writeText(remoteClipboard);
      setClipboardCopied(true);
    } catch {
      setError("Copying remote clipboard text was blocked by the desktop runtime.");
    }
  };

  const toggleFullscreen = async () => {
    const host = hostRef.current;
    if (!host) return;
    setFullscreenError(null);
    try {
      if (document.fullscreenElement === host) await document.exitFullscreen();
      else await host.requestFullscreen();
    } catch {
      setFullscreenError("Fullscreen mode was blocked by the desktop runtime.");
    }
  };

  const sendKey = (event: ReactKeyboardEvent<HTMLCanvasElement>, pressed: boolean) => {
    const sessionId = sessionIdRef.current;
    const code = desktopKeyCode(request.protocol, event);
    if (!IS_TAURI || !sessionId || code === null) return;
    event.preventDefault();
    void invoke("remote_desktop_key", { sessionId, scancode: code, pressed }).catch((sendError) => setError(String(sendError)));
  };

  const sendPointer = (event: ReactMouseEvent<HTMLCanvasElement>) => {
    const sessionId = sessionIdRef.current;
    const canvas = canvasRef.current;
    if (!IS_TAURI || !sessionId || !canvas) return;
    const bounds = canvas.getBoundingClientRect();
    const x = Math.max(0, Math.min(canvas.width - 1, Math.round((event.clientX - bounds.left) * canvas.width / Math.max(bounds.width, 1))));
    const y = Math.max(0, Math.min(canvas.height - 1, Math.round((event.clientY - bounds.top) * canvas.height / Math.max(bounds.height, 1))));
    void invoke("remote_desktop_pointer", { sessionId, x, y, buttons: event.buttons }).catch((sendError) => setError(String(sendError)));
  };

  const paste = (event: ReactClipboardEvent<HTMLCanvasElement>) => {
    const sessionId = sessionIdRef.current;
    const text = event.clipboardData.getData("text/plain");
    if (!IS_TAURI || !sessionId || !text) return;
    event.preventDefault();
    void invoke("remote_desktop_clipboard", { payload: { sessionId, text } }).catch((sendError) => setError(String(sendError)));
  };

  return <div className="remote-desktop-viewport" ref={hostRef} aria-label={`${request.protocol.toUpperCase()} remote desktop`}>
    <canvas ref={canvasRef} className="remote-desktop-canvas" tabIndex={0} onKeyDown={(event) => sendKey(event, true)} onKeyUp={(event) => sendKey(event, false)} onMouseDown={sendPointer} onMouseUp={sendPointer} onMouseMove={(event) => event.buttons > 0 && sendPointer(event)} onPaste={paste} onContextMenu={(event) => event.preventDefault()} />
    {remoteClipboard !== null && <div className="remote-desktop-clipboard" role="status" aria-live="polite"><div><strong>Remote clipboard received</strong><small>Review it before copying into this Mac.</small></div><button type="button" className="outline-button" onClick={() => void copyRemoteClipboard()}><Copy size={13} />{clipboardCopied ? "Copied" : "Copy text"}</button></div>}
    {error && <div className="remote-desktop-reconnect" role="alert"><div><strong>Remote desktop unavailable</strong><small>{error}</small></div><button type="button" className="outline-button" onClick={() => setConnectAttempt((attempt) => attempt + 1)}><RefreshCw size={13} />Reconnect</button></div>}
    {fullscreenError && <div className="remote-desktop-notice" role="status" aria-live="polite">{fullscreenError}</div>}
    <button type="button" className="remote-desktop-fullscreen" aria-label={isFullscreen ? "Exit fullscreen" : "Enter fullscreen"} title={isFullscreen ? "Exit fullscreen" : "Enter fullscreen"} onClick={() => void toggleFullscreen()}>{isFullscreen ? <Minimize2 size={14} /> : <Maximize2 size={14} />}</button>
    <div className="remote-desktop-overlay"><span className="eyebrow">{request.protocol.toUpperCase()} / NATIVE HELPER</span><strong>{dimensions.width} × {dimensions.height}</strong><small>Click the canvas to focus · input stays inside the native protocol boundary</small></div>
  </div>;
}

function desktopKeyCode(protocol: DesktopProtocol, event: ReactKeyboardEvent<HTMLCanvasElement>): number | null {
  if (protocol === "vnc") {
    const special: Record<string, number> = { Enter: 0xff0d, Escape: 0xff1b, Backspace: 0xff08, Tab: 0xff09, Shift: 0xffe1, Control: 0xffe3, Alt: 0xffe9, Meta: 0xffe7, ArrowLeft: 0xff51, ArrowUp: 0xff52, ArrowRight: 0xff53, ArrowDown: 0xff54, Delete: 0xffff, Home: 0xff50, End: 0xff57, PageUp: 0xff55, PageDown: 0xff56 };
    return special[event.key] ?? (event.key.length === 1 ? event.key.codePointAt(0) ?? null : null);
  }
  const scanCodes: Record<string, number> = {
    KeyA: 0x1e, KeyB: 0x30, KeyC: 0x2e, KeyD: 0x20, KeyE: 0x12, KeyF: 0x21, KeyG: 0x22, KeyH: 0x23, KeyI: 0x17, KeyJ: 0x24, KeyK: 0x25, KeyL: 0x26, KeyM: 0x32, KeyN: 0x31, KeyO: 0x18, KeyP: 0x19, KeyQ: 0x10, KeyR: 0x13, KeyS: 0x1f, KeyT: 0x14, KeyU: 0x16, KeyV: 0x2f, KeyW: 0x11, KeyX: 0x2d, KeyY: 0x15, KeyZ: 0x2c,
    Digit0: 0x0b, Digit1: 0x02, Digit2: 0x03, Digit3: 0x04, Digit4: 0x05, Digit5: 0x06, Digit6: 0x07, Digit7: 0x08, Digit8: 0x09, Digit9: 0x0a,
    Enter: 0x1c, Escape: 0x01, Backspace: 0x0e, Tab: 0x0f, Space: 0x39, Minus: 0x0c, Equal: 0x0d, BracketLeft: 0x1a, BracketRight: 0x1b, Backslash: 0x2b, Semicolon: 0x27, Quote: 0x28, Comma: 0x33, Period: 0x34, Slash: 0x35,
    ShiftLeft: 0x2a, ShiftRight: 0x36, ControlLeft: 0x1d, ControlRight: 0x1d, AltLeft: 0x38, AltRight: 0x38, ArrowUp: 0xc8, ArrowDown: 0xd0, ArrowLeft: 0xcb, ArrowRight: 0xcd, Delete: 0xd3, Home: 0xc7, End: 0xcf, PageUp: 0xc9, PageDown: 0xd1,
    F1: 0x3b, F2: 0x3c, F3: 0x3d, F4: 0x3e, F5: 0x3f, F6: 0x40, F7: 0x41, F8: 0x42, F9: 0x43, F10: 0x44, F11: 0x57, F12: 0x58,
  };
  return scanCodes[event.code] ?? null;
}

function TerminalLayoutView({
  node,
  terminals,
  renderPane,
  onFocus,
  onStartResize,
  onAdjustResize,
  onResetResize,
}: {
  node: TerminalLayoutNode;
  terminals: WorkspaceTerminal[];
  renderPane: (terminal: WorkspaceTerminal) => ReactNode;
  onFocus: (terminalId: string) => void;
  onStartResize: (event: ReactMouseEvent<HTMLDivElement>, path: SplitPath, direction: Exclude<SplitDirection, "none">) => void;
  onAdjustResize: (event: ReactKeyboardEvent<HTMLDivElement>, path: SplitPath, direction: Exclude<SplitDirection, "none">) => void;
  onResetResize: (path: SplitPath) => void;
}) {
  const visibleIds = new Set(layoutTerminalIds(node));

  const renderNode = (current: TerminalLayoutNode, path: SplitPath): ReactNode => {
    if (current.kind === "pane") {
      const terminal = terminals.find((item) => item.id === current.terminalId);
      return <div key={`pane-${current.terminalId}`} className="terminal-pane terminal-layout-pane active" onMouseDown={() => onFocus(current.terminalId)} onFocusCapture={() => onFocus(current.terminalId)}>{terminal ? renderPane(terminal) : null}</div>;
    }

    const gridStyle = current.direction === "right"
      ? { gridTemplateColumns: `minmax(0, ${current.ratio}%) 1px minmax(0, ${100 - current.ratio}%)` }
      : { gridTemplateRows: `minmax(0, ${current.ratio}%) 1px minmax(0, ${100 - current.ratio}%)` };
    const dividerPath = path;
    return <div key={`split-${path.join("-") || "root"}`} className={`terminal-layout-split terminal-layout-split-${current.direction}`} style={gridStyle}>
      {renderNode(current.first, [...path, "first"])}
      <div
        className={`terminal-split-divider terminal-split-divider-${current.direction}`}
        role="separator"
        aria-orientation={current.direction === "right" ? "vertical" : "horizontal"}
        tabIndex={0}
        aria-label="Resize split panes"
        aria-valuemin={20}
        aria-valuemax={80}
        aria-valuenow={Math.round(current.ratio)}
        onMouseDown={(event) => onStartResize(event, dividerPath, current.direction)}
        onDoubleClick={() => onResetResize(dividerPath)}
        onKeyDown={(event) => onAdjustResize(event, dividerPath, current.direction)}
      />
      {renderNode(current.second, [...path, "second"])}
    </div>;
  };

  return <>
    <div className="terminal-layout-root">{renderNode(node, [])}</div>
    <div className="terminal-hidden-panes" aria-hidden="true">
      {terminals.filter((terminal) => !visibleIds.has(terminal.id)).map((terminal) => <div key={`hidden-${terminal.id}`} className="terminal-pane terminal-pane-hidden">{renderPane(terminal)}</div>)}
    </div>
  </>;
}

function App() {
  const [activeView, setActiveView] = useState<View>("terminal");
  const [terminalTabs, setTerminalTabs] = useState<WorkspaceTerminal[]>(() => [createWorkspaceTerminal()]);
  const [activeTerminalId, setActiveTerminalId] = useState("");
  const [terminalLayout, setTerminalLayout] = useState<TerminalLayoutNode>({ kind: "pane", terminalId: "" });
  const [terminalSearchOpen, setTerminalSearchOpen] = useState(false);
  const [terminalSearchQuery, setTerminalSearchQuery] = useState("");
  const [terminalSearchCaseSensitive, setTerminalSearchCaseSensitive] = useState(false);
  const [terminalSearchResult, setTerminalSearchResult] = useState({ resultIndex: -1, resultCount: 0 });
  const [search, setSearch] = useState("");
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [quickConnectOpen, setQuickConnectOpen] = useState(false);
  const [helpOpen, setHelpOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [credentialsOpen, setCredentialsOpen] = useState(false);
  const [snippetsOpen, setSnippetsOpen] = useState(false);
  const [macrosOpen, setMacrosOpen] = useState(false);
  const [broadcastOpen, setBroadcastOpen] = useState(false);
  const [broadcastEnabled, setBroadcastEnabled] = useState(false);
  const [broadcastTargetIds, setBroadcastTargetIds] = useState<string[]>([]);
  const [macroRun, setMacroRun] = useState<{ title: string; step: number; total: number; targets: string[] } | null>(null);
  const [macroRecording, setMacroRecording] = useState<MacroRecordingState | null>(null);
  const [recordedMacroDraft, setRecordedMacroDraft] = useState<MacroRecord | null>(null);
  const [settings, setSettings] = useState<AppSettings>(defaultSettings);
  const [portableVaultStatus, setPortableVaultStatus] = useState<PortableVaultStatus | null>(null);
  const [connectionError, setConnectionError] = useState<string | null>(null);
  const [sessionNotice, setSessionNotice] = useState<string | null>(null);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [favoritesOnly, setFavoritesOnly] = useState(false);
  const [now, setNow] = useState(() => new Date());
  const [sessionRows, setSessionRows] = useState<SessionListItem[]>(IS_TAURI ? [] : previewSessions);
  const [recentOnly, setRecentOnly] = useState(false);
  const [savedSessions, setSavedSessions] = useState<SavedSession[]>([]);
  const [snippets, setSnippets] = useState<SnippetRecord[]>([]);
  const [macros, setMacros] = useState<MacroRecord[]>([]);
  const [editingSession, setEditingSession] = useState<SavedSession | null>(null);
  const [remotePath, setRemotePath] = useState(".");
  const [remoteEntries, setRemoteEntries] = useState<RemoteEntry[]>([]);
  const [editingRemoteFile, setEditingRemoteFile] = useState<RemoteTextDocument | null>(null);
  const [sftpStatus, setSftpStatus] = useState<"idle" | "loading" | "ready" | "error">("idle");
  const [transfers, setTransfers] = useState<SshTransferEvent[]>([]);
  const [tunnels, setTunnels] = useState<SshTunnelEvent[]>([]);
  const [remoteMonitor, setRemoteMonitor] = useState<RemoteMonitorSnapshot | null>(null);
  const [remoteMonitorStatus, setRemoteMonitorStatus] = useState<"idle" | "loading" | "ready" | "error">("idle");
  const [remoteMonitorError, setRemoteMonitorError] = useState<string | null>(null);
  const [networkHost, setNetworkHost] = useState("");
  const [networkPort, setNetworkPort] = useState("22");
  const [networkTimeout, setNetworkTimeout] = useState("1500");
  const [networkStatus, setNetworkStatus] = useState<"idle" | "running" | "ready" | "error">("idle");
  const [networkAddresses, setNetworkAddresses] = useState<string[]>([]);
  const [networkResult, setNetworkResult] = useState<TcpCheckResult | null>(null);
  const [networkFingerprint, setNetworkFingerprint] = useState<SshHostKeyInspection | null>(null);
  const [networkError, setNetworkError] = useState<string | null>(null);
  const [networkDiagnosticId, setNetworkDiagnosticId] = useState<string | null>(null);
  const [networkDiagnosticKind, setNetworkDiagnosticKind] = useState<"ping" | "traceroute" | null>(null);
  const [networkDiagnosticStatus, setNetworkDiagnosticStatus] = useState<"idle" | "running" | "completed" | "cancelled" | "failed">("idle");
  const [networkPingResult, setNetworkPingResult] = useState<PingResult | null>(null);
  const [networkTracerouteResult, setNetworkTracerouteResult] = useState<TracerouteResult | null>(null);
  const [networkTraceMaxHops, setNetworkTraceMaxHops] = useState("8");
  const [networkScanId, setNetworkScanId] = useState<string | null>(null);
  const [networkScanStatus, setNetworkScanStatus] = useState<"idle" | "running" | "completed" | "cancelled" | "failed">("idle");
  const [networkScanStart, setNetworkScanStart] = useState("1");
  const [networkScanEnd, setNetworkScanEnd] = useState("1024");
  const [networkScanConcurrency, setNetworkScanConcurrency] = useState("32");
  const [networkScanScanned, setNetworkScanScanned] = useState(0);
  const [networkScanTotal, setNetworkScanTotal] = useState(0);
  const [networkScanResults, setNetworkScanResults] = useState<TcpCheckResult[]>([]);
  const networkScanIdRef = useRef<string | null>(null);
  const networkDiagnosticIdRef = useRef<string | null>(null);
  const nativeTerminalIdsRef = useRef(new Map<string, string>());
  const terminalInstancesRef = useRef(new Map<string, { terminal: Terminal; searchAddon: SearchAddon }>());
  const selectedTerminalIdRef = useRef("");
  const terminalTabsRef = useRef(terminalTabs);
  const broadcastEnabledRef = useRef(broadcastEnabled);
  const broadcastTargetIdsRef = useRef(broadcastTargetIds);
  const macroRecordingRef = useRef<MacroRecordingState | null>(macroRecording);
  const macroCancelRef = useRef(false);
  const macroRunRef = useRef<{ title: string; step: number; total: number; targets: string[] } | null>(null);
  const splitResizeRef = useRef<{ direction: Exclude<SplitDirection, "none">; frame: HTMLElement; path: SplitPath } | null>(null);
  terminalTabsRef.current = terminalTabs;
  broadcastEnabledRef.current = broadcastEnabled;
  broadcastTargetIdsRef.current = broadcastTargetIds;
  macroRecordingRef.current = macroRecording;
  macroRunRef.current = macroRun;

  useEffect(() => {
    const onPointerMove = (event: MouseEvent) => {
      const resize = splitResizeRef.current;
      if (!resize) return;
      const bounds = resize.frame.getBoundingClientRect();
      const position = resize.direction === "right"
        ? event.clientX - bounds.left
        : event.clientY - bounds.top;
      const length = resize.direction === "right" ? bounds.width : bounds.height;
      if (length <= 0) return;
      const ratio = Math.max(20, Math.min(80, (position / length) * 100));
      setTerminalLayout((current) => updateLayoutRatio(current, resize.path, ratio));
    };
    const onPointerUp = () => {
      splitResizeRef.current = null;
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    };
    window.addEventListener("mousemove", onPointerMove);
    window.addEventListener("mouseup", onPointerUp);
    return () => {
      window.removeEventListener("mousemove", onPointerMove);
      window.removeEventListener("mouseup", onPointerUp);
      onPointerUp();
    };
  }, []);

  const activeTerminal = terminalTabs.find((terminal) => terminal.id === activeTerminalId) ?? terminalTabs[0];
  const selectedTerminalId = activeTerminal?.id ?? "";
  selectedTerminalIdRef.current = selectedTerminalId;
  const remoteSessionId = activeTerminal?.remoteSessionId ?? null;
  const remoteProtocol = activeTerminal?.remoteProtocol ?? null;
  const remoteHost = activeTerminal?.remoteHost ?? null;
  const terminalStatus = activeTerminal?.status ?? "closed";

  useEffect(() => {
    if (!activeTerminalId && activeTerminal) setActiveTerminalId(activeTerminal.id);
    if (activeTerminal && terminalLayout.kind === "pane" && !terminalLayout.terminalId) {
      setTerminalLayout({ kind: "pane", terminalId: activeTerminal.id });
    }
  }, [activeTerminal, activeTerminalId, terminalLayout]);

  const startNewTerminal = useCallback(() => {
    const terminal = createWorkspaceTerminal();
    setTerminalTabs((current) => [...current, terminal]);
    setActiveTerminalId(terminal.id);
    setTerminalLayout({ kind: "pane", terminalId: terminal.id });
    setConnectionError(null);
    setSessionNotice(null);
    setActiveView("terminal");
  }, []);

  const openSftpView = useCallback(() => {
    if (remoteSessionId && remoteProtocol === "ssh") {
      setActiveView("files");
      return;
    }
    setConnectionError("Open an SSH session before opening SFTP.");
    setQuickConnectOpen(true);
  }, [remoteProtocol, remoteSessionId]);

  const startMacroRecording = useCallback(() => {
    if (!selectedTerminalId || !activeTerminal || remoteProtocol === "rdp" || remoteProtocol === "vnc") return;
    if (macroRunRef.current) {
      setConnectionError("Stop the running macro before recording terminal input.");
      return;
    }
    if (!nativeTerminalIdsRef.current.has(selectedTerminalId)) {
      setConnectionError("The selected terminal is not ready for recording yet.");
      return;
    }
    const next = { terminalId: selectedTerminalId, terminalLabel: activeTerminal.label, actions: [], textBytes: 0 };
    macroRecordingRef.current = next;
    setMacroRecording(next);
    setRecordedMacroDraft(null);
    setConnectionError(null);
    setSessionNotice(`Recording input from “${activeTerminal.label}”. Stop recording to review it as a macro.`);
  }, [activeTerminal, remoteProtocol, selectedTerminalId]);

  const stopMacroRecording = useCallback(() => {
    const recording = macroRecordingRef.current;
    if (!recording) return;
    macroRecordingRef.current = null;
    setMacroRecording(null);
    if (recording.actions.length === 0) {
      setSessionNotice("Macro recording stopped without captured input.");
      return;
    }
    const draft: MacroRecord = {
      id: crypto.randomUUID(),
      title: `Recorded · ${recording.terminalLabel}`,
      description: "Captured terminal input. Review every action before saving or running.",
      tags: ["recorded"],
      actions: recording.actions,
      approval: "eachAction",
    };
    setRecordedMacroDraft(draft);
    setMacrosOpen(true);
    setSessionNotice(`Captured ${recording.actions.length} bounded macro action${recording.actions.length === 1 ? "" : "s"}. Review before saving.`);
  }, []);

  const handleTerminalStatus = useCallback((workspaceId: string, status: TerminalStatus) => {
    setTerminalTabs((current) => current.map((terminal) => terminal.id === workspaceId ? { ...terminal, status } : terminal));
  }, []);

  const handleNativeTerminalId = useCallback((workspaceId: string, terminalId: string | null) => {
    if (terminalId) nativeTerminalIdsRef.current.set(workspaceId, terminalId);
    else nativeTerminalIdsRef.current.delete(workspaceId);
  }, []);

  const handleTerminalReady = useCallback((workspaceId: string, terminal: Terminal, searchAddon: SearchAddon) => {
    terminalInstancesRef.current.set(workspaceId, { terminal, searchAddon });
  }, []);

  const handleTerminalDisposed = useCallback((workspaceId: string) => {
    terminalInstancesRef.current.delete(workspaceId);
  }, []);

  const handleSearchResults = useCallback((workspaceId: string, resultIndex: number, resultCount: number) => {
    if (workspaceId === selectedTerminalIdRef.current) setTerminalSearchResult({ resultIndex, resultCount });
  }, []);

  const writeTerminalInput = useCallback((workspaceId: string, terminalId: string, data: string) => {
    const terminal = terminalTabsRef.current.find((item) => item.id === workspaceId);
    if (!terminal) return Promise.reject(new Error("terminal target no longer exists"));
    if (terminal.remoteProtocol === "rdp" || terminal.remoteProtocol === "vnc") {
      return Promise.reject(new Error("broadcast text input is not available for remote desktop sessions"));
    }
    const command = terminal.remoteProtocol === "ssh" ? "ssh_write" : terminal.remoteProtocol === "telnet" ? "telnet_write" : terminal.remoteProtocol === "serial" ? "serial_write" : "terminal_write";
    return invoke(command, { terminalId, data });
  }, []);

  const recordTerminalInput = useCallback((workspaceId: string, data: string) => {
    const recording = macroRecordingRef.current;
    if (!recording || recording.terminalId !== workspaceId) return;
    const actions = recordedMacroActions(data);
    if (actions.length === 0) return;
    const textBytes = recording.textBytes + new TextEncoder().encode(data).length;
    const mergedActions = [...recording.actions];
    for (const action of actions) {
      const previous = mergedActions.at(-1);
      if (previous?.kind === "sendText" && action.kind === "sendText") previous.text += action.text;
      else mergedActions.push(action);
    }
    if (mergedActions.length > MAX_RECORDED_MACRO_ACTIONS || textBytes > MAX_RECORDED_MACRO_TEXT_BYTES) {
      macroRecordingRef.current = null;
      setMacroRecording(null);
      setConnectionError("Macro recording stopped at its safe 64-action/64 KiB limit. Review the captured draft before saving.");
      return;
    }
    const next = { ...recording, actions: mergedActions, textBytes };
    macroRecordingRef.current = next;
    setMacroRecording(next);
  }, []);

  const handleTerminalInput = useCallback((workspaceId: string, terminalId: string, data: string) => {
    const selectedTargets = broadcastEnabledRef.current ? broadcastTargetIdsRef.current : [workspaceId];
    const targetIds = [...new Set(selectedTargets)];
    if (targetIds.length === 0) {
      setConnectionError("Broadcast is active but no target terminal is selected. Input was not sent.");
      return;
    }
    const targets = targetIds.map((targetId) => ({
      workspaceId: targetId,
      nativeId: targetId === workspaceId ? terminalId : nativeTerminalIdsRef.current.get(targetId),
    }));
    const unavailable = targets.filter((target) => !target.nativeId);
    if (unavailable.length > 0) {
      setConnectionError(`Input was not sent: ${unavailable.length} selected terminal${unavailable.length === 1 ? " is" : "s are"} not ready.`);
      return;
    }
    recordTerminalInput(workspaceId, data);
    void Promise.all(targets.map((target) => writeTerminalInput(target.workspaceId, target.nativeId!, data)))
      .then(() => setConnectionError(null))
      .catch((error) => setConnectionError(`Terminal input failed: ${String(error)}`));
  }, [recordTerminalInput, writeTerminalInput]);

  const findTerminalMatch = useCallback((direction: "next" | "previous", query = terminalSearchQuery) => {
    const instance = terminalInstancesRef.current.get(selectedTerminalIdRef.current);
    const term = query.trim();
    if (!instance || !term) {
      instance?.searchAddon.clearDecorations();
      setTerminalSearchResult({ resultIndex: -1, resultCount: 0 });
      return;
    }
    const options = {
      caseSensitive: terminalSearchCaseSensitive,
      decorations: {
        matchBackground: "#3b5148",
        matchBorder: "#7b987e",
        matchOverviewRuler: "#7b987e",
        activeMatchBackground: "#e8b45c",
        activeMatchBorder: "#f5d28f",
        activeMatchColorOverviewRuler: "#e8b45c",
      },
    };
    if (direction === "previous") instance.searchAddon.findPrevious(term, options);
    else instance.searchAddon.findNext(term, options);
  }, [terminalSearchCaseSensitive, terminalSearchQuery]);

  const updateTerminalSearch = useCallback((query: string) => {
    setTerminalSearchQuery(query);
    findTerminalMatch("next", query);
  }, [findTerminalMatch]);

  const closeTerminalSearch = useCallback(() => {
    terminalInstancesRef.current.get(selectedTerminalIdRef.current)?.searchAddon.clearDecorations();
    setTerminalSearchOpen(false);
    setTerminalSearchResult({ resultIndex: -1, resultCount: 0 });
  }, []);

  const copyTerminalSelection = useCallback(async () => {
    const selection = terminalInstancesRef.current.get(selectedTerminalIdRef.current)?.terminal.getSelection() ?? "";
    if (!selection) {
      setConnectionError("Select terminal text before copying.");
      return;
    }
    try {
      if (navigator.clipboard?.writeText) await navigator.clipboard.writeText(selection);
      else window.prompt("Copy this terminal selection", selection);
      setConnectionError(null);
      setSessionNotice("Terminal selection copied explicitly.");
    } catch (error) {
      setConnectionError(`Terminal selection could not be copied: ${String(error)}`);
    }
  }, []);

  const clearTerminalScrollback = useCallback(() => {
    const instance = terminalInstancesRef.current.get(selectedTerminalIdRef.current);
    if (!instance) return;
    instance.searchAddon.clearDecorations();
    instance.terminal.clearSelection();
    instance.terminal.clear();
    setTerminalSearchResult({ resultIndex: -1, resultCount: 0 });
    setSessionNotice("Cleared the active terminal scrollback. The process remains connected.");
  }, []);

  useEffect(() => {
    setTerminalSearchResult({ resultIndex: -1, resultCount: 0 });
    if (terminalSearchOpen && terminalSearchQuery.trim()) findTerminalMatch("next");
  }, [findTerminalMatch, selectedTerminalId, terminalSearchOpen, terminalSearchQuery]);

  const closeTerminal = useCallback((workspaceId: string) => {
    nativeTerminalIdsRef.current.delete(workspaceId);
    setBroadcastTargetIds((current) => current.filter((id) => id !== workspaceId));
    if (terminalTabs.length === 1) {
      const replacement = createWorkspaceTerminal();
      setTerminalTabs([replacement]);
      setActiveTerminalId(replacement.id);
      setTerminalLayout({ kind: "pane", terminalId: replacement.id });
      setActiveView("terminal");
      return;
    }
    const closingIndex = terminalTabs.findIndex((terminal) => terminal.id === workspaceId);
    const remaining = terminalTabs.filter((terminal) => terminal.id !== workspaceId);
    setTerminalTabs(remaining);
    setTerminalLayout((current) => removeLayoutNode(current, workspaceId) ?? { kind: "pane", terminalId: remaining[0].id });
    if (activeTerminalId === workspaceId) {
      setActiveTerminalId(remaining[Math.min(closingIndex, remaining.length - 1)].id);
    }
  }, [activeTerminalId, terminalTabs]);

  const cycleTerminal = useCallback((direction: 1 | -1) => {
    if (terminalTabs.length < 2) return;
    const index = Math.max(0, terminalTabs.findIndex((terminal) => terminal.id === selectedTerminalId));
    const nextIndex = (index + direction + terminalTabs.length) % terminalTabs.length;
    setActiveTerminalId(terminalTabs[nextIndex].id);
    setTerminalLayout({ kind: "pane", terminalId: terminalTabs[nextIndex].id });
    setActiveView("terminal");
  }, [selectedTerminalId, terminalTabs]);

  const openSplit = useCallback((direction: Exclude<SplitDirection, "none">) => {
    const firstId = selectedTerminalId;
    if (!firstId) return;
    const visibleIds = new Set(layoutTerminalIds(terminalLayout));
    let second = terminalTabs.find((terminal) => terminal.id !== firstId && !visibleIds.has(terminal.id));
    if (!second) {
      second = createWorkspaceTerminal();
      setTerminalTabs((current) => [...current, second!]);
    }
    const newSplit: TerminalLayoutNode = { kind: "split", direction, ratio: 50, first: { kind: "pane", terminalId: firstId }, second: { kind: "pane", terminalId: second.id } };
    setTerminalLayout((current) => {
      const path = findLayoutPath(current, firstId);
      return path ? replaceLayoutNode(current, path, newSplit) : newSplit;
    });
    setActiveView("terminal");
  }, [selectedTerminalId, terminalLayout, terminalTabs]);

  const beginSplitResize = useCallback((event: ReactMouseEvent<HTMLDivElement>, path: SplitPath, direction: Exclude<SplitDirection, "none">) => {
    const frame = event.currentTarget.parentElement;
    if (!frame) return;
    event.preventDefault();
    splitResizeRef.current = { direction, frame, path };
    document.body.style.cursor = direction === "right" ? "col-resize" : "row-resize";
    document.body.style.userSelect = "none";
  }, []);

  const adjustSplitRatio = useCallback((event: ReactKeyboardEvent<HTMLDivElement>, path: SplitPath, direction: Exclude<SplitDirection, "none">) => {
    const increase = direction === "right"
      ? event.key === "ArrowRight"
      : event.key === "ArrowDown";
    const decrease = direction === "right"
      ? event.key === "ArrowLeft"
      : event.key === "ArrowUp";
    if (!increase && !decrease) return;
    event.preventDefault();
    setTerminalLayout((current) => {
      const currentNode = path.reduce<TerminalLayoutNode | null>((node, part) => node?.kind === "split" ? node[part] : null, current);
      const ratio = currentNode?.kind === "split" ? currentNode.ratio : 50;
      return updateLayoutRatio(current, path, Math.max(20, Math.min(80, ratio + (increase ? 5 : -5))));
    });
  }, []);

  const resetSplitRatio = useCallback((path: SplitPath) => {
    setTerminalLayout((current) => updateLayoutRatio(current, path, 50));
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

  const refreshPortableVaultStatus = useCallback(() => {
    if (!IS_TAURI) return;
    void invoke<PortableVaultStatus>("portable_vault_status")
      .then(setPortableVaultStatus)
      .catch((error) => setConnectionError(`Portable vault status could not be loaded: ${String(error)}`));
  }, []);

  const refreshSnippets = useCallback(() => {
    if (!IS_TAURI) return;
    void invoke<SnippetRecord[]>("snippet_list")
      .then(setSnippets)
      .catch((error) => setConnectionError(`Snippets could not be loaded: ${String(error)}`));
  }, []);

  const refreshMacros = useCallback(() => {
    if (!IS_TAURI) return;
    void invoke<MacroRecord[]>("macro_list")
      .then((records) => setMacros(records.map(normalizeMacroRecord)))
      .catch((error) => setConnectionError(`Macros could not be loaded: ${String(error)}`));
  }, []);

  const saveSnippet = useCallback(async (snippet: SnippetRecord) => {
    try {
      const saved = IS_TAURI ? await invoke<SnippetRecord>("snippet_save", { snippet }) : snippet;
      setSnippets((current) => current.some((item) => item.id === saved.id) ? current.map((item) => item.id === saved.id ? saved : item) : [...current, saved]);
      setSessionNotice(`Saved snippet “${saved.title}”. It is never executed automatically.`);
      setConnectionError(null);
    } catch (error) {
      setConnectionError(`Snippet could not be saved: ${String(error)}`);
    }
  }, []);

  const deleteSnippet = useCallback(async (snippet: SnippetRecord) => {
    if (!window.confirm(`Delete snippet “${snippet.title}”?`)) return;
    try {
      if (IS_TAURI) await invoke<boolean>("snippet_delete", { snippetId: snippet.id });
      setSnippets((current) => current.filter((item) => item.id !== snippet.id));
      setSessionNotice(`Deleted snippet “${snippet.title}”.`);
      setConnectionError(null);
    } catch (error) {
      setConnectionError(`Snippet could not be deleted: ${String(error)}`);
    }
  }, []);

  const copySnippet = useCallback(async (command: string) => {
    try {
      if (navigator.clipboard?.writeText) await navigator.clipboard.writeText(command);
      else window.prompt("Copy this rendered snippet and paste it manually", command);
      setSessionNotice("Rendered snippet copied. Review it, then paste manually; MobaRust does not auto-send it.");
      setConnectionError(null);
    } catch (error) {
      setConnectionError(`Snippet could not be copied: ${String(error)}`);
    }
  }, []);

  const saveMacro = useCallback(async (record: MacroRecord) => {
    try {
      const saved = normalizeMacroRecord(IS_TAURI ? await invoke<MacroRecord>("macro_save", { record }) : record);
      setMacros((current) => current.some((item) => item.id === saved.id) ? current.map((item) => item.id === saved.id ? saved : item) : [...current, saved]);
      setSessionNotice(`Saved macro “${saved.title}”. ${saved.approval === "eachAction" ? "Each action requires approval." : "It requires an explicit run confirmation."}`);
      setConnectionError(null);
    } catch (error) {
      setConnectionError(`Macro could not be saved: ${String(error)}`);
    }
  }, []);

  const deleteMacro = useCallback(async (record: MacroRecord) => {
    if (!window.confirm(`Delete macro “${record.title}”?`)) return;
    try {
      if (IS_TAURI) await invoke<boolean>("macro_delete", { macroId: record.id });
      setMacros((current) => current.filter((item) => item.id !== record.id));
      setSessionNotice(`Deleted macro “${record.title}”.`);
      setConnectionError(null);
    } catch (error) {
      setConnectionError(`Macro could not be deleted: ${String(error)}`);
    }
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

  const createPortableVault = useCallback(async (passphrase: string) => {
    if (!IS_TAURI) return;
    try {
      const status = await invoke<PortableVaultStatus>("portable_vault_create", { payload: { passphrase } });
      setPortableVaultStatus(status);
      setSessionNotice("Created and unlocked the encrypted portable vault. Lock it before leaving the computer unattended.");
      setConnectionError(null);
    } catch (error) {
      setConnectionError(`Portable vault could not be created: ${String(error)}`);
    }
  }, []);

  const unlockPortableVault = useCallback(async (passphrase: string) => {
    if (!IS_TAURI) return;
    try {
      const status = await invoke<PortableVaultStatus>("portable_vault_unlock", { payload: { passphrase } });
      setPortableVaultStatus(status);
      setSessionNotice("Portable vault unlocked in native memory. The passphrase was not returned to the interface.");
      setConnectionError(null);
    } catch (error) {
      setConnectionError(`Portable vault could not be unlocked: ${String(error)}`);
    }
  }, []);

  const lockPortableVault = useCallback(async () => {
    if (!IS_TAURI) return;
    try {
      const status = await invoke<PortableVaultStatus>("portable_vault_lock");
      setPortableVaultStatus(status);
      setSessionNotice("Portable vault locked and its native key material was released.");
      setConnectionError(null);
    } catch (error) {
      setConnectionError(`Portable vault could not be locked: ${String(error)}`);
    }
  }, []);

  const saveCredential = useCallback(async (credentialId: string, secret: string) => {
    if (!IS_TAURI) {
      setConnectionError("Credential vault operations require the desktop runtime.");
      return;
    }
    try {
      const savedId = await invoke<string>("vault_put", {
        payload: { credentialId: credentialId.trim(), secret },
      });
      setCredentialsOpen(false);
      setSessionNotice(`Saved credential reference “${savedId}”. The secret remains in the native vault.`);
      setConnectionError(null);
    } catch (error) {
      setConnectionError(`Credential could not be saved: ${String(error)}`);
    }
  }, []);

  const deleteCredential = useCallback(async (credentialId: string) => {
    if (!IS_TAURI) {
      setConnectionError("Credential vault operations require the desktop runtime.");
      return;
    }
    try {
      const deletedId = await invoke<string>("vault_delete", {
        payload: { credentialId: credentialId.trim() },
      });
      setCredentialsOpen(false);
      setSessionNotice(`Deleted credential reference “${deletedId}”.`);
      setConnectionError(null);
    } catch (error) {
      setConnectionError(`Credential could not be deleted: ${String(error)}`);
    }
  }, []);

  const savePortableCredential = useCallback(async (credentialId: string, secret: string) => {
    if (!IS_TAURI) {
      setConnectionError("Credential vault operations require the desktop runtime.");
      return;
    }
    try {
      const savedId = await invoke<string>("portable_vault_put", {
        payload: { credentialId: credentialId.trim(), secret },
      });
      setCredentialsOpen(false);
      setSessionNotice(`Saved credential reference “${savedId}” in the encrypted portable vault.`);
      setConnectionError(null);
    } catch (error) {
      setConnectionError(`Portable credential could not be saved: ${String(error)}`);
    }
  }, []);

  const deletePortableCredential = useCallback(async (credentialId: string) => {
    if (!IS_TAURI) {
      setConnectionError("Credential vault operations require the desktop runtime.");
      return;
    }
    try {
      const deletedId = await invoke<string>("portable_vault_delete", {
        payload: { credentialId: credentialId.trim() },
      });
      setCredentialsOpen(false);
      setSessionNotice(`Deleted credential reference “${deletedId}” from the encrypted portable vault.`);
      setConnectionError(null);
    } catch (error) {
      setConnectionError(`Portable credential could not be deleted: ${String(error)}`);
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
      const terminal = createWorkspaceTerminal({
        label: `${request.username}@${response.host}`,
        remoteSessionId: response.terminalId,
        remoteProtocol: "ssh",
        remoteHost: response.host,
      });
      setTerminalTabs((current) => [...current, terminal]);
      setActiveTerminalId(terminal.id);
      setTerminalLayout({ kind: "pane", terminalId: terminal.id });
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
      const terminal = createWorkspaceTerminal({
        label: `Telnet · ${response.host}`,
        remoteSessionId: response.terminalId,
        remoteProtocol: "telnet",
        remoteHost: response.host,
      });
      setTerminalTabs((current) => [...current, terminal]);
      setActiveTerminalId(terminal.id);
      setTerminalLayout({ kind: "pane", terminalId: terminal.id });
      setActiveView("terminal");
      setQuickConnectOpen(false);
      setSessionNotice("Connected over Telnet. This connection is unencrypted.");
    } catch (error) {
      setConnectionError(String(error));
    }
  }, []);

  const connectSerial = useCallback(async (request: SerialConnectRequest, offerSave = true) => {
    setConnectionError(null);
    setSessionNotice(null);
    if (!IS_TAURI) {
      setConnectionError("Serial connections require the desktop runtime.");
      return;
    }
    try {
      const response = await invoke<SerialConnectResponse>("serial_connect", { request });
      const terminal = createWorkspaceTerminal({
        label: `Serial · ${response.device}`,
        remoteSessionId: response.terminalId,
        remoteProtocol: "serial",
        remoteHost: response.device,
      });
      setTerminalTabs((current) => [...current, terminal]);
      setActiveTerminalId(terminal.id);
      setTerminalLayout({ kind: "pane", terminalId: terminal.id });
      setActiveView("terminal");
      setQuickConnectOpen(false);
      setSessionNotice(`Connected to ${response.device}. Serial traffic is not encrypted by MobaRust.`);
      if (offerSave) {
        const suggestedName = `${response.device} · ${request.baudRate}`;
        const name = window.prompt("Save this serial profile as", suggestedName);
        if (name?.trim()) {
          try {
            await invoke("session_save_serial", { payload: { name: name.trim(), request } });
            refreshSavedSessions();
          } catch (error) {
            setConnectionError(`Connected, but the serial profile could not be saved: ${String(error)}`);
          }
        }
      }
    } catch (error) {
      setConnectionError(String(error));
    }
  }, [refreshSavedSessions]);

  const connectRemoteDesktop = useCallback((request: RemoteDesktopConnectRequest, offerSave = true) => {
    setConnectionError(null);
    setSessionNotice(null);
    const terminal = createWorkspaceTerminal({
      label: `${request.protocol.toUpperCase()} · ${request.host}`,
      remoteProtocol: request.protocol,
      remoteHost: request.host,
      remoteDesktopRequest: request,
    });
    setTerminalTabs((current) => [...current, terminal]);
    setActiveTerminalId(terminal.id);
    setTerminalLayout({ kind: "pane", terminalId: terminal.id });
    setActiveView("terminal");
    setQuickConnectOpen(false);
    setSessionNotice(`${request.protocol.toUpperCase()} session queued. The native helper will report its actual connection state.`);
    if (offerSave && IS_TAURI) {
      const suggestedName = `${request.username ? `${request.username}@` : ""}${request.host}`;
      const name = window.prompt(`Save this ${request.protocol.toUpperCase()} session as`, suggestedName);
      if (name?.trim()) {
        void invoke("session_save_remote_desktop", { payload: { name: name.trim(), request } })
          .then(() => {
            refreshSavedSessions();
            setSessionNotice(`Queued ${request.protocol.toUpperCase()} session and saved ${name.trim()}.`);
          })
          .catch((error) => setConnectionError(`Queued, but the session could not be saved: ${String(error)}`));
      }
    }
  }, [refreshSavedSessions]);

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

  const touchSavedSession = useCallback((sessionId: string) => {
    if (!IS_TAURI) return;
    void invoke<boolean>("session_touch", { sessionId })
      .then((touched) => {
        if (touched) refreshSavedSessions();
      })
      .catch(() => undefined);
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
    touchSavedSession(session.id);
    if (session.protocol === "SERIAL") {
      const profile = session.serial_profile;
      if (!profile) {
        setConnectionError("This serial profile has no saved device parameters.");
        return;
      }
      void connectSerial({
        device: profile.device,
        baudRate: profile.baud_rate,
        dataBits: profile.data_bits,
        stopBits: profile.stop_bits,
        parity: profile.parity,
        flowControl: profile.flow_control,
        lineEnding: profile.line_ending,
      }, false);
      return;
    }
    if (session.protocol === "RDP" || session.protocol === "VNC") {
      const profile = session.remote_desktop_profile;
      if (!profile || session.port === 0) {
        setConnectionError("This remote desktop profile has no saved display parameters.");
        return;
      }
      const protocol = session.protocol.toLowerCase() as DesktopProtocol;
      const credentialId = session.auth.kind === "password" ? session.auth.credentialRef : undefined;
      if (protocol === "rdp" && (!session.username?.trim() || !credentialId?.trim())) {
        setConnectionError("This RDP profile has no complete username or credential reference.");
        return;
      }
      connectRemoteDesktop({
        protocol,
        host: session.hostname,
        port: session.port,
        username: session.username ?? "",
        domain: profile.domain ?? undefined,
        credentialId,
        width: profile.width,
        height: profile.height,
        colorDepth: profile.color_depth,
        audioEnabled: profile.audio_enabled,
      }, false);
      return;
    }
    const request = requestFromSavedSession(session, savedSessions);
    if (!request) {
      setConnectionError("This saved session uses an authentication method that is not available yet.");
      return;
    }
    void connectSsh(request, false);
  }, [connectRemoteDesktop, connectSerial, connectSsh, savedSessions, touchSavedSession]);

  const writeToExplicitTargets = useCallback(async (targetIds: string[], data: string) => {
    const targets = [...new Set(targetIds)].map((workspaceId) => ({
      workspaceId,
      nativeId: nativeTerminalIdsRef.current.get(workspaceId),
    }));
    const unavailable = targets.filter((target) => !target.nativeId);
    if (targets.length === 0 || unavailable.length > 0) {
      throw new Error("one or more selected terminals are not ready");
    }
    await Promise.all(targets.map((target) => writeTerminalInput(target.workspaceId, target.nativeId!, data)));
  }, [writeTerminalInput]);

  const runMacro = useCallback(async (record: MacroRecord, targetIds: string[]) => {
    if (macroRunRef.current) {
      setConnectionError("A macro is already running. Cancel it before starting another one.");
      return;
    }
    const targets = [...new Set(targetIds)];
    const targetLabels = targets.map((id) => terminalTabsRef.current.find((terminal) => terminal.id === id)?.label ?? id);
    if (targets.length === 0) {
      setConnectionError("Select at least one ready terminal before running a macro.");
      return;
    }
    if (targets.some((id) => !nativeTerminalIdsRef.current.has(id))) {
      setConnectionError("The macro was not started because every selected terminal must be ready.");
      return;
    }
    const warning = record.actions.some((action) => action.kind === "executeCommand" || action.kind === "openSession" || action.kind === "switchWorkspace")
      ? `Macro “${record.title}” includes command or session-control actions. Run it on ${targetLabels.join(", ")}?`
      : `Run macro “${record.title}” on ${targetLabels.join(", ")}?`;
    const approvalNote = record.approval === "eachAction" ? "Every action will ask for approval before it runs." : "Execution is visible and can be cancelled.";
    if (!window.confirm(`${warning}\n\n${approvalNote} Do not include passwords or tokens in macro text.`)) return;

    const keyData: Record<MacroKey, string> = {
      enter: "\r",
      escape: "\x1b",
      tab: "\t",
      backspace: "\x7f",
      ctrlC: "\x03",
      ctrlD: "\x04",
      arrowUp: "\x1b[A",
      arrowDown: "\x1b[B",
      arrowLeft: "\x1b[D",
      arrowRight: "\x1b[C",
    };
    const wait = async (milliseconds: number) => {
      let remaining = milliseconds;
      while (remaining > 0) {
        if (macroCancelRef.current) throw new Error("cancelled");
        const slice = Math.min(remaining, 50);
        await new Promise<void>((resolve) => window.setTimeout(resolve, slice));
        remaining -= slice;
      }
    };

    macroCancelRef.current = false;
    setMacrosOpen(false);
    setRecordedMacroDraft(null);
    setActiveView("terminal");
    const runState = { title: record.title, step: 0, total: record.actions.length, targets: targetLabels };
    macroRunRef.current = runState;
    setMacroRun(runState);
    try {
      for (const [index, action] of record.actions.entries()) {
        if (macroCancelRef.current) throw new Error("cancelled");
        if (record.approval === "eachAction") {
          const actionDescription = action.kind === "executeCommand"
            ? "execute a saved command (review its text in the editor)"
            : action.kind === "sendText"
              ? "send a saved text payload"
              : action.kind === "sendKey"
                ? `send the ${action.key} key`
                : action.kind === "wait"
                  ? `wait ${action.milliseconds} ms`
                  : action.kind === "openSession"
                    ? "open a saved session"
                    : "switch workspace";
          if (!window.confirm(`Approve macro action ${index + 1}/${record.actions.length}: ${actionDescription}?\n\nNo action will run if you cancel.`)) {
            throw new Error("approval declined");
          }
        }
        const nextRunState = { title: record.title, step: index + 1, total: record.actions.length, targets: targetLabels };
        macroRunRef.current = nextRunState;
        setMacroRun(nextRunState);
        if (action.kind === "sendText") await writeToExplicitTargets(targets, action.text);
        if (action.kind === "executeCommand") await writeToExplicitTargets(targets, `${action.command}\r`);
        if (action.kind === "sendKey") await writeToExplicitTargets(targets, keyData[action.key]);
        if (action.kind === "wait") await wait(action.milliseconds);
        if (action.kind === "switchWorkspace") {
          if (!terminalTabsRef.current.some((terminal) => terminal.id === action.workspaceId)) throw new Error("workspace target no longer exists");
          setActiveTerminalId(action.workspaceId);
          setActiveView("terminal");
        }
        if (action.kind === "openSession") {
          const session = savedSessions.find((item) => item.id === action.sessionId);
          if (!session) throw new Error("saved session target no longer exists");
          connectSavedSession(session);
          await wait(100);
        }
      }
      macroRunRef.current = null;
      setMacroRun(null);
      setSessionNotice(`Macro “${record.title}” completed on ${targetLabels.length} terminal${targetLabels.length === 1 ? "" : "s"}.`);
      setConnectionError(null);
    } catch (error) {
      macroRunRef.current = null;
      setMacroRun(null);
      if (macroCancelRef.current || String(error).includes("cancelled")) {
        setSessionNotice(`Macro “${record.title}” cancelled before completion.`);
        setConnectionError(null);
      } else if (String(error).includes("approval declined")) {
        setSessionNotice(`Macro “${record.title}” stopped before the next unapproved action.`);
        setConnectionError(null);
      } else {
        setConnectionError(`Macro “${record.title}” stopped: ${String(error)}`);
      }
    }
  }, [connectSavedSession, savedSessions, writeToExplicitTargets]);

  const cancelMacro = useCallback(() => {
    if (!macroRun) return;
    macroCancelRef.current = true;
    setSessionNotice(`Cancellation requested for macro “${macroRun.title}”.`);
  }, [macroRun]);

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

  const collectRemoteMonitor = useCallback(async () => {
    if (!remoteSessionId || remoteProtocol !== "ssh") return;
    if (!IS_TAURI) {
      setRemoteMonitorStatus("error");
      setRemoteMonitorError("Remote monitoring requires the desktop runtime.");
      return;
    }
    setRemoteMonitorStatus("loading");
    setRemoteMonitorError(null);
    try {
      const snapshot = await invoke<RemoteMonitorSnapshot>("ssh_collect_remote_monitor", { terminalId: remoteSessionId });
      setRemoteMonitor(snapshot);
      setRemoteMonitorStatus("ready");
    } catch (error) {
      setRemoteMonitorStatus("error");
      setRemoteMonitorError(String(error));
    }
  }, [remoteProtocol, remoteSessionId]);

  const openRemoteTextFile = useCallback(async (entry: RemoteEntry) => {
    if (!remoteSessionId || entry.isDirectory) return;
    try {
      const document = await invoke<RemoteTextDocument>("ssh_open_remote_text_file", {
        terminalId: remoteSessionId,
        path: entry.path,
      });
      setEditingRemoteFile(document);
      setConnectionError(null);
    } catch (error) {
      setConnectionError(`Remote file could not be opened: ${String(error)}`);
    }
  }, [remoteSessionId]);

  const saveRemoteTextFile = useCallback(async (content: string, encoding: RemoteTextDocument["encoding"]) => {
    if (!remoteSessionId || !editingRemoteFile) return;
    const saved = await invoke<RemoteTextDocument>("ssh_save_remote_text_file", {
      terminalId: remoteSessionId,
      path: editingRemoteFile.path,
      expectedRevision: editingRemoteFile.revision,
      content,
      encoding,
    });
    setEditingRemoteFile(saved);
    setConnectionError(null);
    setSessionNotice(`Saved ${saved.path}. Remote changes were checked before temporary-file promotion.`);
    void loadRemoteDirectory(remotePath);
  }, [editingRemoteFile, loadRemoteDirectory, remotePath, remoteSessionId]);

  const saveRemoteTextFileAs = useCallback(async (path: string, content: string, encoding: RemoteTextDocument["encoding"], overwrite: boolean) => {
    if (!remoteSessionId || !editingRemoteFile) return;
    const saved = await invoke<RemoteTextDocument>("ssh_save_remote_text_file_as", {
      terminalId: remoteSessionId,
      path,
      content,
      encoding,
      overwrite,
    });
    setEditingRemoteFile(saved);
    setConnectionError(null);
    setSessionNotice(`Saved a new remote file at ${saved.path}.`);
    void loadRemoteDirectory(remotePath);
  }, [editingRemoteFile, loadRemoteDirectory, remotePath, remoteSessionId]);

  const startDownload = useCallback(async (entry: RemoteEntry, protocol: TransferProtocol) => {
    if (!remoteSessionId) return;
    const localPath = window.prompt(entry.isDirectory ? "Local destination directory" : "Local destination path", entry.name);
    if (!localPath?.trim()) return;
    const overwrite = window.confirm(entry.isDirectory ? "Allow replacing existing files inside this directory?" : "Allow replacing an existing local file?");
    try {
      await invoke("ssh_download", {
        terminalId: remoteSessionId,
        request: { remotePath: entry.path, localPath: localPath.trim(), protocol, overwrite, recursive: entry.isDirectory },
      });
      setConnectionError(null);
    } catch (error) {
      setConnectionError(String(error));
    }
  }, [remoteSessionId]);

  const startUpload = useCallback(async (protocol: TransferProtocol) => {
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
        request: { remotePath: destination.trim(), localPath: localPath.trim(), protocol, overwrite, recursive: protocol === "sftp" },
      });
      setConnectionError(null);
    } catch (error) {
      setConnectionError(String(error));
    }
  }, [remotePath, remoteSessionId]);

  const retryTransfer = useCallback(async (transfer: SshTransferEvent) => {
    if (!IS_TAURI) {
      setConnectionError("Transfer retry requires the desktop runtime.");
      return;
    }
    const remotePath = transfer.direction === "download" ? transfer.source : transfer.destination;
    const localPath = transfer.direction === "download" ? transfer.destination : transfer.source;
    const overwrite = window.confirm(`Retry this transfer and allow replacing the destination?\n\n${remotePath}`);
    if (!overwrite) return;
    const command = transfer.direction === "download" ? "ssh_download" : "ssh_upload";
    try {
      await invoke(command, {
        terminalId: transfer.terminalId,
        request: {
          remotePath,
          localPath,
          protocol: transfer.protocol,
          overwrite: true,
          recursive: transfer.recursive,
        },
      });
      setConnectionError(null);
      setSessionNotice(`Retry queued for ${remotePath}.`);
    } catch (error) {
      setConnectionError(`Transfer retry failed: ${String(error)}`);
    }
  }, []);

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

  const setRemotePermissions = useCallback(async (entry: RemoteEntry) => {
    if (!remoteSessionId) return;
    const current = entry.permissions == null ? "644" : (entry.permissions & 0o7777).toString(8).padStart(3, "0");
    const value = window.prompt(`Set POSIX mode for ${entry.name} (octal 0000–7777)`, current);
    if (value === null) return;
    const normalized = value.trim();
    if (!/^[0-7]{3,4}$/.test(normalized)) {
      setConnectionError("Permissions must be an octal mode with 3 or 4 digits, for example 640.");
      return;
    }
    if (!window.confirm(`Apply mode ${normalized} to ${entry.path}?`)) return;
    try {
      await invoke("ssh_set_remote_permissions", {
        terminalId: remoteSessionId,
        path: entry.path,
        permissions: Number.parseInt(normalized, 8),
      });
      setConnectionError(null);
      setSessionNotice(`Updated permissions for ${entry.name} to ${normalized}.`);
      await loadRemoteDirectory(remotePath);
    } catch (error) {
      setConnectionError(String(error));
    }
  }, [loadRemoteDirectory, remotePath, remoteSessionId]);

  const copyRemotePath = useCallback(async (entry: RemoteEntry) => {
    try {
      if (navigator.clipboard?.writeText) await navigator.clipboard.writeText(entry.path);
      else window.prompt("Copy remote path", entry.path);
      setSessionNotice(`Copied remote path ${entry.path}.`);
    } catch (error) {
      setConnectionError(`Remote path could not be copied: ${String(error)}`);
    }
  }, []);

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

  const inspectNetworkHostKey = useCallback(async () => {
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
      setNetworkError("SSH fingerprint inspection requires the desktop runtime.");
      setNetworkStatus("error");
      return;
    }
    setNetworkStatus("running");
    setNetworkFingerprint(null);
    setNetworkError(null);
    try {
      const result = await invoke<SshHostKeyInspection>("ssh_inspect_host_key", { request: { host, port, timeoutMs } });
      setNetworkFingerprint(result);
      setNetworkStatus("ready");
    } catch (error) {
      setNetworkFingerprint(null);
      setNetworkStatus("error");
      setNetworkError(String(error));
    }
  }, [networkHost, networkPort, networkTimeout]);

  const startNetworkPing = useCallback(async () => {
    const host = networkHost.trim();
    const timeoutMs = Number(networkTimeout);
    if (!host) {
      setNetworkError("Enter an explicit hostname or IP address.");
      setNetworkDiagnosticStatus("failed");
      return;
    }
    if (!Number.isInteger(timeoutMs) || timeoutMs < 50 || timeoutMs > 60_000) {
      setNetworkError("Timeout must be an integer between 50 and 60000 milliseconds.");
      setNetworkDiagnosticStatus("failed");
      return;
    }
    if (!IS_TAURI) {
      setNetworkError("Network diagnostics require the desktop runtime.");
      setNetworkDiagnosticStatus("failed");
      return;
    }
    if (networkDiagnosticIdRef.current) await invoke("network_diagnostic_cancel", { operationId: networkDiagnosticIdRef.current }).catch(() => undefined);
    setNetworkDiagnosticKind("ping");
    setNetworkDiagnosticStatus("running");
    setNetworkPingResult(null);
    setNetworkError(null);
    try {
      const response = await invoke<{ operationId: string }>("network_ping_start", { request: { host, timeoutMs } });
      networkDiagnosticIdRef.current = response.operationId;
      setNetworkDiagnosticId(response.operationId);
    } catch (error) {
      networkDiagnosticIdRef.current = null;
      setNetworkDiagnosticId(null);
      setNetworkDiagnosticStatus("failed");
      setNetworkError(String(error));
    }
  }, [networkHost, networkTimeout]);

  const startNetworkTraceroute = useCallback(async () => {
    const host = networkHost.trim();
    const timeoutMs = Number(networkTimeout);
    const maxHops = Number(networkTraceMaxHops);
    if (!host) {
      setNetworkError("Enter an explicit hostname or IP address.");
      setNetworkDiagnosticStatus("failed");
      return;
    }
    if (!Number.isInteger(timeoutMs) || timeoutMs < 50 || timeoutMs > 60_000 || !Number.isInteger(maxHops) || maxHops < 1 || maxHops > 32) {
      setNetworkError("Timeout must be 50–60000 ms and max hops must be between 1 and 32.");
      setNetworkDiagnosticStatus("failed");
      return;
    }
    if (!IS_TAURI) {
      setNetworkError("Network diagnostics require the desktop runtime.");
      setNetworkDiagnosticStatus("failed");
      return;
    }
    if (networkDiagnosticIdRef.current) await invoke("network_diagnostic_cancel", { operationId: networkDiagnosticIdRef.current }).catch(() => undefined);
    setNetworkDiagnosticKind("traceroute");
    setNetworkDiagnosticStatus("running");
    setNetworkTracerouteResult(null);
    setNetworkError(null);
    try {
      const response = await invoke<{ operationId: string }>("network_traceroute_start", { request: { host, timeoutMs, maxHops } });
      networkDiagnosticIdRef.current = response.operationId;
      setNetworkDiagnosticId(response.operationId);
    } catch (error) {
      networkDiagnosticIdRef.current = null;
      setNetworkDiagnosticId(null);
      setNetworkDiagnosticStatus("failed");
      setNetworkError(String(error));
    }
  }, [networkHost, networkTimeout, networkTraceMaxHops]);

  const cancelNetworkDiagnostic = useCallback(async () => {
    const operationId = networkDiagnosticIdRef.current ?? networkDiagnosticId;
    if (!operationId || !IS_TAURI) return;
    try {
      await invoke("network_diagnostic_cancel", { operationId });
    } catch (error) {
      setNetworkError(`Diagnostic cancellation failed: ${String(error)}`);
    }
  }, [networkDiagnosticId]);

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
    if (!IS_TAURI) return;
    let disposed = false;
    let unlisten: UnlistenFn | undefined;
    void listen<NetworkDiagnosticEvent>("network://diagnostic", (event) => {
      const payload = event.payload;
      if (networkDiagnosticIdRef.current && payload.operationId !== networkDiagnosticIdRef.current) return;
      if (!networkDiagnosticIdRef.current) {
        networkDiagnosticIdRef.current = payload.operationId;
        setNetworkDiagnosticId(payload.operationId);
      }
      setNetworkDiagnosticKind(payload.kind);
      setNetworkDiagnosticStatus(payload.state);
      if (payload.ping) setNetworkPingResult(payload.ping);
      if (payload.traceroute) setNetworkTracerouteResult(payload.traceroute);
      if (payload.state === "failed") setNetworkError(payload.error ?? "Network diagnostic failed.");
      if (payload.state === "cancelled") setNetworkError(null);
      if (["completed", "cancelled", "failed"].includes(payload.state)) networkDiagnosticIdRef.current = null;
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
    refreshPortableVaultStatus();
  }, [refreshPortableVaultStatus]);

  useEffect(() => {
    refreshSnippets();
  }, [refreshSnippets]);

  useEffect(() => {
    refreshMacros();
  }, [refreshMacros]);

  useEffect(() => {
    if (activeView === "files" && remoteSessionId && remoteProtocol === "ssh") void loadRemoteDirectory(".");
  }, [activeView, loadRemoteDirectory, remoteProtocol, remoteSessionId]);

  useEffect(() => {
    setRemoteMonitor(null);
    setRemoteMonitorStatus("idle");
    setRemoteMonitorError(null);
  }, [remoteSessionId]);

  useEffect(() => {
    if (activeView === "monitor" && remoteSessionId && remoteProtocol === "ssh" && remoteMonitorStatus === "idle") {
      void collectRemoteMonitor();
    }
  }, [activeView, collectRemoteMonitor, remoteMonitorStatus, remoteProtocol, remoteSessionId]);

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
      if (command && event.key.toLowerCase() === "w") {
        event.preventDefault();
        closeTerminal(selectedTerminalId);
      }
      if (event.ctrlKey && event.key === "Tab") {
        event.preventDefault();
        cycleTerminal(event.shiftKey ? -1 : 1);
      }
      if (command && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setQuickConnectOpen(true);
      }
      if (command && event.shiftKey && event.key.toLowerCase() === "p") {
        event.preventDefault();
        setPaletteOpen((open) => !open);
      }
      if (command && event.shiftKey && event.key.toLowerCase() === "m") {
        event.preventDefault();
        setMacrosOpen(true);
      }
      if (event.key === "Escape") {
        if (terminalSearchOpen) closeTerminalSearch();
        setPaletteOpen(false);
        if (macroRun) cancelMacro();
        if (macroRecording) stopMacroRecording();
        if (broadcastEnabled) {
          setBroadcastEnabled(false);
          setBroadcastOpen(false);
          setSessionNotice("Broadcast mode disabled. No further input will fan out.");
        }
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [broadcastEnabled, cancelMacro, closeTerminal, closeTerminalSearch, cycleTerminal, macroRecording, macroRun, selectedTerminalId, startNewTerminal, stopMacroRecording, terminalSearchOpen]);

  const filteredSessions = sessionRows.filter((session) => {
    const matchesSearch = `${session.name} ${session.detail} ${session.type} ${session.tags.join(" ")}`.toLowerCase().includes(search.toLowerCase());
    return matchesSearch && (!favoritesOnly || session.favorite) && (!recentOnly || session.lastUsedAt != null);
  }).sort((first, second) => (second.lastUsedAt ?? 0) - (first.lastUsedAt ?? 0));
  const localSessionCount = filteredSessions.filter((session) => session.type === "LOCAL").length;
  const remoteSessionCount = filteredSessions.filter((session) => session.type !== "LOCAL").length;
  const activeTransferCount = transfers.filter((transfer) => !["completed", "cancelled", "failed"].includes(transfer.state)).length;
  const activeTunnelCount = tunnels.filter((tunnel) => !["stopped", "failed"].includes(tunnel.state)).length;

  const renderTerminalPane = useCallback((terminal: WorkspaceTerminal) => {
    const isDesktop = (terminal.remoteProtocol === "rdp" || terminal.remoteProtocol === "vnc") && terminal.remoteDesktopRequest;
    return isDesktop ? <RemoteDesktopViewport workspaceId={terminal.id} instanceKey={terminal.instanceKey} request={terminal.remoteDesktopRequest!} onStatusChange={handleTerminalStatus} onNativeTerminalId={handleNativeTerminalId} /> : <TerminalViewport workspaceId={terminal.id} instanceKey={terminal.instanceKey} remoteSessionId={terminal.remoteSessionId} remoteProtocol={terminal.remoteProtocol} fontSize={settings.appearance.fontSize} scrollbackLines={settings.terminal.scrollbackLines} cursorBlink={settings.terminal.cursorBlink} confirmMultilinePaste={settings.general.confirmMultilinePaste} onStatusChange={handleTerminalStatus} onNativeTerminalId={handleNativeTerminalId} onInput={handleTerminalInput} onTerminalReady={handleTerminalReady} onTerminalDisposed={handleTerminalDisposed} onSearchResults={handleSearchResults} />;
  }, [handleNativeTerminalId, handleSearchResults, handleTerminalDisposed, handleTerminalInput, handleTerminalReady, handleTerminalStatus, settings.appearance.fontSize, settings.general.confirmMultilinePaste, settings.terminal.cursorBlink, settings.terminal.scrollbackLines]);

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
          <button className="icon-button" aria-label="Help" title="Help" onClick={() => setHelpOpen(true)}>
            <CircleHelp size={17} strokeWidth={1.7} />
          </button>
          <button className="icon-button" aria-label="Settings" title="Settings" onClick={() => setSettingsOpen(true)}>
            <Settings2 size={17} strokeWidth={1.7} />
          </button>
          <button className="icon-button" aria-label="Credential vault" title="Credential vault" onClick={() => setCredentialsOpen(true)}>
            <KeyRound size={17} strokeWidth={1.7} />
          </button>
          <button className="icon-button" aria-label="Snippets" title="Snippets" onClick={() => setSnippetsOpen(true)}>
            <BookOpen size={17} strokeWidth={1.7} />
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
            <button className={`nav-item ${!favoritesOnly && !recentOnly ? "active" : ""}`} onClick={() => { setFavoritesOnly(false); setRecentOnly(false); }}><LayoutDashboard size={15} /> Overview <span className="nav-count">{sessionRows.length}</span></button>
            <button className={`nav-item ${favoritesOnly && !recentOnly ? "active" : ""}`} onClick={() => { setFavoritesOnly(true); setRecentOnly(false); }}><Star size={15} /> Favorites <span className="nav-count">{sessionRows.filter((session) => session.favorite).length}</span></button>
            <button className={`nav-item ${recentOnly ? "active" : ""}`} onClick={() => { setFavoritesOnly(false); setRecentOnly(true); }}><Activity size={15} /> Recent <span className="nav-count">{sessionRows.filter((session) => session.lastUsedAt != null).length}</span></button>
          </nav>

          <div className="session-list">
            <div className="list-heading"><span>{recentOnly ? "Recent sessions" : favoritesOnly ? "Favorite sessions" : "Sessions"}</span><span className="list-actions"><button aria-label="Import OpenSSH config" title="Import OpenSSH config" onClick={importOpenSshConfig}><Upload size={14} /></button><button aria-label="Import MobaRust session export" title="Import MobaRust session export" onClick={importSessions}><ArrowDownToLine size={14} /></button><button aria-label="Export MobaRust sessions" title="Export secret-free session definitions" onClick={exportSessions}><ArrowUpFromLine size={14} /></button></span></div>
            <div className="folder-heading"><ChevronDown size={13} /> Local terminals <span>{localSessionCount}</span></div>
            {filteredSessions.filter((session) => session.type === "LOCAL").map((session) => (
              <SessionRow key={session.id ?? session.name} {...session} onSelect={startNewTerminal} onToggleFavorite={() => void toggleFavorite(session)} />
            ))}
            <div className="folder-heading muted-folder"><ChevronDown size={13} /> Remote sessions <span>{remoteSessionCount}</span></div>
            {groupSessionsByFolder(filteredSessions.filter((session) => session.type === "SSH" || session.type === "SERIAL" || session.type === "RDP" || session.type === "VNC")).map(([folder, sessions]) => (
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
            <button className={`nav-item ${activeView === "monitor" ? "active" : ""}`} onClick={() => setActiveView("monitor")} disabled={!remoteSessionId || remoteProtocol !== "ssh"} title={remoteSessionId && remoteProtocol === "ssh" ? "Collect a one-shot SSH system snapshot" : "Open an SSH session first"}><Gauge size={15} /> Remote monitor</button>
            <button className={`nav-item ${activeView === "transfers" ? "active" : ""}`} onClick={() => setActiveView("transfers")}><ArrowDownToLine size={15} /> Transfers <span className="nav-count">{activeTransferCount}</span></button>
          </div>
        </aside>

        <section className="workspace">
          {!sidebarOpen && <button className="floating-sidebar-button" onClick={() => setSidebarOpen(true)} aria-label="Expand sidebar"><PanelLeftClose size={16} /></button>}
          <div className="workspace-heading">
            <div>
              <div className="eyebrow"><span>WORKSPACE / 01</span><span className="eyebrow-slash">/</span><span className="muted">{remoteProtocol ? remoteProtocol.toUpperCase() : "LOCAL"}</span></div>
              <h1>{remoteHost ?? "Local workstation"}</h1>
              <p className="workspace-subtitle">{remoteProtocol === "ssh" ? "Interactive SSH shell with native host-key verification." : remoteProtocol === "telnet" ? "Legacy Telnet terminal. Traffic is unencrypted." : remoteProtocol === "serial" ? "Serial terminal with explicit device parameters." : remoteProtocol === "rdp" ? "Remote desktop through a controlled native helper." : remoteProtocol === "vnc" ? "VNC desktop through a controlled native helper." : "A quiet command surface for the machine in front of you."}</p>
            </div>
            <div className="heading-actions">
              <button className="outline-button" onClick={() => setPaletteOpen(true)}><Command size={15} /> Command palette <span>⌘ ⇧ P</span></button>
              <button className="outline-button" onClick={() => setMacrosOpen(true)}><Play size={15} /> Macros <span>⌘ ⇧ M</span></button>
              <button className="outline-button" onClick={() => setQuickConnectOpen(true)}><Network size={15} /> Quick connect <span>⌘ K</span></button>
              <button className="primary-button" onClick={startNewTerminal}><Plus size={15} /> New terminal</button>
            </div>
          </div>
          {sessionNotice && <div className="workspace-notice" role="status"><CheckCircle2 size={14} /><span>{sessionNotice}</span></div>}

          <div className="workspace-grid">
            <div className="main-column">
              <div className="context-strip">
                <div className="context-title"><span className="status-pulse" /> {remoteHost ?? "localhost"} <span className="context-separator">/</span> <span className="muted">{terminalStatus === "connected" ? "shell ready" : terminalStatus}</span></div>
                <div className="context-metrics">{remoteProtocol === "rdp" || remoteProtocol === "vnc" ? <><span><LayoutDashboard size={13} /> framebuffer</span><span><ArrowUpFromLine size={13} /> native input</span><span><ShieldCheck size={13} /> helper isolated</span></> : <><span><TerminalIcon size={13} /> PTY</span><span><ArrowUpFromLine size={13} /> bidirectional</span><span><Radio size={13} /> 32 KB batches</span></>}</div>
              </div>

              <div className="view-tabs" role="tablist" aria-label="Workspace views">
                <button className={activeView === "terminal" ? "selected" : ""} onClick={() => setActiveView("terminal")} role="tab" aria-selected={activeView === "terminal"}><TerminalIcon size={15} /> Terminal</button>
                <button className={activeView === "files" ? "selected" : ""} onClick={() => setActiveView("files")} role="tab" aria-selected={activeView === "files"}><Folder size={15} /> Files <span className="tab-badge">SSH</span></button>
                <button className={activeView === "tunnels" ? "selected" : ""} onClick={() => setActiveView("tunnels")} role="tab" aria-selected={activeView === "tunnels"}><Network size={15} /> Tunnels <span className="tab-badge">{activeTunnelCount}</span></button>
                {remoteSessionId && remoteProtocol === "ssh" && <button className={activeView === "monitor" ? "selected" : ""} onClick={() => setActiveView("monitor")} role="tab" aria-selected={activeView === "monitor"}><Gauge size={15} /> Monitor</button>}
                <button className={activeView === "diagnostics" ? "selected" : ""} onClick={() => setActiveView("diagnostics")} role="tab" aria-selected={activeView === "diagnostics"}><Activity size={15} /> Diagnostics</button>
                <button className={activeView === "transfers" ? "selected" : ""} onClick={() => setActiveView("transfers")} role="tab" aria-selected={activeView === "transfers"}><ArrowDownToLine size={15} /> Transfers <span className="tab-badge">{activeTransferCount}</span></button>
              </div>

              {activeView === "terminal" ? (
                <section className="terminal-card" aria-label="Terminal workspace">
                  <div className="terminal-toolbar">
                    <div className="terminal-tab-strip" role="tablist" aria-label="Terminal sessions">{terminalTabs.map((terminal) => <button type="button" key={terminal.id} className={`terminal-tab ${terminal.id === selectedTerminalId ? "selected" : ""}`} role="tab" aria-selected={terminal.id === selectedTerminalId} onClick={() => { setActiveTerminalId(terminal.id); setTerminalLayout({ kind: "pane", terminalId: terminal.id }); setActiveView("terminal"); }}><span className={`terminal-tab-dot terminal-tab-dot-${terminal.status}`} /><span>{terminal.label}</span><span className="terminal-tab-meta">{terminal.status === "connected" ? (terminal.remoteHost ? terminal.remoteProtocol : "zsh") : terminal.status}</span><span className="terminal-tab-close" role="button" aria-label={`Close ${terminal.label}`} onClick={(event) => { event.stopPropagation(); closeTerminal(terminal.id); }}><X size={13} /></span></button>)}</div>
                  <div className="terminal-toolbar-actions"><button type="button" className={`terminal-broadcast-button ${broadcastEnabled ? "active" : ""}`} aria-label="Configure broadcast input" title="Configure broadcast input" onClick={() => setBroadcastOpen(true)}><Radio size={14} /> {broadcastEnabled ? `${broadcastTargetIds.length} targets` : "Broadcast"}</button><button type="button" className="terminal-new-tab" aria-label="New terminal tab" title="New terminal tab" onClick={startNewTerminal}><Plus size={14} /></button><button type="button" aria-label="Split terminal right" title="Split right" onClick={() => openSplit("right")}><PanelRight size={14} /></button><button type="button" aria-label="Split terminal down" title="Split down" onClick={() => openSplit("down")}><PanelBottom size={14} /></button><span className="terminal-chip">{remoteProtocol === "rdp" || remoteProtocol === "vnc" ? "RGBA" : "UTF-8"}</span><span className="terminal-chip">{remoteProtocol === "rdp" || remoteProtocol === "vnc" ? "native" : "256 colors"}</span><button type="button" className={macroRecording ? "terminal-record-button active" : ""} aria-label={macroRecording ? "Stop macro recording" : "Record macro from terminal input"} title={macroRecording ? "Stop macro recording" : "Record macro from terminal input"} onClick={macroRecording ? stopMacroRecording : startMacroRecording} disabled={remoteProtocol === "rdp" || remoteProtocol === "vnc"}><Radio size={14} /></button><button type="button" aria-label="Search terminal" title="Search terminal" onClick={() => setTerminalSearchOpen(true)} disabled={remoteProtocol === "rdp" || remoteProtocol === "vnc"}><Search size={14} /></button><button type="button" aria-label="Copy terminal selection" title="Copy selected terminal text" onClick={() => void copyTerminalSelection()} disabled={remoteProtocol === "rdp" || remoteProtocol === "vnc"}><Copy size={14} /></button><button type="button" aria-label="Clear terminal scrollback" title="Clear terminal scrollback" onClick={clearTerminalScrollback} disabled={remoteProtocol === "rdp" || remoteProtocol === "vnc"}><Trash2 size={14} /></button><button type="button" aria-label="Terminal options" title="Open terminal settings" onClick={() => setSettingsOpen(true)}><MoreHorizontal size={16} /></button></div>
                  </div>
                  {terminalSearchOpen && <div className="terminal-search-bar" role="search"><Search size={14} /><input autoFocus value={terminalSearchQuery} onChange={(event) => updateTerminalSearch(event.target.value)} onKeyDown={(event) => { if (event.key === "Escape") { event.preventDefault(); closeTerminalSearch(); } if (event.key === "Enter") { event.preventDefault(); findTerminalMatch(event.shiftKey ? "previous" : "next"); } }} placeholder="Find in terminal" aria-label="Find in terminal" /><span>{terminalSearchResult.resultCount > 0 ? `${terminalSearchResult.resultIndex + 1}/${terminalSearchResult.resultCount}` : terminalSearchQuery ? "No match" : "Search"}</span><label><input type="checkbox" checked={terminalSearchCaseSensitive} onChange={(event) => setTerminalSearchCaseSensitive(event.target.checked)} /> Aa</label><button type="button" aria-label="Previous terminal match" title="Previous match" onClick={() => findTerminalMatch("previous")} disabled={!terminalSearchQuery}><ArrowUpFromLine size={13} /></button><button type="button" aria-label="Next terminal match" title="Next match" onClick={() => findTerminalMatch("next")} disabled={!terminalSearchQuery}><ArrowDownToLine size={13} /></button><button type="button" aria-label="Close terminal search" title="Close search" onClick={closeTerminalSearch}><X size={14} /></button></div>}
                  {macroRecording && <div className="macro-recording-banner" role="alert"><Radio size={15} /><div><strong>RECORDING INPUT · {macroRecording.terminalLabel}</strong><span>Only terminal input is captured locally. Do not type passwords, tokens, or private keys.</span></div><button type="button" className="danger-button" onClick={stopMacroRecording}><Square size={13} /> Stop recording</button></div>}
                  {broadcastEnabled && <div className="broadcast-banner" role="alert"><ShieldAlert size={15} /><div><strong>BROADCAST INPUT ACTIVE</strong><span>{broadcastTargetIds.length} explicitly selected terminal{broadcastTargetIds.length === 1 ? "" : "s"} · every keystroke is fanned out</span></div><button type="button" className="danger-button" onClick={() => { setBroadcastEnabled(false); setBroadcastOpen(false); setSessionNotice("Broadcast mode disabled. No further input will fan out."); }}><Square size={13} /> Emergency disable <kbd>Esc</kbd></button></div>}
                  {macroRun && <div className="macro-run-banner" role="status"><LoaderCircle className="spin" size={15} /><div><strong>MACRO RUNNING · {macroRun.title}</strong><span>Step {macroRun.step}/{macroRun.total} · {macroRun.targets.join(", ")}</span></div><button type="button" className="danger-button" onClick={cancelMacro}><Square size={13} /> Cancel macro <kbd>Esc</kbd></button></div>}
                  <div className={`terminal-frame terminal-tabs-frame ${terminalLayout.kind === "split" ? "terminal-frame-has-layout" : "terminal-frame-single"}`}><TerminalLayoutView node={terminalLayout} terminals={terminalTabs} renderPane={renderTerminalPane} onFocus={setActiveTerminalId} onStartResize={beginSplitResize} onAdjustResize={adjustSplitRatio} onResetResize={resetSplitRatio} /></div>
                  <div className="terminal-statusbar"><span><span className="status-square" /> {terminalStatus === "connected" ? "connected" : terminalStatus}</span><span>{remoteProtocol ? `${remoteProtocol} transport` : "local process"}</span><span>scrollback 5,000</span><span className="terminal-status-spacer" /><span>⌘K for quick connect</span></div>
                </section>
              ) : activeView === "files" && remoteSessionId && remoteProtocol === "ssh" ? (
                <RemoteFilesView entries={remoteEntries} path={remotePath} status={sftpStatus} error={connectionError} transfers={transfers.filter((transfer) => transfer.terminalId === remoteSessionId)} onOpenTerminal={() => setActiveView("terminal")} onNavigate={navigateRemote} onDownload={startDownload} onUpload={startUpload} onCreateDirectory={createRemoteDirectory} onRename={renameRemote} onDelete={deleteRemote} onSetPermissions={setRemotePermissions} onCopyPath={copyRemotePath} onEdit={openRemoteTextFile} onCancelTransfer={cancelTransfer} onRetryTransfer={retryTransfer} />
              ) : activeView === "tunnels" && remoteSessionId && remoteProtocol === "ssh" ? (
                <TunnelView tunnels={tunnels} onNewTunnel={startLocalForward} onNewDynamicForward={startDynamicForward} onNewRemoteForward={startRemoteForward} onCancelTunnel={cancelTunnel} />
              ) : activeView === "monitor" && remoteSessionId && remoteProtocol === "ssh" ? (
                <RemoteMonitorView snapshot={remoteMonitor} status={remoteMonitorStatus} error={remoteMonitorError} onRefresh={() => void collectRemoteMonitor()} />
              ) : activeView === "transfers" ? (
                <TransferManagerView transfers={transfers} onCancelTransfer={cancelTransfer} onRetryTransfer={retryTransfer} />
              ) : activeView === "diagnostics" ? (
                <NetworkDiagnosticsView host={networkHost} port={networkPort} timeout={networkTimeout} status={networkStatus} addresses={networkAddresses} result={networkResult} fingerprint={networkFingerprint} error={networkError} scanId={networkScanId} scanStatus={networkScanStatus} scanStart={networkScanStart} scanEnd={networkScanEnd} scanConcurrency={networkScanConcurrency} scanScanned={networkScanScanned} scanTotal={networkScanTotal} scanResults={networkScanResults} diagnosticKind={networkDiagnosticKind} diagnosticStatus={networkDiagnosticStatus} pingResult={networkPingResult} tracerouteResult={networkTracerouteResult} traceMaxHops={networkTraceMaxHops} onHostChange={setNetworkHost} onPortChange={setNetworkPort} onTimeoutChange={setNetworkTimeout} onTraceMaxHopsChange={setNetworkTraceMaxHops} onResolve={resolveNetworkHost} onCheckTcp={checkNetworkTcp} onInspectFingerprint={inspectNetworkHostKey} onPing={startNetworkPing} onTraceroute={startNetworkTraceroute} onCancelDiagnostic={cancelNetworkDiagnostic} onScanStartChange={setNetworkScanStart} onScanEndChange={setNetworkScanEnd} onScanConcurrencyChange={setNetworkScanConcurrency} onStartScan={startNetworkScan} onCancelScan={cancelNetworkScan} />
              ) : (
                <EmptyProtocolView view={activeView} onAction={activeView === "tunnels" || activeView === "monitor" ? () => setQuickConnectOpen(true) : undefined} />
              )}

              <div className="lower-grid">
                <InfoCard icon={ShieldCheck} label="Security boundary" title="Credentials never cross into React" detail="Session records carry references. Secret material stays in the native layer." action="Read threat model" onAction={() => setHelpOpen(true)} />
                <InfoCard icon={ArrowUpFromLine} label="Transport" title="Backpressure is explicit" detail="PTY output is bounded before it reaches the renderer, keeping noisy jobs responsive." action="View architecture" onAction={() => setHelpOpen(true)} />
              </div>
            </div>

            <aside className="right-rail">
              <div className="rail-heading"><span>Session brief</span><button aria-label="Session options" title="Open connection options" onClick={() => setQuickConnectOpen(true)}><MoreHorizontal size={15} /></button></div>
                <div className="machine-card">
                <div className="machine-icon"><Server size={18} /></div>
                <div><div className="machine-name">{remoteHost ?? "This Mac"}</div><div className="machine-detail">{remoteHost ? (remoteProtocol === "telnet" ? "Telnet · unencrypted" : remoteProtocol === "serial" ? "Serial · device" : remoteProtocol === "rdp" ? "RDP · isolated helper" : remoteProtocol === "vnc" ? "VNC · isolated helper" : "SSH · verified transport") : "Apple Silicon · local"}</div></div>
                <span className="machine-live">LIVE</span>
              </div>
              <div className="rail-group"><div className="rail-label">Runtime</div><Metric label="Surface" value={remoteProtocol === "rdp" || remoteProtocol === "vnc" ? "remote desktop" : remoteHost ? "remote shell" : "zsh"} /><Metric label="Renderer" value={remoteProtocol === "rdp" || remoteProtocol === "vnc" ? "RGBA framebuffer" : "xterm-256color"} /><Metric label="Process" value={terminalStatus === "connected" ? "running" : "idle"} /></div>
              <div className="rail-group"><div className="rail-label">Workspace notes</div><p className="rail-copy">The local terminal is the first real vertical slice. SSH and SFTP slots are visible so the workspace can grow without hiding unfinished protocol claims.</p></div>
              <div className="rail-callout"><div className="callout-icon"><Network size={15} /></div><div><strong>{remoteProtocol === "telnet" ? "Telnet transport active" : remoteProtocol === "serial" ? "Serial transport active" : remoteProtocol === "rdp" ? "RDP helper active" : remoteProtocol === "vnc" ? "VNC helper active" : remoteHost ? "SSH transport active" : "Connect securely"}</strong><p>{remoteProtocol === "telnet" ? "This legacy terminal is unencrypted; use SSH for protected administration." : remoteProtocol === "serial" ? "Serial traffic depends on the connected hardware; MobaRust does not add encryption." : remoteProtocol === "rdp" ? "The remote desktop is isolated behind the native helper boundary; certificate and gateway options remain explicit." : remoteProtocol === "vnc" ? "The VNC framebuffer and input stay in the native helper boundary; legacy VNC transport is not SSH-level encryption." : remoteHost ? "Host-key verification and native PTY negotiation are active for this shell." : "Known-host verification and PTY negotiation are ready for a real SSH connection."}</p><button onClick={() => setQuickConnectOpen(true)}>{remoteHost ? "Open another session" : "Quick connect"} <ExternalLink size={12} /></button></div></div>
            </aside>
          </div>

          <footer className="workspace-footer"><span><span className="footer-led" /> MobaRust core · v0.1.0</span><span>Rust PTY bridge</span><span>{navigator.platform.includes("Mac") ? "macOS" : navigator.platform.includes("Win") ? "Windows" : "Linux"} · local mode</span><span className="footer-spacer" /><span>{now.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })} CET</span></footer>
        </section>
      </div>

      {paletteOpen && <CommandPalette onClose={() => setPaletteOpen(false)} onNewTerminal={startNewTerminal} onQuickConnect={() => { setQuickConnectOpen(true); setPaletteOpen(false); }} onOpenFiles={openSftpView} onOpenSettings={() => { setSettingsOpen(true); setPaletteOpen(false); }} onOpenCredentials={() => { setCredentialsOpen(true); setPaletteOpen(false); }} onOpenSnippets={() => { setSnippetsOpen(true); setPaletteOpen(false); }} onOpenMacros={() => { setMacrosOpen(true); setPaletteOpen(false); }} onToggleSidebar={() => { setSidebarOpen((open) => !open); setPaletteOpen(false); }} />}
      {helpOpen && <HelpModal onClose={() => setHelpOpen(false)} />}
      {quickConnectOpen && <QuickConnectDialog error={connectionError} onClose={() => { setQuickConnectOpen(false); setConnectionError(null); }} onConnectSsh={connectSsh} onConnectTelnet={connectTelnet} onConnectSerial={connectSerial} onConnectRemoteDesktop={connectRemoteDesktop} />}
      {editingSession && <SessionEditor session={editingSession} onClose={() => setEditingSession(null)} onSave={saveEditedSession} />}
      {settingsOpen && <SettingsModal settings={settings} portableVaultStatus={portableVaultStatus} onClose={() => setSettingsOpen(false)} onSave={saveSettings} onReset={resetSettings} onPortableCreate={createPortableVault} onPortableUnlock={unlockPortableVault} onPortableLock={lockPortableVault} />}
      {credentialsOpen && <CredentialVaultModal portableVaultStatus={portableVaultStatus} onClose={() => setCredentialsOpen(false)} onSave={saveCredential} onDelete={deleteCredential} onPortableSave={savePortableCredential} onPortableDelete={deletePortableCredential} />}
      {editingRemoteFile && <RemoteEditorModal key={editingRemoteFile.revision} document={editingRemoteFile} onClose={() => setEditingRemoteFile(null)} onSave={saveRemoteTextFile} onSaveAs={saveRemoteTextFileAs} />}
      {snippetsOpen && <SnippetsModal snippets={snippets} onClose={() => setSnippetsOpen(false)} onSave={saveSnippet} onDelete={deleteSnippet} onCopy={copySnippet} />}
      {macrosOpen && <MacrosModal key={recordedMacroDraft?.id ?? "macros"} initialDraft={recordedMacroDraft ?? undefined} macros={macros} terminals={terminalTabs} savedSessions={savedSessions} onClose={() => { setMacrosOpen(false); setRecordedMacroDraft(null); }} onSave={saveMacro} onDelete={deleteMacro} onRun={runMacro} />}
      {broadcastOpen && <BroadcastModal terminals={terminalTabs} selectedIds={broadcastTargetIds} enabled={broadcastEnabled} onClose={() => setBroadcastOpen(false)} onToggle={(id) => setBroadcastTargetIds((current) => current.includes(id) ? current.filter((item) => item !== id) : [...current, id])} onEnable={() => { if (broadcastTargetIds.length === 0) { setConnectionError("Select at least one ready terminal before enabling broadcast."); return; } setBroadcastEnabled(true); setBroadcastOpen(false); setConnectionError(null); setSessionNotice("Broadcast mode enabled. Review the red banner before typing."); }} onDisable={() => { setBroadcastEnabled(false); setBroadcastOpen(false); setSessionNotice("Broadcast mode disabled. No further input will fan out."); }} />}
    </main>
  );
}

function requestFromSavedAuth(auth: SavedAuth): SshConnectRequest["auth"] | null {
  if (auth.kind === "agent") return { method: "agent" };
  if (auth.kind === "password" && auth.credentialRef.trim()) {
    return { method: "password", credentialId: auth.credentialRef };
  }
  if (auth.kind === "keyboardInteractive" && auth.credentialRef.trim()) {
    return { method: "keyboardInteractive", credentialId: auth.credentialRef };
  }
  if (auth.kind === "privateKey" && auth.keyRef.trim()) {
    return { method: "privateKey", path: auth.keyRef, passphraseCredentialId: auth.credentialRef ?? undefined };
  }
  return null;
}

function findSavedJumpSession(catalog: SavedSession[], alias: string): SavedSession | undefined {
  const trimmed = alias.trim();
  const direct = catalog.find((candidate) => candidate.id === trimmed || candidate.name === trimmed);
  if (direct) return direct;
  const userless = trimmed.includes("@") ? trimmed.slice(trimmed.lastIndexOf("@") + 1) : trimmed;
  const bracketedHost = userless.startsWith("[") ? userless.match(/^\[([^\]]+)\](?::(\d+))?$/) : null;
  const host = bracketedHost?.[1] ?? userless.replace(/:(\d+)$/, "");
  const portText = bracketedHost?.[2] ?? userless.match(/:(\d+)$/)?.[1];
  const port = portText ? Number(portText) : undefined;
  return catalog.find((candidate) => candidate.hostname === host && (!port || candidate.port === port));
}

function requestFromSavedSession(session: SavedSession, catalog: SavedSession[], visited = new Set<string>()): SshConnectRequest | null {
  if (session.protocol !== "SSH" || visited.has(session.id)) return null;
  const username = session.username?.trim();
  if (!username || session.port === 0) return null;
  const auth = requestFromSavedAuth(session.auth);
  if (!auth) return null;
  const nextVisited = new Set(visited);
  nextVisited.add(session.id);
  const directJumpHosts = session.jump_host_profiles && session.jump_host_profiles.length > 0
    ? session.jump_host_profiles.map((jump) => {
      const jumpAuth = requestFromSavedAuth(jump.auth);
      if (!jumpAuth || jump.port === 0 || !jump.username.trim() || !jump.host.trim()) return null;
      return {
        host: jump.host,
        port: jump.port,
        username: jump.username,
        auth: jumpAuth,
        knownHostsPath: jump.known_hosts_path ?? undefined,
        pinnedFingerprint: jump.pinned_fingerprint ?? undefined,
      };
    })
    : undefined;
  let jumpHosts: SshJumpHostRequest[] = [];
  if (directJumpHosts) {
    if (directJumpHosts.some((jump) => jump === null)) return null;
    jumpHosts = directJumpHosts as SshJumpHostRequest[];
  } else {
    for (const alias of session.jump_hosts) {
      const jumpSession = findSavedJumpSession(catalog, alias);
      if (!jumpSession) return null;
      const jumpRequest = requestFromSavedSession(jumpSession, catalog, nextVisited);
      if (!jumpRequest) return null;
      jumpHosts.push(...(jumpRequest.jumpHosts ?? []));
      jumpHosts.push({
        host: jumpRequest.host,
        port: jumpRequest.port,
        username: jumpRequest.username,
        auth: jumpRequest.auth,
        knownHostsPath: jumpRequest.knownHostsPath,
        pinnedFingerprint: jumpRequest.pinnedFingerprint,
      });
    }
  }
  return {
    host: session.hostname,
    port: session.port,
    username,
    auth,
    knownHostsPath: session.known_hosts_path ?? undefined,
    pinnedFingerprint: session.pinned_fingerprint ?? undefined,
    jumpHosts: jumpHosts.length > 0 ? jumpHosts : undefined,
    cols: 120,
    rows: 32,
  };
}

function toSessionListItem(session: SavedSession): SessionListItem {
  if (session.protocol === "LOCAL") {
    return { id: session.id, name: session.name, detail: "zsh · localhost", type: "LOCAL", folder: session.folder ?? "Local terminals", active: true, favorite: session.favorite, tags: session.tags, lastUsedAt: session.last_used_at };
  }
  if (session.protocol === "SERIAL" && session.serial_profile) {
    return { id: session.id, name: session.name, detail: `${session.serial_profile.device} · ${session.serial_profile.baud_rate}`, type: session.protocol, folder: session.folder ?? "Serial devices", active: false, favorite: session.favorite, tags: session.tags, lastUsedAt: session.last_used_at };
  }
  const user = session.username ? `${session.username}@` : "";
  const port = session.port && session.port !== 22 ? `:${session.port}` : "";
  return { id: session.id, name: session.name, detail: `${user}${session.hostname}${port}`, type: session.protocol, folder: session.folder ?? "Unfiled", active: false, favorite: session.favorite, tags: session.tags, lastUsedAt: session.last_used_at };
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

function RemoteFilesView({ entries, path, status, error, transfers, onOpenTerminal, onNavigate, onDownload, onUpload, onCreateDirectory, onRename, onDelete, onSetPermissions, onCopyPath, onEdit, onCancelTransfer, onRetryTransfer }: {
  entries: RemoteEntry[];
  path: string;
  status: "idle" | "loading" | "ready" | "error";
  error: string | null;
  transfers: SshTransferEvent[];
  onOpenTerminal: () => void;
  onNavigate: (path: string) => void;
  onDownload: (entry: RemoteEntry, protocol: TransferProtocol) => void;
  onUpload: (protocol: TransferProtocol) => void;
  onCreateDirectory: () => void;
  onRename: (entry: RemoteEntry) => void;
  onDelete: (entry: RemoteEntry) => void;
  onSetPermissions: (entry: RemoteEntry) => void;
  onCopyPath: (entry: RemoteEntry) => void;
  onEdit: (entry: RemoteEntry) => void;
  onCancelTransfer: (transferId: string) => void;
  onRetryTransfer: (transfer: SshTransferEvent) => void;
}) {
  const [transferProtocol, setTransferProtocol] = useState<TransferProtocol>("sftp");
  const [sort, setSort] = useState<RemoteFileSort>("name");
  const [showHidden, setShowHidden] = useState(true);
  const parentPath = path === "." || path === "/" ? path : path.split("/").slice(0, -1).join("/") || ".";
  const visibleEntries = entries
    .filter((entry) => showHidden || !entry.name.startsWith("."))
    .slice()
    .sort((first, second) => {
      if (sort === "type" && first.isDirectory !== second.isDirectory) return first.isDirectory ? -1 : 1;
      if (sort === "size" && first.size !== second.size) return second.size > first.size ? 1 : -1;
      if (sort === "modified" && first.modifiedUnixSeconds !== second.modifiedUnixSeconds) return (second.modifiedUnixSeconds ?? 0) > (first.modifiedUnixSeconds ?? 0) ? 1 : -1;
      return first.name.localeCompare(second.name, undefined, { sensitivity: "base", numeric: true });
    });
  return <section className="remote-files" aria-label="Remote files">
      <div className="remote-files-toolbar">
      <div><span className="eyebrow">SFTP / BROWSER</span><strong>{path}</strong></div>
      <div className="remote-files-toolbar-actions">
        <button className="outline-button" onClick={onOpenTerminal}><TerminalIcon size={14} /> Open terminal</button>
        <label className="transfer-protocol-select">Transport<select aria-label="Transfer transport" value={transferProtocol} onChange={(event) => setTransferProtocol(event.target.value as TransferProtocol)}><option value="sftp">SFTP · recommended</option><option value="scp">SCP · legacy files</option></select></label>
        <label className="transfer-protocol-select">Sort<select aria-label="Sort remote files" value={sort} onChange={(event) => setSort(event.target.value as RemoteFileSort)}><option value="name">Name</option><option value="type">Type</option><option value="size">Size</option><option value="modified">Modified</option></select></label>
        <label className="remote-files-hidden"><input type="checkbox" checked={showHidden} onChange={(event) => setShowHidden(event.target.checked)} /> Hidden</label>
        <button className="outline-button" onClick={onCreateDirectory}><FolderPlus size={14} /> New folder</button>
        <button className="outline-button" onClick={() => onUpload(transferProtocol)}><Upload size={14} /> Upload</button>
        <button className="outline-button" onClick={() => onNavigate(path)} disabled={status === "loading"}><RefreshCw size={14} /> {status === "loading" ? "Refreshing" : "Refresh"}</button>
      </div>
    </div>
    <div className="remote-files-meta"><span>{status === "ready" ? `${visibleEntries.length}${visibleEntries.length === entries.length ? "" : ` of ${entries.length}`} entries` : status === "error" ? "Unable to list directory" : "Streaming directory listing"}</span><span className="remote-files-safe"><ShieldCheck size={13} /> Native transport · bounded transfers</span></div>
    {error && <div className="remote-files-error" role="alert"><CircleX size={14} /><span>{error}</span></div>}
    <div className="remote-files-list">
      <div className="remote-file-row parent"><button className="remote-file-main" onClick={() => onNavigate(parentPath)}><span className="remote-file-icon"><Folder size={15} /></span><span>..</span><small>parent directory</small></button></div>
      {visibleEntries.map((entry) => <div className={`remote-file-row ${entry.isDirectory ? "directory" : ""}`} key={entry.path}>
        <button className="remote-file-main" onClick={() => entry.isDirectory ? onNavigate(entry.path) : undefined} aria-label={entry.isDirectory ? `Open ${entry.name}` : entry.name}>
          <span className="remote-file-icon">{entry.isDirectory ? <Folder size={15} /> : <ArrowDownToLine size={15} />}</span><span>{entry.name}</span><small>{remoteEntryDetails(entry)}</small>
        </button>
        <button className="remote-file-action" onClick={() => onDownload(entry, entry.isDirectory ? "sftp" : transferProtocol)} title={`${entry.isDirectory ? "Download directory" : "Download"} ${entry.name}`} aria-label={`${entry.isDirectory ? "Download directory" : "Download"} ${entry.name}`}><Download size={14} /></button>
        {!entry.isDirectory && <button className="remote-file-action" onClick={() => onEdit(entry)} title={`Edit ${entry.name}`} aria-label={`Edit ${entry.name}`}><Pencil size={14} /></button>}
        <button className="remote-file-action" onClick={() => onCopyPath(entry)} title={`Copy path for ${entry.name}`} aria-label={`Copy path for ${entry.name}`}><Copy size={14} /></button>
        <button className="remote-file-action" onClick={() => onSetPermissions(entry)} title={`Change permissions for ${entry.name}`} aria-label={`Change permissions for ${entry.name}`}><Settings2 size={14} /></button>
        <button className="remote-file-action" onClick={() => onRename(entry)} title={`Rename ${entry.name}`} aria-label={`Rename ${entry.name}`}><Pencil size={14} /></button>
        <button className="remote-file-action danger" onClick={() => onDelete(entry)} title={`Delete ${entry.name}`} aria-label={`Delete ${entry.name}`}><Trash2 size={14} /></button>
      </div>)}
      {status === "ready" && entries.length === 0 && <div className="remote-files-empty">This directory is empty.</div>}
    </div>
    {transfers.length > 0 && <TransferPanel transfers={transfers} onCancelTransfer={onCancelTransfer} onRetryTransfer={onRetryTransfer} />}
    <div className="remote-files-note">SFTP is the default and supports bounded recursive transfers. SCP is available for single-file compatibility transfers; directories always use SFTP. Individual files commit from temporary local or remote paths, and cancellation never presents a partial local file as complete.</div>
  </section>;
}

function RemoteEditorModal({ document, onClose, onSave, onSaveAs }: { document: RemoteTextDocument; onClose: () => void; onSave: (content: string, encoding: RemoteTextDocument["encoding"]) => Promise<void>; onSaveAs: (path: string, content: string, encoding: RemoteTextDocument["encoding"], overwrite: boolean) => Promise<void> }) {
  const highlightRef = useRef<HTMLPreElement>(null);
  const [content, setContent] = useState(document.content);
  const [encoding, setEncoding] = useState<RemoteTextDocument["encoding"]>(document.encoding);
  const [searchQuery, setSearchQuery] = useState("");
  const [replacement, setReplacement] = useState("");
  const [matchCase, setMatchCase] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const dirty = content !== document.content || encoding !== document.encoding;
  const lineCount = content.split(/\r?\n/).length;
  const matchCount = countTextMatches(content, searchQuery, matchCase);
  const language = remoteEditorLanguage(document.path);

  const close = () => {
    if (dirty && !window.confirm("Discard unsaved remote changes?")) return;
    onClose();
  };

  const save = async () => {
    if (!dirty) return;
    setBusy(true);
    setError(null);
    try {
      await onSave(content, encoding);
    } catch (saveError) {
      setError(String(saveError));
    } finally {
      setBusy(false);
    }
  };

  const replaceAll = () => {
    if (!searchQuery) return;
    setContent((current) => replaceTextMatches(current, searchQuery, replacement, matchCase));
  };

  const saveAs = async () => {
    const target = window.prompt("Remote target path", document.path);
    if (!target?.trim() || target.trim() === document.path) return;
    const overwrite = window.confirm("If the remote target already exists, allow replacing it atomically? Cancel to create only.");
    setBusy(true);
    setError(null);
    try {
      await onSaveAs(target.trim(), content, encoding, overwrite);
    } catch (saveError) {
      setError(String(saveError));
    } finally {
      setBusy(false);
    }
  };

  const syncHighlightScroll = (event: ReactUIEvent<HTMLTextAreaElement>) => {
    if (!highlightRef.current) return;
    highlightRef.current.scrollTop = event.currentTarget.scrollTop;
    highlightRef.current.scrollLeft = event.currentTarget.scrollLeft;
  };

  return <div className="palette-backdrop" role="presentation" onMouseDown={close}><section className="remote-editor-modal" role="dialog" aria-modal="true" aria-label={`Edit ${document.path}`} onMouseDown={(event) => event.stopPropagation()}>
    <div className="session-editor-heading"><div><span className="eyebrow">REMOTE FILE / {encoding.toUpperCase()}</span><h2>{document.path}</h2><p>Bounded editor buffer · {formatBytes(document.size)} · revision {document.revision.slice(0, 18)}…</p></div><button type="button" className="icon-button" aria-label="Close remote editor" onClick={close}><X size={17} /></button></div>
    <div className="remote-editor-toolbar"><div className="remote-editor-search"><input value={searchQuery} onChange={(event) => setSearchQuery(event.target.value)} placeholder="Find" aria-label="Find in remote file" /><input value={replacement} onChange={(event) => setReplacement(event.target.value)} placeholder="Replace with" aria-label="Replacement text" /><label className="remote-editor-case"><input type="checkbox" checked={matchCase} onChange={(event) => setMatchCase(event.target.checked)} /> Aa</label><button type="button" className="outline-button" onClick={replaceAll} disabled={!searchQuery || matchCount === 0 || busy}>Replace all</button><span>{searchQuery ? `${matchCount.toLocaleString()} match${matchCount === 1 ? "" : "es"}` : "Search"}</span></div><div className="remote-editor-meta"><label>Encoding<select value={encoding} onChange={(event) => setEncoding(event.target.value as RemoteTextDocument["encoding"])} aria-label="Remote file encoding"><option value="utf-8">UTF-8</option><option value="windows-1252">Windows-1252</option></select></label><span>{lineCount.toLocaleString()} lines</span><span className={dirty ? "remote-editor-dirty" : ""}>{dirty ? "Unsaved changes" : "No local changes"}</span></div></div>
    {error && <div className="connect-error remote-editor-error" role="alert"><CircleX size={14} /><span>{error.includes("changed since") ? "The remote file changed after it was opened. Reload it before saving to avoid overwriting someone else’s work." : error.includes("target already exists") ? "That remote target already exists. Choose Replace when using Save as if overwriting is intentional." : error}</span></div>}
    <div className="remote-editor-code-shell">
      <pre ref={highlightRef} className="remote-editor-highlight" aria-hidden="true" dangerouslySetInnerHTML={{ __html: highlightRemoteCode(content, language) }} />
      <textarea className="remote-editor-textarea" value={content} onChange={(event) => setContent(event.target.value)} onScroll={syncHighlightScroll} spellCheck={false} autoCapitalize="off" autoCorrect="off" aria-label="Remote file contents" />
    </div>
    <div className="session-editor-footer"><span className="remote-editor-safety"><ShieldCheck size={13} /> Conflict check + rollback-safe promotion</span><div><button type="button" className="outline-button" onClick={close} disabled={busy}>Close</button><button type="button" className="outline-button" onClick={() => void saveAs()} disabled={busy}>{busy ? "Working…" : "Save as"}</button><button type="button" className="primary-button" onClick={() => void save()} disabled={busy || !dirty}>{busy ? "Saving…" : "Save remote file"}</button></div></div>
  </section></div>;
}

function countTextMatches(value: string, query: string, matchCase: boolean): number {
  if (!query) return 0;
  const source = matchCase ? value : value.toLocaleLowerCase();
  const needle = matchCase ? query : query.toLocaleLowerCase();
  let count = 0;
  let offset = 0;
  while ((offset = source.indexOf(needle, offset)) !== -1) {
    count += 1;
    offset += Math.max(needle.length, 1);
  }
  return count;
}

function replaceTextMatches(value: string, query: string, replacement: string, matchCase: boolean): string {
  if (!query) return value;
  if (matchCase) return value.split(query).join(replacement);
  const lowerValue = value.toLocaleLowerCase();
  const lowerQuery = query.toLocaleLowerCase();
  let result = "";
  let offset = 0;
  let match;
  while ((match = lowerValue.indexOf(lowerQuery, offset)) !== -1) {
    result += value.slice(offset, match) + replacement;
    offset = match + query.length;
  }
  return offset === 0 ? value : result + value.slice(offset);
}

type RemoteEditorLanguage = "plain" | "shell" | "json" | "yaml" | "ini";

function remoteEditorLanguage(path: string): RemoteEditorLanguage {
  const lower = path.toLocaleLowerCase();
  if (lower.endsWith(".json") || lower.endsWith(".jsonc")) return "json";
  if (lower.endsWith(".yaml") || lower.endsWith(".yml")) return "yaml";
  if (lower.endsWith(".ini") || lower.endsWith(".conf") || lower.endsWith(".cfg")) return "ini";
  if (lower.endsWith(".sh") || lower.endsWith(".bash") || lower.endsWith(".zsh") || lower.endsWith(".fish") || lower.endsWith("/profile") || lower.endsWith("/rc")) return "shell";
  return "plain";
}

function escapeRemoteEditorHtml(value: string): string {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

/**
 * Remote content is escaped before the fixed, local token spans are added.
 * This function never treats bytes received from a host as HTML.
 */
function highlightRemoteCode(value: string, language: RemoteEditorLanguage): string {
  const escaped = escapeRemoteEditorHtml(value);
  if (language === "plain") return escaped;
  if (language === "shell") {
    return escaped.replace(
      /\b(?:sudo|docker|kubectl|git|ssh|scp|sftp|cd|ls|cat|grep|systemctl|cargo|npm|pnpm)\b|--[A-Za-z0-9-]+|\$\{?[A-Za-z_][A-Za-z0-9_]*\}?/g,
      (token) => `<span class="remote-editor-token-command">${token}</span>`,
    );
  }
  if (language === "json") {
    return escaped
      .replace(/(&quot;[^\n]*?&quot;)(?=\s*:)/g, '<span class="remote-editor-token-key">$1</span>')
      .replace(/\b(true|false|null)\b/g, '<span class="remote-editor-token-literal">$1</span>')
      .replace(/\b-?\d+(?:\.\d+)?\b/g, '<span class="remote-editor-token-number">$&</span>');
  }
  const separator = language === "yaml" ? ":" : "=";
  const keyPattern = new RegExp(`(^|\\n)([A-Za-z][A-Za-z0-9_.-]*)(?=\\s*\\${separator})`, "gm");
  return escaped.replace(keyPattern, '$1<span class="remote-editor-token-key">$2</span>');
}

function TransferManagerView({ transfers, onCancelTransfer, onRetryTransfer }: { transfers: SshTransferEvent[]; onCancelTransfer: (transferId: string) => void; onRetryTransfer: (transfer: SshTransferEvent) => void }) {
  const active = transfers.filter((transfer) => !["completed", "cancelled", "failed"].includes(transfer.state)).length;
  const completed = transfers.filter((transfer) => transfer.state === "completed").length;
  return <section className="transfer-manager" aria-label="Global transfer manager">
    <div className="transfer-manager-heading"><div><span className="eyebrow">WORKSPACE / TRANSFERS</span><strong>Global transfer manager</strong><p>All SFTP and SCP jobs share one bounded queue. Each job can be cancelled without presenting a partial file as complete.</p></div><div className="transfer-manager-summary"><span><b>{active}</b> active</span><span><b>{completed}</b> completed</span><span><b>{transfers.length}</b> retained</span></div></div>
    <div className="transfer-manager-meta"><span>Latest jobs across every SSH session</span><span className="remote-files-safe"><ShieldCheck size={13} /> Native transport · bounded concurrency</span></div>
    <TransferPanel transfers={transfers} onCancelTransfer={onCancelTransfer} onRetryTransfer={onRetryTransfer} />
    <div className="transfer-manager-note">Transfers are retained in memory for the current application run. Source and destination paths are displayed for operator review; secrets are never included in transfer events.</div>
  </section>;
}

function TransferPanel({ transfers, onCancelTransfer, onRetryTransfer }: { transfers: SshTransferEvent[]; onCancelTransfer: (transferId: string) => void; onRetryTransfer: (transfer: SshTransferEvent) => void }) {
  return <section className="transfer-panel" aria-label="Transfers"><div className="transfer-panel-heading"><span className="eyebrow">TRANSFER QUEUE</span><span>{transfers.length} retained</span></div>{transfers.length === 0 && <div className="transfer-empty"><ArrowDownToLine size={20} /><strong>No transfers yet</strong><span>Start an upload or download from a connected SSH session. Jobs will appear here across all sessions.</span></div>}{transfers.slice().reverse().map((transfer) => {
    const percent = transfer.totalBytes && transfer.totalBytes > 0 ? Math.min(100, Math.round((transfer.bytesTransferred / transfer.totalBytes) * 100)) : null;
    const active = !["completed", "cancelled", "failed"].includes(transfer.state);
    const retryable = transfer.state === "failed" || transfer.state === "cancelled";
    return <div className="transfer-row" key={transfer.transferId}><div className="transfer-row-icon">{transfer.state === "completed" ? <CheckCircle2 size={15} /> : transfer.state === "failed" ? <CircleX size={15} /> : <LoaderCircle className={active ? "spin" : ""} size={15} />}</div><div className="transfer-row-copy"><strong>{transfer.direction === "download" ? "↓" : "↑"} {transfer.destination.split(/[\\/]/).pop() || transfer.destination}</strong><small>{transfer.protocol.toUpperCase()} · {transfer.state} · {formatBytes(transfer.bytesTransferred)}{transfer.totalBytes ? ` / ${formatBytes(transfer.totalBytes)}` : ""}{percent === null ? "" : ` · ${percent}%`}{transfer.bytesPerSecond ? ` · ${formatBytesPerSecond(transfer.bytesPerSecond)}` : ""}{active && transfer.etaSeconds != null ? ` · ETA ${formatTransferEta(transfer.etaSeconds)}` : ""}</small><small className="transfer-paths" title={`${transfer.source} → ${transfer.destination}`}>{transfer.source} → {transfer.destination}</small>{transfer.error && <small className="transfer-error">{transfer.error}</small>}<div className="transfer-progress"><span style={{ width: `${percent ?? (active ? 8 : 100)}%` }} /></div></div>{active ? <button className="transfer-cancel" onClick={() => onCancelTransfer(transfer.transferId)} aria-label="Cancel transfer" title="Cancel transfer"><CircleX size={14} /></button> : retryable ? <button className="transfer-cancel" onClick={() => onRetryTransfer(transfer)} aria-label="Retry transfer" title="Retry transfer"><RefreshCw size={14} /></button> : null}</div>;
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

function RemoteMonitorView({ snapshot, status, error, onRefresh }: { snapshot: RemoteMonitorSnapshot | null; status: "idle" | "loading" | "ready" | "error"; error: string | null; onRefresh: () => void }) {
  const memoryUsed = snapshot?.memoryTotalBytes != null && snapshot.memoryAvailableBytes != null
    ? Math.max(0, snapshot.memoryTotalBytes - snapshot.memoryAvailableBytes)
    : null;
  const statusLabel = status === "loading" ? "Collecting" : status === "ready" ? "Snapshot ready" : status === "error" ? "Unavailable" : "Ready";
  return <section className="remote-monitor" aria-label="Remote system monitor">
    <div className="remote-monitor-toolbar">
      <div><span className="eyebrow">SSH / SYSTEM SNAPSHOT</span><strong>Remote system monitor</strong><p>Collect one bounded snapshot through the active SSH connection. No agent is installed and no polling starts automatically.</p></div>
      <div className="remote-monitor-actions"><span className={`remote-monitor-status monitor-${status}`}><span /> {statusLabel}</span><button className="primary-button" onClick={onRefresh} disabled={status === "loading"}><RefreshCw className={status === "loading" ? "spin" : ""} size={14} /> {status === "loading" ? "Collecting…" : "Refresh snapshot"}</button></div>
    </div>
    {error && <div className="connect-error remote-monitor-error" role="alert"><CircleX size={14} /><span>{error}</span></div>}
    {snapshot ? <>
      <div className="remote-monitor-identity"><div><span>Host</span><strong>{snapshot.hostname ?? "Unknown host"}</strong></div><div><span>Kernel</span><strong>{snapshot.kernel ?? "Unavailable"}</strong></div><div><span>Metrics</span><strong>{snapshot.supportedMetrics.length} available</strong></div></div>
      <div className="remote-monitor-grid">
        <MonitorMetric label="Uptime" value={formatDuration(snapshot.uptimeSeconds)} detail="remote clock" />
        <MonitorMetric label="Load average" value={snapshot.loadAverage ? snapshot.loadAverage.map((value) => value.toFixed(2)).join(" · ") : "Unavailable"} detail="1 / 5 / 15 min where supported" />
        <MonitorMetric label="Memory" value={memoryUsed != null && snapshot.memoryTotalBytes != null ? `${formatBytes(memoryUsed)} / ${formatBytes(snapshot.memoryTotalBytes)}` : "Unavailable"} detail="used / total" />
        <MonitorMetric label="Available memory" value={snapshot.memoryAvailableBytes != null ? formatBytes(snapshot.memoryAvailableBytes) : "Unavailable"} detail="best-effort capability" />
        <MonitorMetric label="Root disk" value={snapshot.rootDiskUsedPercent != null ? `${snapshot.rootDiskUsedPercent}% used` : "Unavailable"} detail="/ filesystem" />
        <MonitorMetric label="Processes" value={snapshot.processCount != null ? snapshot.processCount.toLocaleString() : "Unavailable"} detail="visible process count" />
      </div>
      <div className="remote-monitor-note"><ShieldCheck size={14} /><span>Only a fixed, read-only metrics query is sent. Missing commands or platform-specific files produce unavailable fields instead of an assumed Linux result.</span></div>
    </> : <div className="remote-monitor-empty"><Gauge size={21} /><strong>{status === "error" ? "No compatible metrics returned" : "No snapshot yet"}</strong><span>{status === "error" ? "The host may not expose the standard read-only capabilities." : "Refresh to request a one-shot snapshot from this SSH session."}</span></div>}
  </section>;
}

function MonitorMetric({ label, value, detail }: { label: string; value: string; detail: string }) {
  return <article className="remote-monitor-card"><span>{label}</span><strong>{value}</strong><small>{detail}</small></article>;
}

function NetworkDiagnosticsView({ host, port, timeout, status, addresses, result, fingerprint, error, scanId, scanStatus, scanStart, scanEnd, scanConcurrency, scanScanned, scanTotal, scanResults, diagnosticKind, diagnosticStatus, pingResult, tracerouteResult, traceMaxHops, onHostChange, onPortChange, onTimeoutChange, onTraceMaxHopsChange, onResolve, onCheckTcp, onInspectFingerprint, onPing, onTraceroute, onCancelDiagnostic, onScanStartChange, onScanEndChange, onScanConcurrencyChange, onStartScan, onCancelScan }: {
  host: string;
  port: string;
  timeout: string;
  status: "idle" | "running" | "ready" | "error";
  addresses: string[];
  result: TcpCheckResult | null;
  fingerprint: SshHostKeyInspection | null;
  error: string | null;
  scanId: string | null;
  scanStatus: "idle" | "running" | "completed" | "cancelled" | "failed";
  scanStart: string;
  scanEnd: string;
  scanConcurrency: string;
  scanScanned: number;
  scanTotal: number;
  scanResults: TcpCheckResult[];
  diagnosticKind: "ping" | "traceroute" | null;
  diagnosticStatus: "idle" | "running" | "completed" | "cancelled" | "failed";
  pingResult: PingResult | null;
  tracerouteResult: TracerouteResult | null;
  traceMaxHops: string;
  onHostChange: (value: string) => void;
  onPortChange: (value: string) => void;
  onTimeoutChange: (value: string) => void;
  onTraceMaxHopsChange: (value: string) => void;
  onResolve: () => void;
  onCheckTcp: () => void;
  onInspectFingerprint: () => void;
  onPing: () => void;
  onTraceroute: () => void;
  onCancelDiagnostic: () => void;
  onScanStartChange: (value: string) => void;
  onScanEndChange: (value: string) => void;
  onScanConcurrencyChange: (value: string) => void;
  onStartScan: () => void;
  onCancelScan: () => void;
}) {
  const statusLabel = status === "running" ? "Running" : status === "ready" ? "Ready" : status === "error" ? "Needs attention" : "Idle";
  const scanActive = scanStatus === "running";
  const diagnosticActive = diagnosticStatus === "running";
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
    <div className="diagnostics-actions"><button className="outline-button" onClick={onResolve} disabled={status === "running" || diagnosticActive}><Search size={14} /> Resolve DNS</button><button className="primary-button" onClick={onCheckTcp} disabled={status === "running" || diagnosticActive}><Network size={14} /> Check TCP port</button><button className="outline-button" onClick={onInspectFingerprint} disabled={status === "running" || diagnosticActive}><KeyRound size={14} /> Inspect SSH key</button><button className="outline-button" onClick={onPing} disabled={diagnosticActive}><Radio size={14} /> Ping</button><button className="outline-button" onClick={onTraceroute} disabled={diagnosticActive}><Activity size={14} /> Traceroute</button>{diagnosticActive && <button className="outline-button danger-button" onClick={onCancelDiagnostic}><CircleX size={14} /> Cancel {diagnosticKind}</button>}</div>
    <div className="diagnostics-trace-options"><label>Traceroute max hops<input inputMode="numeric" pattern="[0-9]+" value={traceMaxHops} onChange={(event) => onTraceMaxHopsChange(event.target.value)} disabled={diagnosticActive} /><small>1–32 hops; platform permissions may limit results.</small></label></div>
    {error && <div className="connect-error diagnostics-error" role="alert"><CircleX size={14} /><span>{error}</span></div>}
    <div className="diagnostics-results">
      <article className="diagnostic-card"><div className="diagnostic-card-heading"><span className="eyebrow">DNS / ADDRESSES</span><Search size={15} /></div><h3>{addresses.length > 0 ? `${addresses.length} address${addresses.length === 1 ? "" : "es"}` : "No lookup yet"}</h3>{addresses.length > 0 ? <div className="diagnostic-addresses">{addresses.map((address) => <code key={address}>{address}</code>)}</div> : <p>Enter a target and run an explicit lookup. Results are kept in this view only.</p>}</article>
      <article className="diagnostic-card"><div className="diagnostic-card-heading"><span className="eyebrow">TCP / REACHABILITY</span><Network size={15} /></div>{result ? <><h3>{result.host}:{result.port}</h3><div className={`diagnostic-result diagnostic-result-${result.status}`}><span /> {result.status === "open" ? "Open" : result.status === "closed" ? "Closed" : "Timed out"}</div><p>The result describes TCP reachability only; it does not authenticate or identify the service.</p></> : <><h3>No TCP check yet</h3><p>Choose a port explicitly, then run a bounded connection check.</p></>}</article>
      <article className="diagnostic-card"><div className="diagnostic-card-heading"><span className="eyebrow">SSH / HOST KEY</span><KeyRound size={15} /></div>{fingerprint ? <><h3>{fingerprint.host}:{fingerprint.port}</h3><div className="diagnostic-fingerprint"><code>{fingerprint.fingerprint}</code></div><p>Observed during one unauthenticated SSH handshake. Nothing was added to known_hosts.</p></> : <><h3>No SSH key observed</h3><p>Inspect an explicit SSH target to read its server fingerprint without using credentials.</p></>}</article>
      <article className="diagnostic-card"><div className="diagnostic-card-heading"><span className="eyebrow">ICMP / PING</span><Radio size={15} /></div>{pingResult ? <><h3>{pingResult.reachable ? "Reachable" : "No reply"}</h3><div className={`diagnostic-result ${pingResult.reachable ? "diagnostic-result-open" : "diagnostic-result-closed"}`}><span /> {pingResult.elapsedMs} ms observed</div><p>One platform-native echo request was sent to the explicit target.</p></> : <><h3>No ping yet</h3><p>Ping uses one bounded platform-native probe and can be cancelled.</p></>}</article>
      <article className="diagnostic-card"><div className="diagnostic-card-heading"><span className="eyebrow">PATH / TRACEROUTE</span><Activity size={15} /></div>{tracerouteResult ? <><h3>{tracerouteResult.reached ? "Destination reached" : "Path incomplete"}</h3><div className={`diagnostic-result ${tracerouteResult.reached ? "diagnostic-result-open" : "diagnostic-result-timed-out"}`}><span /> {tracerouteResult.hops.length} hop lines · {tracerouteResult.elapsedMs} ms</div><pre className="diagnostic-hops">{tracerouteResult.hops.length > 0 ? tracerouteResult.hops.join("\n") : "No hop output returned."}</pre></> : <><h3>No route trace yet</h3><p>Traceroute is bounded to the selected maximum hops and timeout.</p></>}</article>
    </div>
    <section className="diagnostics-scan" aria-label="Bounded TCP port scan">
      <div className="diagnostics-scan-heading"><div><span className="eyebrow">TCP / BOUNDED SCAN</span><h3>Scan an explicit range</h3><p>Maximum 4096 ports, maximum 128 concurrent checks, and a visible cancellation control.</p></div><span className={`diagnostics-scan-state scan-${scanStatus}`}>{scanStatus === "idle" ? "Ready" : scanStatus}</span></div>
      <div className="diagnostics-scan-fields"><label>Start port<input inputMode="numeric" pattern="[0-9]+" value={scanStart} onChange={(event) => onScanStartChange(event.target.value)} disabled={scanActive} /></label><label>End port<input inputMode="numeric" pattern="[0-9]+" value={scanEnd} onChange={(event) => onScanEndChange(event.target.value)} disabled={scanActive} /></label><label>Concurrency<input inputMode="numeric" pattern="[0-9]+" value={scanConcurrency} onChange={(event) => onScanConcurrencyChange(event.target.value)} disabled={scanActive} /></label><div className="diagnostics-scan-action">{scanActive ? <button className="outline-button" onClick={onCancelScan}><CircleX size={14} /> Cancel scan</button> : <button className="primary-button" onClick={onStartScan}><Search size={14} /> Start bounded scan</button>}</div></div>
      {(scanStatus !== "idle" || scanResults.length > 0) && <div className="diagnostics-scan-progress"><div className="diagnostics-progress-label"><span>{scanId ? `Scan ${scanId.slice(0, 8)}` : "Scan"}</span><strong>{scanScanned}/{scanTotal || "—"} · {scanProgress}%</strong></div><div className="diagnostics-progress-track"><span style={{ width: `${scanProgress}%` }} /></div><div className="diagnostics-open-results">{scanResults.length > 0 ? scanResults.filter((item) => item.status === "open").map((item) => <code key={item.port}>{item.port} open</code>) : <span>No open ports reported yet.</span>}</div></div>}
    </section>
    <div className="diagnostics-note"><ShieldCheck size={14} /><span>Safety boundary: target, range, hop limit, concurrency, timeout, and action are explicit. SSH key inspection uses no credentials, agent, personal key, or known_hosts file; results are diagnostics only.</span></div>
  </section>;
}

function formatRemoteModified(seconds?: number | null) {
  if (seconds == null || !Number.isFinite(seconds)) return "modified unknown";
  const date = new Date(seconds * 1000);
  return Number.isNaN(date.getTime()) ? "modified unknown" : `modified ${date.toLocaleDateString([], { year: "numeric", month: "short", day: "numeric" })}`;
}

function remoteEntryDetails(entry: RemoteEntry) {
  const details = [
    entry.isDirectory ? "directory" : formatBytes(entry.size),
    formatRemoteModified(entry.modifiedUnixSeconds),
    formatRemotePermissions(entry.permissions),
    formatRemoteOwner(entry),
  ].filter(Boolean);
  return details.join(" · ");
}

function formatRemotePermissions(permissions?: number | null) {
  if (permissions == null || !Number.isFinite(permissions)) return null;
  return `mode ${((permissions >>> 0) & 0o7777).toString(8).padStart(4, "0")}`;
}

function formatRemoteOwner(entry: RemoteEntry) {
  const owner = entry.owner?.trim() || (entry.uid != null ? `uid ${entry.uid}` : null);
  const group = entry.group?.trim() || (entry.gid != null ? `gid ${entry.gid}` : null);
  return owner && group ? `${owner}:${group}` : owner || group;
}

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function formatBytesPerSecond(bytes: number) {
  return `${formatBytes(bytes)}/s`;
}

function formatTransferEta(seconds: number) {
  if (!Number.isFinite(seconds) || seconds < 0) return "—";
  if (seconds < 60) return `${Math.round(seconds)}s`;
  const minutes = Math.floor(seconds / 60);
  const remainingSeconds = Math.round(seconds % 60);
  return `${minutes}m ${String(remainingSeconds).padStart(2, "0")}s`;
}

function formatDuration(seconds?: number | null) {
  if (seconds == null || !Number.isFinite(seconds)) return "Unavailable";
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${minutes}m`;
  return `${minutes}m`;
}

function EmptyProtocolView({ view, onAction }: { view: "files" | "tunnels" | "monitor"; onAction?: () => void }) {
  const isFiles = view === "files";
  const isMonitor = view === "monitor";
  return <section className="empty-protocol"><div className="empty-protocol-art"><div className="empty-ring ring-one" /><div className="empty-ring ring-two" />{isFiles ? <Folder size={24} /> : isMonitor ? <Gauge size={24} /> : <Network size={24} />}</div><span className="eyebrow">{isFiles ? "REMOTE FILES" : isMonitor ? "REMOTE MONITOR" : "NETWORK FABRIC"}</span><h2>{isFiles ? "Open an SSH session to browse files" : isMonitor ? "Connect an SSH session to inspect it" : "No tunnels are active"}</h2><p>{isFiles ? "SFTP listing, streaming transfers, cancellation, and path safety are ready for a connected SSH session." : isMonitor ? "The monitor sends a single bounded read-only query and leaves unsupported platform metrics blank." : "Create a tunnel from a connected SSH session. The manager will expose endpoints, ownership, state, and byte counts."}</p>{onAction ? <button className="outline-button" onClick={onAction}><Network size={14} /> Quick connect</button> : <button className="outline-button" disabled><Settings2 size={14} /> Delivery map</button>}</section>;
}

function InfoCard({ icon: Icon, label, title, detail, action, onAction }: { icon: LucideIcon; label: string; title: string; detail: string; action: string; onAction: () => void }) {
  return <article className="info-card"><div className="info-card-top"><span className="info-icon"><Icon size={15} /></span><span>{label}</span><button aria-label={`More information about ${label}`} title={`More information about ${label}`} onClick={onAction}><MoreHorizontal size={15} /></button></div><h3>{title}</h3><p>{detail}</p><button className="text-button" onClick={onAction}>{action} <ExternalLink size={12} /></button></article>;
}

function Metric({ label, value }: { label: string; value: string }) {
  return <div className="metric"><span>{label}</span><strong>{value}</strong></div>;
}

function HelpModal({ onClose }: { onClose: () => void }) {
  return <div className="palette-backdrop" role="presentation" onMouseDown={onClose}>
    <section className="help-modal" role="dialog" aria-modal="true" aria-label="MobaRust help" onMouseDown={(event) => event.stopPropagation()}>
      <div className="session-editor-heading">
        <div><span className="eyebrow">MOBA / HELP</span><h2>Operate safely</h2><p>Shortcuts, protocol boundaries, and the local-only testing posture.</p></div>
        <button type="button" className="icon-button" aria-label="Close help" onClick={onClose}><X size={17} /></button>
      </div>
      <div className="help-grid">
        <section className="help-section"><span className="settings-section-label">Shortcuts</span><div className="help-shortcut"><span>Quick connect</span><kbd>⌘ K</kbd></div><div className="help-shortcut"><span>Command palette</span><kbd>⌘ ⇧ P</kbd></div><div className="help-shortcut"><span>New local terminal</span><kbd>⌘ N</kbd></div><div className="help-shortcut"><span>Emergency broadcast disable</span><kbd>Esc</kbd></div></section>
        <section className="help-section"><span className="settings-section-label">Security boundary</span><p>Credential references may appear in session configuration, but passwords, passphrases, private-key bytes, and agent material stay in the Rust/native layer.</p><p>Remote terminal output is treated as untrusted text. Pasted multiline commands require confirmation by default.</p></section>
        <section className="help-section"><span className="settings-section-label">Protocol posture</span><p>SSH provides host-key verification, native PTY, SFTP, forwarding, bounded reconnect, and cancellation. Telnet and serial are clearly marked as unencrypted transports.</p><p>RDP and VNC run behind controlled native helper boundaries; packaging and cross-platform interoperability remain explicit release gates.</p></section>
        <section className="help-section"><span className="settings-section-label">Safe testing</span><p>Local validation uses temporary fixtures, synthetic credentials, and loopback services only. It does not inspect personal SSH directories, GitHub keys, your SSH agent, keychains, real hosts, or attached serial devices.</p></section>
      </div>
      <div className="session-editor-footer"><span className="remote-editor-safety"><ShieldCheck size={13} /> No credentials are read by this help screen</span><div><button type="button" className="primary-button" onClick={onClose}>Done</button></div></div>
    </section>
  </div>;
}

function CommandPalette({ onClose, onNewTerminal, onQuickConnect, onOpenFiles, onOpenSettings, onOpenCredentials, onOpenSnippets, onOpenMacros, onToggleSidebar }: { onClose: () => void; onNewTerminal: () => void; onQuickConnect: () => void; onOpenFiles: () => void; onOpenSettings: () => void; onOpenCredentials: () => void; onOpenSnippets: () => void; onOpenMacros: () => void; onToggleSidebar: () => void }) {
  const [query, setQuery] = useState("");
  const commands = quickActions.filter((action) => action.label.toLowerCase().includes(query.toLowerCase()));
  return <div className="palette-backdrop" role="presentation" onMouseDown={onClose}><section className="command-palette" role="dialog" aria-modal="true" aria-label="Command palette" onMouseDown={(event) => event.stopPropagation()}><div className="palette-search"><Search size={17} /><input autoFocus value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search commands" /><kbd>ESC</kbd></div><div className="palette-section-label">Actions</div>{commands.map((action) => { const ActionIcon = action.icon; const run = action.label === "New local terminal" ? onNewTerminal : action.label === "Quick connect" ? onQuickConnect : action.label === "Open SFTP" ? onOpenFiles : action.label === "Settings" ? onOpenSettings : action.label === "Credential vault" ? onOpenCredentials : action.label === "Snippets" ? onOpenSnippets : action.label === "Macros" ? onOpenMacros : onClose; return <button key={action.label} className="palette-item" onClick={() => { run(); onClose(); }}><ActionIcon size={16} /><span>{action.label}</span><kbd>{action.hint}</kbd></button>; })}<button className="palette-item" onClick={onToggleSidebar}><PanelLeftClose size={16} /><span>Toggle sidebar</span><kbd>⌘ B</kbd></button><div className="palette-footer"><span>Navigate <b>↑ ↓</b></span><span>Run <b>↵</b></span><span>Close <b>esc</b></span></div></section></div>;
}

function CredentialVaultModal({ portableVaultStatus, onClose, onSave, onDelete, onPortableSave, onPortableDelete }: { portableVaultStatus: PortableVaultStatus | null; onClose: () => void; onSave: (credentialId: string, secret: string) => Promise<void>; onDelete: (credentialId: string) => Promise<void>; onPortableSave: (credentialId: string, secret: string) => Promise<void>; onPortableDelete: (credentialId: string) => Promise<void> }) {
  const [credentialId, setCredentialId] = useState("");
  const [secret, setSecret] = useState("");
  const [backend, setBackend] = useState<VaultBackend>("platform");
  const [busy, setBusy] = useState(false);
  const portableReady = portableVaultStatus?.enabled && portableVaultStatus.unlocked;

  const save = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!credentialId.trim() || !secret) return;
    setBusy(true);
    try {
      if (backend === "portable") await onPortableSave(credentialId, secret);
      else await onSave(credentialId, secret);
    } finally {
      setSecret("");
      setBusy(false);
    }
  };

  const remove = async () => {
    const backendLabel = backend === "portable" ? "encrypted portable vault" : "platform vault";
    if (!credentialId.trim() || !window.confirm(`Delete credential “${credentialId.trim()}” from the ${backendLabel}?`)) return;
    setBusy(true);
    try {
      if (backend === "portable") await onPortableDelete(credentialId);
      else await onDelete(credentialId);
    } finally {
      setSecret("");
      setBusy(false);
    }
  };

  return <div className="palette-backdrop" role="presentation" onMouseDown={onClose}><form className="credential-modal" role="dialog" aria-modal="true" aria-label="Credential vault" onMouseDown={(event) => event.stopPropagation()} onSubmit={save}>
    <div className="session-editor-heading"><div><span className="eyebrow">NATIVE SECURITY</span><h2>Credential vault</h2><p>Save an opaque reference. Secrets stay inside Rust and are never listed or returned to React.</p></div><button type="button" className="icon-button" aria-label="Close credential vault" onClick={onClose}><X size={17} /></button></div>
    <div className="credential-modal-body">
      <label>Credential reference<input required autoFocus value={credentialId} onChange={(event) => setCredentialId(event.target.value)} placeholder="prod-bastion-password" autoComplete="off" /><small>Letters, numbers, dots, dashes, and underscores only.</small></label>
      <label>Secret<input required type="password" value={secret} onChange={(event) => setSecret(event.target.value)} placeholder="Enter only for this explicit save" autoComplete="new-password" /><small>The field is cleared after the native operation. It is not persisted in app state.</small></label>
      <label>Storage backend<select value={backend} onChange={(event) => setBackend(event.target.value as VaultBackend)}><option value="platform">Platform secure store</option><option value="portable" disabled={!portableReady}>Encrypted portable vault{portableReady ? "" : " (unlock in Settings)"}</option></select></label>
    </div>
    <div className="credential-modal-note"><KeyRound size={14} /><span>{backend === "portable" ? "Encrypted portable storage uses the explicit unlock passphrase and is kept separate from the platform keyring." : "Uses macOS Keychain, Windows Credential Manager, or Linux Secret Service through Rust."} No vault operation runs until you confirm.</span></div>
    <div className="session-editor-footer"><button type="button" className="outline-button danger-button" onClick={() => void remove()} disabled={busy || !credentialId.trim() || (backend === "portable" && !portableReady)}><Trash2 size={14} /> Delete reference</button><div><button type="button" className="outline-button" onClick={onClose} disabled={busy}>Cancel</button><button type="submit" className="primary-button" disabled={busy || !credentialId.trim() || !secret || (backend === "portable" && !portableReady)}>{busy ? "Saving…" : "Save secret"}</button></div></div>
  </form></div>;
}

function newSnippetRecord(): SnippetRecord {
  return { id: crypto.randomUUID(), title: "", description: "", command: "", tags: [], variables: [] };
}

function snippetVariables(command: string): string[] {
  return [...new Set([...command.matchAll(/\$\{([A-Za-z][A-Za-z0-9_]*)\}/g)].map((match) => match[1]))];
}

function renderSnippetCommand(command: string, values: Record<string, string>): string {
  return command.replace(/\$\{([A-Za-z][A-Za-z0-9_]*)\}/g, (match, name: string) => values[name] === undefined ? match : values[name]);
}

function SnippetsModal({ snippets, onClose, onSave, onDelete, onCopy }: { snippets: SnippetRecord[]; onClose: () => void; onSave: (snippet: SnippetRecord) => Promise<void>; onDelete: (snippet: SnippetRecord) => Promise<void>; onCopy: (command: string) => Promise<void> }) {
  const [selectedId, setSelectedId] = useState<string | null>(snippets[0]?.id ?? null);
  const [draft, setDraft] = useState<SnippetRecord>(() => newSnippetRecord());
  const selected = snippets.find((snippet) => snippet.id === selectedId);
  const active = selected ?? draft;

  const save = async (snippet: SnippetRecord) => {
    await onSave(snippet);
    setSelectedId(snippet.id);
    setDraft(snippet);
  };

  const remove = async () => {
    if (!selected) return;
    await onDelete(selected);
    setSelectedId(null);
    setDraft(newSnippetRecord());
  };

  return <div className="palette-backdrop" role="presentation" onMouseDown={onClose}><section className="snippets-modal" role="dialog" aria-modal="true" aria-label="Command snippets" onMouseDown={(event) => event.stopPropagation()}>
    <div className="session-editor-heading"><div><span className="eyebrow">COMMAND LIBRARY</span><h2>Snippets</h2><p>Preview, edit, then copy for a deliberate manual paste. Nothing runs automatically.</p></div><button type="button" className="icon-button" aria-label="Close snippets" onClick={onClose}><X size={17} /></button></div>
    <div className="snippet-layout">
      <aside className="snippet-list" aria-label="Saved snippets"><div className="snippet-list-heading"><span>{snippets.length} saved</span><button type="button" className="outline-button" onClick={() => { setSelectedId(null); setDraft(newSnippetRecord()); }}><Plus size={13} /> New</button></div>{snippets.length === 0 ? <p className="snippet-empty">No snippets yet. Start with a safe, reviewable command.</p> : snippets.map((snippet) => <button type="button" key={snippet.id} className={`snippet-list-item ${snippet.id === selectedId ? "selected" : ""}`} onClick={() => setSelectedId(snippet.id)}><strong>{snippet.title || "Untitled snippet"}</strong><small>{snippet.tags.join(" · ") || "No tags"}</small></button>)}</aside>
      <SnippetForm key={active.id} snippet={active} isNew={!selected} onSave={save} onDelete={remove} onCopy={onCopy} />
    </div>
  </section></div>;
}

function newMacroRecord(): MacroRecord {
  return { id: crypto.randomUUID(), title: "", description: "", tags: [], actions: [], approval: "beforeRun" };
}

function newMacroAction(kind: MacroAction["kind"]): MacroAction {
  if (kind === "sendText") return { kind, text: "" };
  if (kind === "wait") return { kind, milliseconds: 250 };
  if (kind === "sendKey") return { kind, key: "enter" };
  if (kind === "executeCommand") return { kind, command: "" };
  if (kind === "openSession") return { kind, sessionId: "" };
  return { kind, workspaceId: "" };
}

function macroActionSummary(action: MacroAction): string {
  if (action.kind === "sendText") return `Send text · ${action.text.trim() || "empty"}`;
  if (action.kind === "executeCommand") return `Execute command · ${action.command.trim() || "empty"}`;
  if (action.kind === "wait") return `Wait · ${action.milliseconds} ms`;
  if (action.kind === "sendKey") return `Send key · ${action.key}`;
  if (action.kind === "openSession") return "Open saved session";
  return "Switch workspace";
}

function MacrosModal({ initialDraft, macros, terminals, savedSessions, onClose, onSave, onDelete, onRun }: { initialDraft?: MacroRecord; macros: MacroRecord[]; terminals: WorkspaceTerminal[]; savedSessions: SavedSession[]; onClose: () => void; onSave: (record: MacroRecord) => Promise<void>; onDelete: (record: MacroRecord) => Promise<void>; onRun: (record: MacroRecord, targetIds: string[]) => Promise<void> }) {
  const [selectedId, setSelectedId] = useState<string | null>(initialDraft ? null : macros[0]?.id ?? null);
  const [draft, setDraft] = useState<MacroRecord>(() => initialDraft ?? newMacroRecord());
  const [targetIds, setTargetIds] = useState<string[]>(() => terminals.filter((terminal) => terminal.status === "connected" && terminal.remoteProtocol !== "rdp" && terminal.remoteProtocol !== "vnc").map((terminal) => terminal.id));
  const selected = macros.find((record) => record.id === selectedId);
  const active = selected ?? draft;

  const save = async (record: MacroRecord) => {
    await onSave(record);
    setSelectedId(record.id);
    setDraft(record);
  };

  const remove = async () => {
    if (!selected) return;
    await onDelete(selected);
    setSelectedId(null);
    setDraft(newMacroRecord());
  };

  const readyTerminals = terminals.filter((terminal) => terminal.status === "connected" && terminal.remoteProtocol !== "rdp" && terminal.remoteProtocol !== "vnc");

  return <div className="palette-backdrop" role="presentation" onMouseDown={onClose}><section className="macros-modal" role="dialog" aria-modal="true" aria-label="Terminal macros" onMouseDown={(event) => event.stopPropagation()}>
    <div className="session-editor-heading"><div><span className="eyebrow">OPERATOR AUTOMATION</span><h2>Macros</h2><p>Run only after review and confirmation. Actions are bounded, visible, and cancellable.</p></div><button type="button" className="icon-button" aria-label="Close macros" onClick={onClose}><X size={17} /></button></div>
    <div className="macro-layout">
      <aside className="macro-list" aria-label="Saved macros"><div className="snippet-list-heading"><span>{macros.length} saved</span><button type="button" className="outline-button" onClick={() => { setSelectedId(null); setDraft(newMacroRecord()); }}><Plus size={13} /> New</button></div>{macros.length === 0 ? <p className="snippet-empty">No macros yet. Save a small, reviewable sequence.</p> : macros.map((record) => <button type="button" key={record.id} className={`snippet-list-item ${record.id === selectedId ? "selected" : ""}`} onClick={() => setSelectedId(record.id)}><strong>{record.title || "Untitled macro"}</strong><small>{record.actions.length} action{record.actions.length === 1 ? "" : "s"} · {record.tags.join(" · ") || "No tags"}</small></button>)}</aside>
      <MacroEditor key={active.id} record={active} isNew={!selected} savedSessions={savedSessions} terminals={terminals} targets={targetIds} readyTerminals={readyTerminals} onTargetsChange={setTargetIds} onSave={save} onDelete={remove} onRun={(record) => void onRun(record, targetIds)} />
    </div>
  </section></div>;
}

function MacroEditor({ record, isNew, savedSessions, terminals, targets, readyTerminals, onTargetsChange, onSave, onDelete, onRun }: { record: MacroRecord; isNew: boolean; savedSessions: SavedSession[]; terminals: WorkspaceTerminal[]; targets: string[]; readyTerminals: WorkspaceTerminal[]; onTargetsChange: (ids: string[]) => void; onSave: (record: MacroRecord) => Promise<void>; onDelete: () => Promise<void>; onRun: (record: MacroRecord) => void }) {
  const [title, setTitle] = useState(record.title);
  const [description, setDescription] = useState(record.description);
  const [tags, setTags] = useState(record.tags.join(", "));
  const [actions, setActions] = useState<MacroAction[]>(record.actions);
  const [approval, setApproval] = useState<MacroApprovalPolicy>(record.approval ?? "beforeRun");

  const updateAction = (index: number, action: MacroAction) => setActions((current) => current.map((item, itemIndex) => itemIndex === index ? action : item));
  const addAction = (kind: MacroAction["kind"]) => setActions((current) => [...current, newMacroAction(kind)]);
  const removeAction = (index: number) => setActions((current) => current.filter((_, itemIndex) => itemIndex !== index));
  const buildRecord = (): MacroRecord => ({ id: record.id, title: title.trim(), description: description.trim(), tags: [...new Set(tags.split(",").map((tag) => tag.trim()).filter(Boolean))], actions, approval });
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    void onSave(buildRecord());
  };
  const current = buildRecord();
  const hasSessionActions = actions.some((action) => action.kind === "openSession" || action.kind === "switchWorkspace");

  return <form className="macro-form" onSubmit={submit}><div className="snippet-form-heading"><div><span className="settings-section-label">{isNew ? "New macro" : "Edit macro"}</span><strong>{title.trim() || "Untitled macro"}</strong></div>{!isNew && <button type="button" className="session-action danger" onClick={() => void onDelete()} aria-label="Delete macro" title="Delete macro"><Trash2 size={14} /></button>}</div>
    <label>Title<input required value={title} onChange={(event) => setTitle(event.target.value)} placeholder="Restart service safely" /></label>
    <label>Description<textarea value={description} onChange={(event) => setDescription(event.target.value)} placeholder="What this sequence does" rows={2} /></label>
    <label>Tags<input value={tags} onChange={(event) => setTags(event.target.value)} placeholder="ops, maintenance" /></label>
    <label>Approval policy<select value={approval} onChange={(event) => setApproval(event.target.value as MacroApprovalPolicy)}><option value="beforeRun">Confirm before run</option><option value="eachAction">Confirm every action</option></select><small>{approval === "eachAction" ? "A second confirmation appears before each action; cancelling stops the sequence." : "One explicit confirmation appears before the visible, cancellable sequence."}</small></label>
    <div className="macro-actions-heading"><span>Actions · {actions.length}/64</span><small>Each step runs in order and can be cancelled.</small></div>
    <div className="macro-actions-list">{actions.length === 0 && <div className="macro-empty">Add a typed action below. Saving or running an empty macro is blocked.</div>}{actions.map((action, index) => <MacroActionRow key={`${index}-${action.kind}`} index={index} action={action} savedSessions={savedSessions} terminals={terminals} onChange={updateAction} onRemove={removeAction} />)}</div>
    <div className="macro-add-actions"><button type="button" className="outline-button" onClick={() => addAction("sendText")}><Plus size={13} /> Text</button><button type="button" className="outline-button" onClick={() => addAction("executeCommand")}><Plus size={13} /> Command</button><button type="button" className="outline-button" onClick={() => addAction("sendKey")}><Plus size={13} /> Key</button><button type="button" className="outline-button" onClick={() => addAction("wait")}><Plus size={13} /> Wait</button><button type="button" className="outline-button" onClick={() => addAction("openSession")}><Plus size={13} /> Open</button><button type="button" className="outline-button" onClick={() => addAction("switchWorkspace")}><Plus size={13} /> Switch</button></div>
    <div className="macro-targets"><div className="macro-targets-heading"><span>Explicit targets</span><small>{readyTerminals.length} ready · {targets.length} selected</small></div>{readyTerminals.length === 0 ? <p>No connected terminal is ready for a macro run.</p> : readyTerminals.map((terminal) => <label key={terminal.id} className="macro-target"><input type="checkbox" checked={targets.includes(terminal.id)} onChange={() => onTargetsChange(targets.includes(terminal.id) ? targets.filter((id) => id !== terminal.id) : [...targets, terminal.id])} /><span>{terminal.label}</span><small>{terminal.remoteHost ?? "local"}</small></label>)}</div>
    <div className="macro-safety"><ShieldAlert size={14} /><span>{hasSessionActions ? "This macro can change session focus or open a saved session; confirmation is required at run time." : "Do not put passwords, tokens, or private keys in macro text. MobaRust never logs macro contents."}</span></div>
    <div className="snippet-form-footer"><span><ShieldCheck size={13} /> Review + confirm before run</span><div><button type="button" className="outline-button" onClick={() => onRun(current)} disabled={actions.length === 0 || targets.length === 0}><Play size={13} /> Run on selected</button><button type="submit" className="primary-button"><CheckCircle2 size={14} /> Save macro</button></div></div>
  </form>;
}

function MacroActionRow({ index, action, savedSessions, terminals, onChange, onRemove }: { index: number; action: MacroAction; savedSessions: SavedSession[]; terminals: WorkspaceTerminal[]; onChange: (index: number, action: MacroAction) => void; onRemove: (index: number) => void }) {
  return <div className="macro-action-row"><div className="macro-action-row-heading"><span>{String(index + 1).padStart(2, "0")} · {macroActionSummary(action)}</span><button type="button" className="session-action danger" onClick={() => onRemove(index)} aria-label={`Remove action ${index + 1}`} title="Remove action"><Trash2 size={13} /></button></div><label>Type<select value={action.kind} onChange={(event) => onChange(index, newMacroAction(event.target.value as MacroAction["kind"]))}><option value="sendText">Send text</option><option value="executeCommand">Execute command</option><option value="sendKey">Send key</option><option value="wait">Wait</option><option value="openSession">Open saved session</option><option value="switchWorkspace">Switch workspace</option></select></label>{action.kind === "sendText" && <label>Text<textarea value={action.text} onChange={(event) => onChange(index, { kind: "sendText", text: event.target.value })} rows={2} placeholder="Text sent exactly as entered" /></label>}{action.kind === "executeCommand" && <label>Command<textarea value={action.command} onChange={(event) => onChange(index, { kind: "executeCommand", command: event.target.value })} rows={2} placeholder="systemctl status app" /></label>}{action.kind === "sendKey" && <label>Key<select value={action.key} onChange={(event) => onChange(index, { kind: "sendKey", key: event.target.value as MacroKey })}><option value="enter">Enter</option><option value="escape">Escape</option><option value="tab">Tab</option><option value="backspace">Backspace</option><option value="ctrlC">Ctrl+C</option><option value="ctrlD">Ctrl+D</option><option value="arrowUp">Arrow up</option><option value="arrowDown">Arrow down</option><option value="arrowLeft">Arrow left</option><option value="arrowRight">Arrow right</option></select></label>}{action.kind === "wait" && <label>Milliseconds<input type="number" min="1" max="300000" value={action.milliseconds} onChange={(event) => onChange(index, { kind: "wait", milliseconds: Number(event.target.value) })} /></label>}{action.kind === "openSession" && <label>Saved session<select value={action.sessionId} onChange={(event) => onChange(index, { kind: "openSession", sessionId: event.target.value })}><option value="">Select a saved session</option>{savedSessions.map((session) => <option key={session.id} value={session.id}>{session.name} · {session.protocol}</option>)}</select></label>}{action.kind === "switchWorkspace" && <label>Workspace<select value={action.workspaceId} onChange={(event) => onChange(index, { kind: "switchWorkspace", workspaceId: event.target.value })}><option value="">Select a workspace</option>{terminals.map((terminal) => <option key={terminal.id} value={terminal.id}>{terminal.label}</option>)}</select></label>}</div>;
}

function BroadcastModal({ terminals, selectedIds, enabled, onClose, onToggle, onEnable, onDisable }: { terminals: WorkspaceTerminal[]; selectedIds: string[]; enabled: boolean; onClose: () => void; onToggle: (id: string) => void; onEnable: () => void; onDisable: () => void }) {
  const ready = terminals.filter((terminal) => terminal.status === "connected" && terminal.remoteProtocol !== "rdp" && terminal.remoteProtocol !== "vnc");
  return <div className="palette-backdrop" role="presentation" onMouseDown={onClose}><section className="broadcast-modal" role="dialog" aria-modal="true" aria-label="Broadcast input" onMouseDown={(event) => event.stopPropagation()}><div className="session-editor-heading"><div><span className="eyebrow">TERMINAL / BROADCAST</span><h2>Broadcast input</h2><p>Select exact targets before enabling. Every keystroke is sent to all selected terminals.</p></div><button type="button" className="icon-button" aria-label="Close broadcast settings" onClick={onClose}><X size={17} /></button></div><div className="broadcast-warning"><ShieldAlert size={17} /><div><strong>{enabled ? "Broadcast is active" : "Broadcast is off"}</strong><span>Use <kbd>Esc</kbd> at any time to disable it. Pasted multiline input still requires confirmation.</span></div></div><div className="broadcast-target-list"><div className="macro-targets-heading"><span>Target terminals</span><small>{selectedIds.length} selected · {ready.length} ready</small></div>{terminals.map((terminal) => { const isReady = terminal.status === "connected" && terminal.remoteProtocol !== "rdp" && terminal.remoteProtocol !== "vnc"; return <label className={`macro-target ${!isReady ? "disabled" : ""}`} key={terminal.id}><input type="checkbox" disabled={!isReady} checked={selectedIds.includes(terminal.id)} onChange={() => onToggle(terminal.id)} /><span>{terminal.label}</span><small>{isReady ? (terminal.remoteHost ?? "local") : terminal.remoteProtocol === "rdp" || terminal.remoteProtocol === "vnc" ? "remote desktop · text broadcast disabled" : terminal.status}</small></label>; })}</div><div className="session-editor-footer"><span className="remote-editor-safety"><ShieldCheck size={13} /> No input is sent while configuring</span><div><button type="button" className="outline-button" onClick={enabled ? onDisable : onClose}>{enabled ? "Disable" : "Cancel"}</button>{!enabled && <button type="button" className="primary-button" onClick={onEnable} disabled={selectedIds.length === 0}><Radio size={14} /> Enable broadcast</button>}</div></div></section></div>;
}

function SnippetForm({ snippet, isNew, onSave, onDelete, onCopy }: { snippet: SnippetRecord; isNew: boolean; onSave: (snippet: SnippetRecord) => Promise<void>; onDelete: () => Promise<void>; onCopy: (command: string) => Promise<void> }) {
  const [title, setTitle] = useState(snippet.title);
  const [description, setDescription] = useState(snippet.description);
  const [command, setCommand] = useState(snippet.command);
  const [tags, setTags] = useState(snippet.tags.join(", "));
  const [variables, setVariables] = useState(snippet.variables.join(", "));
  const [values, setValues] = useState<Record<string, string>>({});
  const detectedVariables = [...new Set([...variables.split(",").map((value) => value.trim()).filter(Boolean), ...snippetVariables(command)])];
  const rendered = renderSnippetCommand(command, values);

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    void onSave({
      id: snippet.id,
      title: title.trim(),
      description: description.trim(),
      command,
      tags: [...new Set(tags.split(",").map((tag) => tag.trim()).filter(Boolean))],
      variables: [...new Set(detectedVariables)],
    });
  };

  return <form className="snippet-form" onSubmit={submit}><div className="snippet-form-heading"><div><span className="settings-section-label">{isNew ? "New snippet" : "Edit snippet"}</span><strong>{title.trim() || "Untitled snippet"}</strong></div>{!isNew && <button type="button" className="session-action danger" onClick={() => void onDelete()} aria-label="Delete snippet" title="Delete snippet"><Trash2 size={14} /></button>}</div>
    <label>Title<input required value={title} onChange={(event) => setTitle(event.target.value)} placeholder="Restart service" /></label>
    <label>Description<textarea value={description} onChange={(event) => setDescription(event.target.value)} placeholder="What this command is for" rows={2} /></label>
    <label>Command<textarea required value={command} onChange={(event) => setCommand(event.target.value)} placeholder="docker restart ${service}" rows={5} /></label>
    <div className="snippet-form-row"><label>Tags<input value={tags} onChange={(event) => setTags(event.target.value)} placeholder="docker, ops" /></label><label>Variables<input value={variables} onChange={(event) => setVariables(event.target.value)} placeholder="service, host" /><small>Detected placeholders are included automatically.</small></label></div>
    <div className="snippet-preview"><div className="snippet-preview-heading"><span>Preview</span><small>{detectedVariables.length} variable{detectedVariables.length === 1 ? "" : "s"}</small></div>{detectedVariables.length > 0 && <div className="snippet-variable-grid">{detectedVariables.map((variable) => <label key={variable}>{variable}<input value={values[variable] ?? ""} onChange={(event) => setValues((current) => ({ ...current, [variable]: event.target.value }))} placeholder={`Value for ${variable}`} /></label>)}</div>}<pre>{rendered || "Your command preview will appear here."}</pre></div>
    <div className="snippet-form-footer"><span><ShieldCheck size={13} /> Manual review required</span><div><button type="button" className="outline-button" onClick={() => void onCopy(rendered)} disabled={!rendered.trim()}>Copy rendered</button><button type="submit" className="primary-button"><CheckCircle2 size={14} /> Save snippet</button></div></div>
  </form>;
}

function SessionEditor({ session, onClose, onSave }: { session: SavedSession; onClose: () => void; onSave: (session: SavedSession) => void }) {
  const [name, setName] = useState(session.name);
  const [folder, setFolder] = useState(session.folder ?? "");
  const [tags, setTags] = useState(session.tags.join(", "));
  const [startupDirectory, setStartupDirectory] = useState(session.startup_directory ?? "");
  const [startupCommand, setStartupCommand] = useState(session.startup_command ?? "");
  const [notes, setNotes] = useState(session.notes ?? "");
  const [favorite, setFavorite] = useState(session.favorite);
  const isSsh = session.protocol === "SSH";
  const [authKind, setAuthKind] = useState<"agent" | "password" | "privateKey" | "keyboardInteractive">(
    session.auth.kind === "none" ? "agent" : session.auth.kind,
  );
  const [credentialRef, setCredentialRef] = useState(
    session.auth.kind === "password" || session.auth.kind === "keyboardInteractive" || session.auth.kind === "privateKey"
      ? session.auth.credentialRef ?? ""
      : "",
  );
  const [keyRef, setKeyRef] = useState(session.auth.kind === "privateKey" ? session.auth.keyRef : "");
  const [authError, setAuthError] = useState<string | null>(null);

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const normalizedTags = [...new Set(tags.split(",").map((tag) => tag.trim()).filter(Boolean))];
    let auth = session.auth;
    if (isSsh) {
      setAuthError(null);
      if (authKind === "password" || authKind === "keyboardInteractive") {
        if (!credentialRef.trim()) {
          setAuthError("Enter an opaque vault credential reference.");
          return;
        }
        auth = { kind: authKind, credentialRef: credentialRef.trim() };
      } else if (authKind === "privateKey") {
        if (!keyRef.trim()) {
          setAuthError("Enter the private-key path or reference. MobaRust will not read it while editing.");
          return;
        }
        auth = { kind: "privateKey", keyRef: keyRef.trim(), credentialRef: credentialRef.trim() || null };
      } else {
        auth = { kind: "agent" };
      }
    }
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
      auth,
    });
  };

  const endpoint = session.username ? `${session.username}@${session.hostname}:${session.port}` : `${session.hostname}:${session.port}`;
  const authLabel = isSsh
    ? authKind === "agent" ? "SSH agent" : authKind === "password" ? "Vault credential reference" : authKind === "keyboardInteractive" ? "Keyboard-interactive vault response" : "Private key reference"
    : session.auth.kind === "none" ? "No authentication" : session.auth.kind === "agent" ? "SSH agent" : session.auth.kind === "password" ? "Vault credential reference" : session.auth.kind === "keyboardInteractive" ? "Keyboard-interactive vault response" : "Private key reference";

  return (
    <div className="palette-backdrop" role="presentation" onMouseDown={onClose}>
      <form className="session-editor" role="dialog" aria-modal="true" aria-label={`Edit ${session.name}`} onMouseDown={(event) => event.stopPropagation()} onSubmit={submit}>
        <div className="session-editor-heading">
          <div>
            <span className="eyebrow">SESSION / METADATA</span>
            <h2>Edit session</h2>
            <p>Organize this profile and change only opaque credential references. Secret material stays native.</p>
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

        {isSsh && <div className="session-editor-auth">
          <span className="settings-section-label">SSH authentication</span>
          <div className="session-editor-grid">
            <label className="quick-connect-wide">
              Method
              <select value={authKind} onChange={(event) => { setAuthKind(event.target.value as typeof authKind); setAuthError(null); }}>
                <option value="agent">Local SSH agent</option>
                <option value="privateKey">Private key path</option>
                <option value="password">Vault password reference</option>
                <option value="keyboardInteractive">Keyboard-interactive vault response</option>
              </select>
            </label>
            {authKind === "privateKey" && <label className="quick-connect-wide">
              Private key path or reference
              <input value={keyRef} onChange={(event) => setKeyRef(event.target.value)} placeholder="path or approved key reference" />
              <small>Only this non-secret path crosses IPC. The key file is never opened by the editor.</small>
            </label>}
            {(authKind === "password" || authKind === "keyboardInteractive" || authKind === "privateKey") && <label className="quick-connect-wide">
              {authKind === "privateKey" ? "Passphrase credential reference" : authKind === "keyboardInteractive" ? "Response credential reference" : "Credential reference"}
              <input value={credentialRef} onChange={(event) => setCredentialRef(event.target.value)} placeholder="ops-password" />
              <small>Opaque identifier only; the secret is retrieved inside Rust when connecting.</small>
            </label>}
          </div>
          {authError && <div className="connect-error-inline" role="alert">{authError}</div>}
          <div className="credential-modal-note"><ShieldCheck size={14} /><span>No password, passphrase, private-key bytes, or agent material is displayed or persisted here.</span></div>
        </div>}

        <div className="session-editor-footer">
          <button type="button" className={`favorite-toggle ${favorite ? "selected" : ""}`} onClick={() => setFavorite((value) => !value)}><Star size={14} fill={favorite ? "currentColor" : "none"} /> {favorite ? "Favorite" : "Add to favorites"}</button>
          <div><button type="button" className="outline-button" onClick={onClose}>Cancel</button><button type="submit" className="primary-button"><CheckCircle2 size={14} /> Save changes</button></div>
        </div>
      </form>
    </div>
  );
}

function SettingsModal({ settings, portableVaultStatus, onClose, onSave, onReset, onPortableCreate, onPortableUnlock, onPortableLock }: { settings: AppSettings; portableVaultStatus: PortableVaultStatus | null; onClose: () => void; onSave: (settings: AppSettings) => void; onReset: () => void; onPortableCreate: (passphrase: string) => Promise<void>; onPortableUnlock: (passphrase: string) => Promise<void>; onPortableLock: () => Promise<void> }) {
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
  const [portablePassphrase, setPortablePassphrase] = useState("");
  const [portableBusy, setPortableBusy] = useState(false);

  const runPortableAction = async (action: (passphrase: string) => Promise<void>) => {
    if (!portablePassphrase) return;
    setPortableBusy(true);
    try {
      await action(portablePassphrase);
    } finally {
      setPortablePassphrase("");
      setPortableBusy(false);
    }
  };

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

        <div className="settings-section">
          <span className="settings-section-label">Portable encrypted vault</span>
          <div className="settings-grid settings-vault-grid">
            <div className="settings-vault-status"><strong>{portableVaultStatus?.enabled ? portableVaultStatus.unlocked ? "Unlocked" : portableVaultStatus.exists ? "Locked" : "Not created" : "Unavailable"}</strong><small>{portableVaultStatus?.enabled ? `File: ${portableVaultStatus.path}` : "Portable mode activates only when portable.flag is beside the executable."}</small></div>
            <label>Unlock passphrase<input type="password" value={portablePassphrase} onChange={(event) => setPortablePassphrase(event.target.value)} autoComplete="new-password" placeholder="Required for explicit vault action" /><small>Cleared after the native operation; never returned to React.</small></label>
            <div className="settings-vault-actions">{portableVaultStatus?.enabled && !portableVaultStatus.unlocked && portableVaultStatus.exists ? <button type="button" className="outline-button" onClick={() => void runPortableAction(onPortableUnlock)} disabled={portableBusy || !portablePassphrase}>Unlock</button> : null}{portableVaultStatus?.enabled && !portableVaultStatus.unlocked && !portableVaultStatus.exists ? <button type="button" className="outline-button" onClick={() => void runPortableAction(onPortableCreate)} disabled={portableBusy || !portablePassphrase}>Create encrypted vault</button> : null}{portableVaultStatus?.unlocked && <button type="button" className="outline-button danger-button" onClick={() => void onPortableLock()} disabled={portableBusy}>Lock vault</button>}</div>
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

function QuickConnectDialog({ error, onClose, onConnectSsh, onConnectTelnet, onConnectSerial, onConnectRemoteDesktop }: { error: string | null; onClose: () => void; onConnectSsh: (request: SshConnectRequest) => void; onConnectTelnet: (request: TelnetConnectRequest) => void; onConnectSerial: (request: SerialConnectRequest) => void; onConnectRemoteDesktop: (request: RemoteDesktopConnectRequest) => void }) {
  const [host, setHost] = useState("");
  const [port, setPort] = useState("22");
  const [username, setUsername] = useState("");
  const [protocol, setProtocol] = useState<"ssh" | "telnet" | "serial" | DesktopProtocol>("ssh");
  const [method, setMethod] = useState<"agent" | "privateKey" | "password" | "keyboardInteractive">("agent");
  const [keyPath, setKeyPath] = useState("");
  const [passphraseCredentialId, setPassphraseCredentialId] = useState("");
  const [credentialId, setCredentialId] = useState("");
  const [domain, setDomain] = useState("");
  const [desktopWidth, setDesktopWidth] = useState("1280");
  const [desktopHeight, setDesktopHeight] = useState("720");
  const [desktopColorDepth, setDesktopColorDepth] = useState("32");
  const [desktopAudio, setDesktopAudio] = useState(false);
  const [knownHostsPath, setKnownHostsPath] = useState("");
  const [pinnedFingerprint, setPinnedFingerprint] = useState("");
  const [jumpHost, setJumpHost] = useState("");
  const [jumpPort, setJumpPort] = useState("22");
  const [jumpUsername, setJumpUsername] = useState("");
  const [terminal, setTerminal] = useState("xterm-256color");
  const [encoding, setEncoding] = useState<"utf-8" | "windows-1252">("utf-8");
  const [serialDevice, setSerialDevice] = useState("");
  const [serialDevices, setSerialDevices] = useState<SerialDeviceInfo[]>([]);
  const [serialDevicesLoading, setSerialDevicesLoading] = useState(false);
  const [serialDevicesError, setSerialDevicesError] = useState<string | null>(null);
  const [baudRate, setBaudRate] = useState("115200");
  const [dataBits, setDataBits] = useState<SerialConnectRequest["dataBits"]>("eight");
  const [stopBits, setStopBits] = useState<SerialConnectRequest["stopBits"]>("one");
  const [parity, setParity] = useState<SerialConnectRequest["parity"]>("none");
  const [flowControl, setFlowControl] = useState<SerialConnectRequest["flowControl"]>("none");
  const [lineEnding, setLineEnding] = useState<SerialConnectRequest["lineEnding"]>("cr-lf");

  const refreshSerialDevices = async () => {
    if (!IS_TAURI) {
      setSerialDevicesError("Device refresh requires the desktop runtime.");
      return;
    }
    setSerialDevicesLoading(true);
    setSerialDevicesError(null);
    try {
      setSerialDevices(await invoke<SerialDeviceInfo[]>("serial_list_devices"));
    } catch (refreshError) {
      setSerialDevicesError(String(refreshError));
    } finally {
      setSerialDevicesLoading(false);
    }
  };

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
    if (protocol === "rdp" || protocol === "vnc") {
      onConnectRemoteDesktop({
        protocol,
        host: host.trim(),
        port: Number(port),
        username: username.trim() || (protocol === "vnc" ? "viewer" : ""),
        domain: domain.trim() || undefined,
        credentialId: credentialId.trim() || undefined,
        width: Number(desktopWidth),
        height: Number(desktopHeight),
        colorDepth: Number(desktopColorDepth),
        audioEnabled: desktopAudio,
      });
      return;
    }
    const auth = method === "agent"
      ? { method: "agent" as const }
      : method === "privateKey"
        ? { method: "privateKey" as const, path: keyPath, passphraseCredentialId: passphraseCredentialId.trim() || undefined }
        : method === "keyboardInteractive"
          ? { method: "keyboardInteractive" as const, credentialId }
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
                  : protocol === "rdp"
                    ? "Open a remote desktop through the isolated native helper."
                    : protocol === "vnc"
                      ? "Open a VNC desktop through the isolated native helper."
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
                const next = event.target.value as "ssh" | "telnet" | "serial" | DesktopProtocol;
                setProtocol(next);
                setPort(next === "ssh" ? "22" : next === "telnet" ? "23" : next === "rdp" ? "3389" : next === "vnc" ? "5900" : "0");
              }}
            >
              <option value="ssh">SSH</option>
              <option value="telnet">Telnet · unencrypted</option>
              <option value="rdp">RDP · native helper</option>
              <option value="vnc">VNC · native helper</option>
              <option value="serial">Serial device</option>
            </select>
          </label>

          {protocol === "serial" ? (
            <>
              <label className="quick-connect-wide">
                Device path
                <div className="serial-device-input"><input autoFocus required list="serial-device-options" value={serialDevice} onChange={(event) => setSerialDevice(event.target.value)} placeholder="/dev/tty.usbserial-… or COM3" /><button type="button" className="outline-button" onClick={() => void refreshSerialDevices()} disabled={serialDevicesLoading}>{serialDevicesLoading ? "Refreshing" : "Refresh"}</button></div>
                <datalist id="serial-device-options">{serialDevices.map((item) => <option key={item.device} value={item.device}>{item.kind}</option>)}</datalist>
                <small>Refresh is explicit and reads port metadata only; manual entry remains available.</small>
                {serialDevicesError && <small className="connect-error-inline" role="alert">{serialDevicesError}</small>}
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
                    <select value={method} onChange={(event) => setMethod(event.target.value as "agent" | "privateKey" | "password" | "keyboardInteractive")}>
                      <option value="agent">Local SSH agent</option>
                      <option value="privateKey">Private key path</option>
                      <option value="password">Existing vault credential reference</option>
                      <option value="keyboardInteractive">Keyboard-interactive · vault response</option>
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
                  ) : method === "password" || method === "keyboardInteractive" ? (
                    <label className="quick-connect-wide">
                      {method === "keyboardInteractive" ? "Keyboard-interactive response reference" : "Credential reference"}
                      <input required value={credentialId} onChange={(event) => setCredentialId(event.target.value)} placeholder="prod-bastion-password" />
                      <small>{method === "keyboardInteractive" ? "The native layer answers non-echo prompts with this vault secret; echo prompts are refused." : "Only an opaque vault reference crosses IPC, never the password."}</small>
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
              ) : protocol === "rdp" || protocol === "vnc" ? (
                <>
                  <label className="quick-connect-wide">
                    Username <span className="optional">{protocol === "vnc" ? "optional for no-auth servers" : "required"}</span>
                    <input required={protocol === "rdp"} value={username} onChange={(event) => setUsername(event.target.value)} placeholder={protocol === "rdp" ? "Administrator" : "viewer"} />
                  </label>
                  {protocol === "rdp" && <label>
                    Domain <span className="optional">optional</span>
                    <input value={domain} onChange={(event) => setDomain(event.target.value)} placeholder="WORKGROUP" />
                  </label>}
                  <label className="quick-connect-wide">
                    Credential reference <span className="optional">{protocol === "vnc" ? "optional for no-auth" : "required"}</span>
                    <input required={protocol === "rdp"} value={credentialId} onChange={(event) => setCredentialId(event.target.value)} placeholder={protocol === "rdp" ? "windows-admin-password" : "vnc-password"} />
                    <small>Only an opaque vault reference crosses IPC. The secret is handed to the isolated native helper.</small>
                  </label>
                  <label>
                    Width
                    <input required inputMode="numeric" pattern="[0-9]+" value={desktopWidth} onChange={(event) => setDesktopWidth(event.target.value)} />
                  </label>
                  <label>
                    Height
                    <input required inputMode="numeric" pattern="[0-9]+" value={desktopHeight} onChange={(event) => setDesktopHeight(event.target.value)} />
                  </label>
                  <label>
                    Color depth
                    <select value={desktopColorDepth} onChange={(event) => setDesktopColorDepth(event.target.value)}><option value="16">16-bit</option><option value="24">24-bit</option><option value="32">32-bit</option></select>
                  </label>
                  {protocol === "rdp" && <label className="quick-connect-check"><input type="checkbox" checked={desktopAudio} onChange={(event) => setDesktopAudio(event.target.checked)} /> Request audio</label>}
                  <div className="quick-connect-wide quick-connect-hint"><ShieldCheck size={14} /><span>{protocol === "rdp" ? "RDP is isolated in a native helper; certificate validation and gateway support remain explicit capability work." : "VNC is legacy protocol transport; use a protected tunnel when the server does not provide transport encryption."}</span></div>
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
                : protocol === "rdp"
                  ? "RDP uses the isolated helper; credentials stay in the native vault boundary."
                  : protocol === "vnc"
                    ? "VNC capability is isolated; legacy VNC password transport is not SSH-level encryption."
                : "Serial device traffic is not encrypted by MobaRust."}
          </span>
          <div>
            <button type="button" className="outline-button" onClick={onClose}>Cancel</button>
            <button className="primary-button" type="submit">
              <Network size={14} />
              Connect {protocol === "ssh" ? "SSH" : protocol === "telnet" ? "Telnet" : protocol === "rdp" ? "RDP" : protocol === "vnc" ? "VNC" : "Serial"}
            </button>
          </div>
        </div>
      </form>
    </div>
  );
}

export default App;
