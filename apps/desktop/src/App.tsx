import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import {
  Activity,
  ArrowDownToLine,
  ArrowUpFromLine,
  ChevronDown,
  CircleHelp,
  Command,
  Copy,
  ExternalLink,
  Folder,
  LayoutDashboard,
  MoreHorizontal,
  Network,
  PanelLeftClose,
  Plus,
  Radio,
  Search,
  Server,
  Settings2,
  ShieldCheck,
  Star,
  Terminal as TerminalIcon,
  X,
  type LucideIcon,
} from "lucide-react";

type View = "terminal" | "files" | "tunnels";

type TerminalOutputEvent = {
  terminalId: string;
  data: string;
};

type TerminalClosedEvent = {
  terminalId: string;
};

type TerminalViewportProps = {
  instanceKey: number;
  remoteSessionId: string | null;
  onStatusChange: (status: "starting" | "connected" | "closed" | "error") => void;
};

const IS_TAURI = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

type SessionListItem = {
  name: string;
  detail: string;
  type: string;
  active: boolean;
};

type SavedSession = {
  id: string;
  name: string;
  protocol: string;
  hostname: string;
  port: number;
  username?: string | null;
};

type SshConnectRequest = {
  host: string;
  port: number;
  username: string;
  auth: { method: "agent" } | { method: "privateKey"; path: string; passphraseCredentialId?: string } | { method: "password"; credentialId: string };
  knownHostsPath?: string;
  pinnedFingerprint?: string;
  cols: number;
  rows: number;
};

type SshConnectResponse = {
  terminalId: string;
  host: string;
};

type RemoteEntry = {
  name: string;
  path: string;
  size: number;
  isDirectory: boolean;
  modifiedUnixSeconds?: number | null;
};

const previewSessions: SessionListItem[] = [
  { name: "Local workstation", detail: "zsh · localhost", type: "LOCAL", active: true },
  { name: "Production bastion", detail: "ops@bastion.example", type: "SSH", active: false },
  { name: "Staging cluster", detail: "dev@staging.example", type: "SSH", active: false },
];

const quickActions: Array<{ label: string; hint: string; icon: LucideIcon }> = [
  { label: "New local terminal", hint: "⌘ N", icon: TerminalIcon },
  { label: "Quick connect", hint: "⌘ K", icon: Network },
  { label: "Command palette", hint: "⌘ ⇧ P", icon: Command },
];

function TerminalViewport({ instanceKey, remoteSessionId, onStatusChange }: TerminalViewportProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const terminalIdRef = useRef<string | null>(null);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    let disposed = false;
    let unlistenOutput: UnlistenFn | undefined;
    let unlistenClosed: UnlistenFn | undefined;
    const terminal = new Terminal({
      allowProposedApi: true,
      convertEol: false,
      cursorBlink: true,
      cursorStyle: "bar",
      fontFamily: '"JetBrains Mono", "SFMono-Regular", Consolas, monospace',
      fontSize: 13,
      lineHeight: 1.35,
      scrollback: 5000,
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
      if (IS_TAURI && terminalId) {
        void invoke(remoteSessionId ? "ssh_resize" : "terminal_resize", {
          terminalId,
          cols: terminal.cols,
          rows: terminal.rows,
        }).catch(() => undefined);
      }
    };

    const resizeObserver = new ResizeObserver(fit);
    resizeObserver.observe(host);
    requestAnimationFrame(fit);

    const input = terminal.onData((data) => {
      const terminalId = terminalIdRef.current;
      if (!IS_TAURI || !terminalId) {
        terminal.write(data.replace(/\r/g, "\r\n"));
        return;
      }
      void invoke(remoteSessionId ? "ssh_write" : "terminal_write", { terminalId, data }).catch(() => onStatusChange("error"));
    });

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
        const outputEvent = remoteSessionId ? "ssh://output" : "terminal://output";
        const closedEvent = remoteSessionId ? "ssh://closed" : "terminal://closed";
        unlistenOutput = await listen<TerminalOutputEvent>(outputEvent, (event) => {
          if (event.payload.terminalId === terminalIdRef.current) terminal.write(event.payload.data);
        });
        unlistenClosed = await listen<TerminalClosedEvent>(closedEvent, (event) => {
          if (event.payload.terminalId === terminalIdRef.current) onStatusChange("closed");
        });
        if (remoteSessionId) {
          terminalIdRef.current = remoteSessionId;
          const pendingOutput = await invoke<string[]>("ssh_attach", { terminalId: remoteSessionId });
          if (disposed) {
            void invoke("ssh_close", { terminalId: remoteSessionId });
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
      resizeObserver.disconnect();
      unlistenOutput?.();
      unlistenClosed?.();
      const terminalId = terminalIdRef.current;
      if (IS_TAURI && terminalId) void invoke(remoteSessionId ? "ssh_close" : "terminal_close", { terminalId });
      terminalIdRef.current = null;
      terminal.dispose();
    };
  }, [instanceKey, onStatusChange, remoteSessionId]);

  return <div className="terminal-host" ref={hostRef} aria-label="Local terminal" />;
}

function App() {
  const [activeView, setActiveView] = useState<View>("terminal");
  const [terminalKey, setTerminalKey] = useState(0);
  const [terminalOpen, setTerminalOpen] = useState(true);
  const [terminalStatus, setTerminalStatus] = useState<"starting" | "connected" | "closed" | "error">("starting");
  const [search, setSearch] = useState("");
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [quickConnectOpen, setQuickConnectOpen] = useState(false);
  const [connectionError, setConnectionError] = useState<string | null>(null);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [now, setNow] = useState(() => new Date());
  const [sessionRows, setSessionRows] = useState<SessionListItem[]>(IS_TAURI ? [] : previewSessions);
  const [remoteSessionId, setRemoteSessionId] = useState<string | null>(null);
  const [remoteHost, setRemoteHost] = useState<string | null>(null);
  const [remotePath, setRemotePath] = useState(".");
  const [remoteEntries, setRemoteEntries] = useState<RemoteEntry[]>([]);
  const [sftpStatus, setSftpStatus] = useState<"idle" | "loading" | "ready" | "error">("idle");

  const startNewTerminal = useCallback(() => {
    setRemoteSessionId(null);
    setRemoteHost(null);
    setConnectionError(null);
    setTerminalStatus("starting");
    setTerminalOpen(true);
    setTerminalKey((key) => key + 1);
    setActiveView("terminal");
  }, []);

  const handleTerminalStatus = useCallback((status: "starting" | "connected" | "closed" | "error") => {
    setTerminalStatus(status);
  }, []);

  const connectSsh = useCallback(async (request: SshConnectRequest) => {
    setConnectionError(null);
    if (!IS_TAURI) {
      setConnectionError("SSH connections require the desktop runtime.");
      return;
    }
    try {
      const response = await invoke<SshConnectResponse>("ssh_connect", { request });
      setRemoteSessionId(response.terminalId);
      setRemoteHost(response.host);
      setTerminalOpen(true);
      setTerminalStatus("starting");
      setTerminalKey((key) => key + 1);
      setActiveView("terminal");
      setQuickConnectOpen(false);
    } catch (error) {
      setConnectionError(String(error));
    }
  }, []);

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

  useEffect(() => {
    const timer = window.setInterval(() => setNow(new Date()), 30_000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    if (!IS_TAURI) return;
    void invoke<SavedSession[]>("session_list")
      .then((savedSessions) => setSessionRows(savedSessions.map(toSessionListItem)))
      .catch(() => undefined);
  }, []);

  useEffect(() => {
    if (activeView === "files" && remoteSessionId) void loadRemoteDirectory(".");
  }, [activeView, loadRemoteDirectory, remoteSessionId]);

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

  const filteredSessions = sessionRows.filter((session) =>
    `${session.name} ${session.detail} ${session.type}`.toLowerCase().includes(search.toLowerCase()),
  );
  const localSessionCount = sessionRows.filter((session) => session.type === "LOCAL").length;
  const remoteSessionCount = sessionRows.filter((session) => session.type !== "LOCAL").length;

  return (
    <main className={`app-shell ${sidebarOpen ? "" : "sidebar-collapsed"}`}>
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
          <button className="icon-button" aria-label="Settings" title="Settings">
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
            <button className="nav-item active"><LayoutDashboard size={15} /> Overview <span className="nav-count">1</span></button>
            <button className="nav-item"><Star size={15} /> Favorites <span className="nav-count">1</span></button>
            <button className="nav-item"><Activity size={15} /> Recent</button>
          </nav>

          <div className="session-list">
            <div className="list-heading"><span>Sessions</span><button aria-label="Session options"><MoreHorizontal size={15} /></button></div>
            <div className="folder-heading"><ChevronDown size={13} /> Local terminals <span>{localSessionCount}</span></div>
            {filteredSessions.filter((session) => session.type === "LOCAL").map((session) => (
              <SessionRow key={session.name} {...session} />
            ))}
            <div className="folder-heading muted-folder"><ChevronDown size={13} /> Remote sessions <span>{remoteSessionCount}</span></div>
            {filteredSessions.filter((session) => session.type === "SSH").map((session) => (
              <SessionRow key={session.name} {...session} />
            ))}
            {filteredSessions.length === 0 && <div className="empty-search">No matching sessions</div>}
          </div>

          <div className="sidebar-footer">
            <div className="security-note"><ShieldCheck size={15} /><span><strong>Secrets stay native</strong><small>Vault boundary is Rust-owned</small></span></div>
            <button className="nav-item"><Network size={15} /> Tunnel manager</button>
            <button className="nav-item"><ArrowDownToLine size={15} /> Transfers <span className="nav-count">0</span></button>
          </div>
        </aside>

        <section className="workspace">
          {!sidebarOpen && <button className="floating-sidebar-button" onClick={() => setSidebarOpen(true)} aria-label="Expand sidebar"><PanelLeftClose size={16} /></button>}
          <div className="workspace-heading">
            <div>
              <div className="eyebrow"><span>WORKSPACE / 01</span><span className="eyebrow-slash">/</span><span className="muted">{remoteSessionId ? "SSH" : "LOCAL"}</span></div>
              <h1>{remoteHost ?? "Local workstation"}</h1>
              <p className="workspace-subtitle">{remoteHost ? "Interactive SSH shell with native host-key verification." : "A quiet command surface for the machine in front of you."}</p>
            </div>
            <div className="heading-actions">
              <button className="outline-button" onClick={() => setPaletteOpen(true)}><Command size={15} /> Command palette <span>⌘ ⇧ P</span></button>
              <button className="outline-button" onClick={() => setQuickConnectOpen(true)}><Network size={15} /> Quick connect <span>⌘ K</span></button>
              <button className="primary-button" onClick={startNewTerminal}><Plus size={15} /> New terminal</button>
            </div>
          </div>

          <div className="workspace-grid">
            <div className="main-column">
              <div className="context-strip">
                <div className="context-title"><span className="status-pulse" /> {remoteHost ?? "localhost"} <span className="context-separator">/</span> <span className="muted">{terminalStatus === "connected" ? "shell ready" : terminalStatus}</span></div>
                <div className="context-metrics"><span><TerminalIcon size={13} /> PTY</span><span><ArrowUpFromLine size={13} /> bidirectional</span><span><Radio size={13} /> 32 KB batches</span></div>
              </div>

              <div className="view-tabs" role="tablist" aria-label="Workspace views">
                <button className={activeView === "terminal" ? "selected" : ""} onClick={() => setActiveView("terminal")} role="tab" aria-selected={activeView === "terminal"}><TerminalIcon size={15} /> Terminal</button>
                <button className={activeView === "files" ? "selected" : ""} onClick={() => setActiveView("files")} role="tab" aria-selected={activeView === "files"}><Folder size={15} /> Files <span className="tab-badge">SSH</span></button>
                <button className={activeView === "tunnels" ? "selected" : ""} onClick={() => setActiveView("tunnels")} role="tab" aria-selected={activeView === "tunnels"}><Network size={15} /> Tunnels <span className="tab-badge">0</span></button>
              </div>

              {activeView === "terminal" ? (
                <section className="terminal-card" aria-label="Terminal workspace">
                  <div className="terminal-toolbar">
                    <div className="terminal-tab"><span className="terminal-tab-dot" /><span>{remoteHost ? "remote shell" : "local shell"}</span><span className="terminal-tab-meta">{terminalStatus === "connected" ? (remoteHost ? "ssh" : "zsh") : terminalStatus}</span><button aria-label="Close terminal" onClick={() => { setTerminalOpen(false); setTerminalStatus("closed"); setRemoteSessionId(null); setRemoteHost(null); }}><X size={14} /></button></div>
                    <div className="terminal-toolbar-actions"><span className="terminal-chip">UTF-8</span><span className="terminal-chip">256 colors</span><button aria-label="Copy terminal output"><Copy size={14} /></button><button aria-label="Terminal options"><MoreHorizontal size={16} /></button></div>
                  </div>
                  <div className={`terminal-frame ${terminalOpen ? "" : "terminal-frame-closed"}`}>{terminalOpen ? <TerminalViewport key={terminalKey} instanceKey={terminalKey} remoteSessionId={remoteSessionId} onStatusChange={handleTerminalStatus} /> : <div className="terminal-closed"><div className="empty-protocol-art"><TerminalIcon size={21} /></div><strong>Terminal closed</strong><span>Start a fresh local or SSH shell when you are ready.</span><button className="primary-button" onClick={startNewTerminal}><Plus size={14} /> New terminal</button></div>}</div>
                  <div className="terminal-statusbar"><span><span className="status-square" /> {terminalStatus === "connected" ? "connected" : terminalStatus}</span><span>local process</span><span>scrollback 5,000</span><span className="terminal-status-spacer" /><span>⌘K for quick connect</span></div>
                </section>
              ) : activeView === "files" && remoteSessionId ? (
                <RemoteFilesView entries={remoteEntries} path={remotePath} status={sftpStatus} onNavigate={navigateRemote} />
              ) : (
                <EmptyProtocolView view={activeView} />
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
                <div><div className="machine-name">{remoteHost ?? "This Mac"}</div><div className="machine-detail">{remoteHost ? "SSH · verified transport" : "Apple Silicon · local"}</div></div>
                <span className="machine-live">LIVE</span>
              </div>
              <div className="rail-group"><div className="rail-label">Runtime</div><Metric label="Shell" value={remoteHost ? "remote" : "zsh"} /><Metric label="Terminal" value="xterm-256color" /><Metric label="Process" value={terminalStatus === "connected" ? "running" : "idle"} /></div>
              <div className="rail-group"><div className="rail-label">Workspace notes</div><p className="rail-copy">The local terminal is the first real vertical slice. SSH and SFTP slots are visible so the workspace can grow without hiding unfinished protocol claims.</p></div>
              <div className="rail-callout"><div className="callout-icon"><Network size={15} /></div><div><strong>{remoteHost ? "SSH transport active" : "Connect securely"}</strong><p>{remoteHost ? "Host-key verification and native PTY negotiation are active for this shell." : "Known-host verification and PTY negotiation are ready for a real SSH connection."}</p><button onClick={() => setQuickConnectOpen(true)}>{remoteHost ? "Open another session" : "Quick connect"} <ExternalLink size={12} /></button></div></div>
            </aside>
          </div>

          <footer className="workspace-footer"><span><span className="footer-led" /> MobaRust core · v0.1.0</span><span>Rust PTY bridge</span><span>{navigator.platform.includes("Mac") ? "macOS" : navigator.platform.includes("Win") ? "Windows" : "Linux"} · local mode</span><span className="footer-spacer" /><span>{now.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })} CET</span></footer>
        </section>
      </div>

      {paletteOpen && <CommandPalette onClose={() => setPaletteOpen(false)} onNewTerminal={startNewTerminal} onQuickConnect={() => { setQuickConnectOpen(true); setPaletteOpen(false); }} onToggleSidebar={() => { setSidebarOpen((open) => !open); setPaletteOpen(false); }} />}
      {quickConnectOpen && <QuickConnectDialog error={connectionError} onClose={() => { setQuickConnectOpen(false); setConnectionError(null); }} onConnect={connectSsh} />}
    </main>
  );
}

function toSessionListItem(session: SavedSession): SessionListItem {
  if (session.protocol === "LOCAL") {
    return { name: session.name, detail: "zsh · localhost", type: "LOCAL", active: true };
  }
  const user = session.username ? `${session.username}@` : "";
  const port = session.port && session.port !== 22 ? `:${session.port}` : "";
  return { name: session.name, detail: `${user}${session.hostname}${port}`, type: session.protocol, active: false };
}

function SessionRow({ name, detail, type, active }: SessionListItem) {
  return <button className={`session-row ${active ? "active" : ""}`}><span className={`session-icon ${type === "LOCAL" ? "local" : "remote"}`}>{type === "LOCAL" ? <TerminalIcon size={14} /> : <Server size={14} />}</span><span className="session-copy"><strong>{name}</strong><small>{detail}</small></span><span className={`session-type ${type === "LOCAL" ? "local-type" : ""}`}>{type}</span></button>;
}

function RemoteFilesView({ entries, path, status, onNavigate }: { entries: RemoteEntry[]; path: string; status: "idle" | "loading" | "ready" | "error"; onNavigate: (path: string) => void }) {
  const parentPath = path === "." || path === "/" ? path : path.split("/").slice(0, -1).join("/") || ".";
  return <section className="remote-files" aria-label="Remote files"><div className="remote-files-toolbar"><div><span className="eyebrow">SFTP / BROWSER</span><strong>{path}</strong></div><button className="outline-button" onClick={() => onNavigate(path)} disabled={status === "loading"}><Radio size={14} /> {status === "loading" ? "Refreshing" : "Refresh"}</button></div><div className="remote-files-meta"><span>{status === "ready" ? `${entries.length} entries` : status === "error" ? "Unable to list directory" : "Streaming directory listing"}</span><span className="remote-files-safe"><ShieldCheck size={13} /> Native transport · no whole-file buffering</span></div><div className="remote-files-list"><button className="remote-file-row parent" onClick={() => onNavigate(parentPath)}><Folder size={15} /><span>..</span><small>parent directory</small></button>{entries.map((entry) => <button className={`remote-file-row ${entry.isDirectory ? "directory" : ""}`} key={entry.path} onClick={() => entry.isDirectory ? onNavigate(entry.path) : undefined}><span className="remote-file-icon">{entry.isDirectory ? <Folder size={15} /> : <ArrowDownToLine size={15} />}</span><span>{entry.name}</span><small>{entry.isDirectory ? "directory" : formatBytes(entry.size)}</small></button>)}{status === "ready" && entries.length === 0 && <div className="remote-files-empty">This directory is empty.</div>}</div><div className="remote-files-note">Remote editing, atomic upload, conflict detection, and transfer progress are deliberately separate follow-up surfaces.</div></section>;
}

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function EmptyProtocolView({ view }: { view: Exclude<View, "terminal"> }) {
  const isFiles = view === "files";
  return <section className="empty-protocol"><div className="empty-protocol-art"><div className="empty-ring ring-one" /><div className="empty-ring ring-two" />{isFiles ? <Folder size={24} /> : <Network size={24} />}</div><span className="eyebrow">{isFiles ? "REMOTE FILES" : "NETWORK FABRIC"}</span><h2>{isFiles ? "SFTP browser is staged for the SSH slice" : "No tunnels are active"}</h2><p>{isFiles ? "This surface will only appear as usable once streaming transfers, cancellation, and path safety are implemented." : "Create a tunnel from a connected SSH session. The manager will expose endpoints, ownership, state, and byte counts."}</p><button className="outline-button" disabled><Settings2 size={14} /> Delivery map</button></section>;
}

function InfoCard({ icon: Icon, label, title, detail, action }: { icon: LucideIcon; label: string; title: string; detail: string; action: string }) {
  return <article className="info-card"><div className="info-card-top"><span className="info-icon"><Icon size={15} /></span><span>{label}</span><button aria-label="More information"><MoreHorizontal size={15} /></button></div><h3>{title}</h3><p>{detail}</p><button className="text-button">{action} <ExternalLink size={12} /></button></article>;
}

function Metric({ label, value }: { label: string; value: string }) {
  return <div className="metric"><span>{label}</span><strong>{value}</strong></div>;
}

function CommandPalette({ onClose, onNewTerminal, onQuickConnect, onToggleSidebar }: { onClose: () => void; onNewTerminal: () => void; onQuickConnect: () => void; onToggleSidebar: () => void }) {
  const [query, setQuery] = useState("");
  const commands = quickActions.filter((action) => action.label.toLowerCase().includes(query.toLowerCase()));
  return <div className="palette-backdrop" role="presentation" onMouseDown={onClose}><section className="command-palette" role="dialog" aria-modal="true" aria-label="Command palette" onMouseDown={(event) => event.stopPropagation()}><div className="palette-search"><Search size={17} /><input autoFocus value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search commands" /><kbd>ESC</kbd></div><div className="palette-section-label">Actions</div>{commands.map((action) => { const ActionIcon = action.icon; const run = action.label === "New local terminal" ? onNewTerminal : action.label === "Quick connect" ? onQuickConnect : onClose; return <button key={action.label} className="palette-item" onClick={() => { run(); onClose(); }}><ActionIcon size={16} /><span>{action.label}</span><kbd>{action.hint}</kbd></button>; })}<button className="palette-item" onClick={onToggleSidebar}><PanelLeftClose size={16} /><span>Toggle sidebar</span><kbd>⌘ B</kbd></button><div className="palette-footer"><span>Navigate <b>↑ ↓</b></span><span>Run <b>↵</b></span><span>Close <b>esc</b></span></div></section></div>;
}

function QuickConnectDialog({ error, onClose, onConnect }: { error: string | null; onClose: () => void; onConnect: (request: SshConnectRequest) => void }) {
  const [host, setHost] = useState("");
  const [port, setPort] = useState("22");
  const [username, setUsername] = useState("");
  const [method, setMethod] = useState<"agent" | "privateKey" | "password">("agent");
  const [keyPath, setKeyPath] = useState("");
  const [credentialId, setCredentialId] = useState("");
  const [knownHostsPath, setKnownHostsPath] = useState("");
  const [pinnedFingerprint, setPinnedFingerprint] = useState("");

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const auth = method === "agent"
      ? { method: "agent" as const }
      : method === "privateKey"
        ? { method: "privateKey" as const, path: keyPath }
        : { method: "password" as const, credentialId };
    onConnect({
      host: host.trim(),
      port: Number(port),
      username: username.trim(),
      auth,
      knownHostsPath: knownHostsPath.trim() || undefined,
      pinnedFingerprint: pinnedFingerprint.trim() || undefined,
      cols: 120,
      rows: 32,
    });
  };

  return <div className="palette-backdrop" role="presentation" onMouseDown={onClose}><form className="quick-connect" role="dialog" aria-modal="true" aria-label="Quick connect" onMouseDown={(event) => event.stopPropagation()} onSubmit={submit}><div className="quick-connect-heading"><div><span className="eyebrow">NEW SSH SESSION</span><h2>Quick connect</h2><p>Open a real native SSH shell in seconds.</p></div><button type="button" className="icon-button" aria-label="Close quick connect" onClick={onClose}><X size={17} /></button></div><div className="quick-connect-grid"><label>Host<input autoFocus required value={host} onChange={(event) => setHost(event.target.value)} placeholder="bastion.example.com" /></label><label>Port<input required inputMode="numeric" pattern="[0-9]+" value={port} onChange={(event) => setPort(event.target.value)} /></label><label className="quick-connect-wide">Username<input required value={username} onChange={(event) => setUsername(event.target.value)} placeholder="ops" /></label><label className="quick-connect-wide">Authentication<select value={method} onChange={(event) => setMethod(event.target.value as "agent" | "privateKey" | "password")}><option value="agent">Local SSH agent</option><option value="privateKey">Private key path</option><option value="password">Existing vault credential reference</option></select></label>{method === "privateKey" ? <label className="quick-connect-wide">Private key path<input required value={keyPath} onChange={(event) => setKeyPath(event.target.value)} placeholder="~/.ssh/id_ed25519" /><small>The key stays on disk; its passphrase is never entered here.</small></label> : method === "password" ? <label className="quick-connect-wide">Credential reference<input required value={credentialId} onChange={(event) => setCredentialId(event.target.value)} placeholder="prod-bastion-password" /><small>Only an opaque vault reference crosses IPC, never the password.</small></label> : <div className="quick-connect-wide quick-connect-hint"><ShieldCheck size={14} /><span>The native SSH agent signs authentication; private key material stays with the agent.</span></div>}<label className="quick-connect-wide">Known hosts path <span className="optional">optional</span><input value={knownHostsPath} onChange={(event) => setKnownHostsPath(event.target.value)} placeholder="Default: ~/.ssh/known_hosts" /></label><label className="quick-connect-wide">Pinned SHA-256 fingerprint <span className="optional">optional</span><input value={pinnedFingerprint} onChange={(event) => setPinnedFingerprint(event.target.value)} placeholder="SHA256:... (for deliberate first trust)" /></label></div>{error && <div className="connect-error" role="alert"><strong>Connection failed</strong><span>{error}</span></div>}<div className="quick-connect-footer"><span><ShieldCheck size={14} /> Unknown host keys are rejected.</span><div><button type="button" className="outline-button" onClick={onClose}>Cancel</button><button className="primary-button" type="submit"><Network size={14} /> Connect SSH</button></div></div></form></div>;
}

export default App;
