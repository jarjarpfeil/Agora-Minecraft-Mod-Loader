import { useCallback, useEffect, useRef, useState } from 'react';
import { check } from '@tauri-apps/plugin-updater';
import { invoke } from '@tauri-apps/api/core';
import { open as openUrl } from '@tauri-apps/plugin-shell';
import { listen } from '@tauri-apps/api/event';
import {
  cancelJavaRuntime,
  copilotStatus,
  copilotLogout,
  detectMojangLauncher,
  ensureJavaRuntime,
  formatError,
  getAuthStatus,
  getGithubProfile,
  getMcpSkillContent,
  getMcpStatus,
  getSetting,
  githubLogin,
  githubLoginPoll,
  githubLogout,
  inspectJavaExecutable,
  isAuthExpired,
  listInstances,
  listJavaRuntimes,
  msaLogin,
  msaGetStatus,
  msaLogout,
  pickOpenFile,
  removeUnusedJavaRuntimes,
  setMcpApproval,
  startMcpServer,
  stopMcpServer,
  getMCPToken,
  regenerateMCPToken,
  testLauncherPath,
} from '../lib/tauri';
import type { CopilotToken, DeviceFlowResponse, GithubProfile, InstanceRow, JavaRuntimeProgressEvent, JavaRuntimeSummary, McpStatus, McpTokenData, MsaAccountStatus } from '../lib/tauri';
import { Privacy } from './Privacy';
import { useAdvancedMode } from '../components/AdvancedModeContext';
import { DeviceFlowPanel } from '../components/DeviceFlowPanel';
import { useTypedSettings, SETTINGS } from '../lib/useTypedSettings';
import { showToast } from '../components/Toast';

// --- CopyButton helper ---

function CopyButton({ text, label }: { text: string; label: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      onClick={async () => {
        await navigator.clipboard.writeText(text);
        setCopied(true);
        setTimeout(() => setCopied(false), 2000);
      }}
      className="rounded-md bg-primary px-2.5 py-1 text-xs font-medium text-white hover:bg-brand-700 disabled:opacity-50"
      disabled={copied}
    >
      {copied ? 'Copied!' : label}
    </button>
  );
}

/** Token display — hidden by default with a Show/Hide toggle. */
function TokenDisplay({ token }: { token: string }) {
  const [visible, setVisible] = useState(false);
  return (
    <div className="space-y-1">
      {visible ? (
        <pre className="text-xs bg-background rounded-lg p-2.5 overflow-x-auto select-all break-all border border-border">{token}</pre>
      ) : (
        <pre className="text-xs bg-background rounded-lg p-2.5 overflow-x-auto border border-border text-muted-foreground">••••••••••••••••</pre>
      )}
      <button
        onClick={() => setVisible(!visible)}
        className="text-xs text-primary hover:underline"
      >
        {visible ? 'Hide token' : 'Show token'}
      </button>
    </div>
  );
}

export function Settings() {
  const ts = useTypedSettings();

  const [modrinth, setModrinth] = useState(false);
  const [aiMcp, setAiMcp] = useState(false);
  const [aiChatEnabled, setAiChatEnabled] = useState(false);
  const [launcherPath, setLauncherPath] = useState('');
  const [alwaysPreTouch, setAlwaysPreTouch] = useState(true);
  const [loading, setLoading] = useState(true);
  const [directLaunch, setDirectLaunch] = useState(false);

  // MCP server state
  const [mcpStatus, setMcpStatus] = useState<McpStatus | null>(null);
  const [mcpInstances, setMcpInstances] = useState<InstanceRow[]>([]);
  const [instanceApprovals, setInstanceApprovals] = useState<Record<string, string>>({});

  // Skill content state
  const [skillContent, setSkillContent] = useState<string | null>(null);
  const [skillLoading, setSkillLoading] = useState(false);

  // MCP token state (for Bearer auth)
  const [mcpToken, setMcpToken] = useState<McpTokenData | null>(null);
  const [skillCopied, setSkillCopied] = useState(false);

  // AI Copilot state
  const [copilotToken, setCopilotToken] = useState<CopilotToken | null>(null);
  const [copilotLoading, setCopilotLoading] = useState(true);

  // GitHub governance auth state
  const [githubAuth, setGithubAuth] = useState(false);
  const [githubProfile, setGithubProfile] = useState<GithubProfile | null>(null);
  const [githubLoading, setGithubLoading] = useState(true);
  const [ghDevice, setGhDevice] = useState<DeviceFlowResponse | null>(null);
  const [ghPolling, setGhPolling] = useState(false);
  const [ghResult, setGhResult] = useState<string | null>(null);
  const [ghError, setGhError] = useState<string | null>(null);
  const ghSessionRef = useRef(0);

  // MSA auth state
  const [msaCreds, setMsaCreds] = useState<MsaAccountStatus | null>(null);
  const [msaLoading, setMsaLoading] = useState(true);
  const [msaError, setMsaError] = useState<string | null>(null);
  const [msaBusy, setMsaBusy] = useState(false);


  // Java Runtime Management state
  const [javaRuntimeMode, setJavaRuntimeMode] = useState<'automatic' | 'prompt' | 'manual'>('automatic');
  const [javaRuntimes, setJavaRuntimes] = useState<JavaRuntimeSummary[]>([]);
  const [javaRuntimesLoading, setJavaRuntimesLoading] = useState(false);
  const [javaRuntimesError, setJavaRuntimesError] = useState<string | null>(null);
  const [globalJavaPath, setGlobalJavaPath] = useState('');
  const [globalJavaPathInspected, setGlobalJavaPathInspected] = useState<string | null>(null);
  const [globalJavaPathError, setGlobalJavaPathError] = useState<string | null>(null);
  const [javaDownloadBusy, setJavaDownloadBusy] = useState<number | null>(null);
  const [javaDownloadProgress, setJavaDownloadProgress] = useState<string | null>(null);
  const [javaDownloadPercent, setJavaDownloadPercent] = useState<number | null>(null);
  const [javaCancelling, setJavaCancelling] = useState(false);
  const [javaRemoveBusy, setJavaRemoveBusy] = useState(false);
  const [customMajorInput, setCustomMajorInput] = useState('');

  // Launcher path action states (Browse, Auto-detect, Test)
  const [launcherPathError, setLauncherPathError] = useState<string | null>(null);
  const [launcherPathSuccess, setLauncherPathSuccess] = useState<string | null>(null);
  const [launcherPathTesting, setLauncherPathTesting] = useState(false);
  const [launcherPathDetecting, setLauncherPathDetecting] = useState(false);

  const { advancedMode, toggleAdvanced } = useAdvancedMode();
  const isWindows = typeof navigator !== 'undefined' && navigator.platform.includes('Win');

  const fetchMcpStatus = async () => {
    try {
      const s = await getMcpStatus();
      setMcpStatus(s);
    } catch {
      setMcpStatus({ running: false, url: null });
    }
  };

  const fetchInstances = async () => {
    try {
      const instances = await listInstances();
      setMcpInstances(instances);
    } catch {
      setMcpInstances([]);
    }
  };

  // Sync typed settings into local state for backward-compatible render code.
  useEffect(() => {
    if (ts.loading) return;
    setModrinth(ts.values.modrinthEnabled as boolean ?? false);
    setAiMcp(ts.values.aiMcpEnabled as boolean ?? false);
    setAiChatEnabled(ts.values.aiChatEnabled as boolean ?? false);
    setLauncherPath(ts.values.launcherPath as string ?? '');
    setAlwaysPreTouch(ts.values.alwaysPreTouch as boolean ?? true);
    setDirectLaunch((ts.values.launchMode as string) === 'direct');
    setJavaRuntimeMode((ts.values.javaRuntimeMode as string) as 'automatic' | 'prompt' | 'manual' || 'automatic');
    setGlobalJavaPath((ts.values.javaPath as string) ?? '');
    setLoading(false);
  }, [ts.loading, ts.values]);

  // Load MSA status on mount
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const creds = await msaGetStatus();
        if (!cancelled) setMsaCreds(creds);
      } catch {
        if (!cancelled) setMsaCreds(null);
      } finally {
        if (!cancelled) setMsaLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, []);

  // Load launch_mode setting
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const mode = await getSetting('launch_mode');
        if (!cancelled) setDirectLaunch(mode === 'direct');
      } catch {
        // Default to delegation
      }
    })();
    return () => { cancelled = true; };
  }, []);

  // Load MCP status + instances when aiMcp is enabled
  useEffect(() => {
    if (!aiMcp) {
      setMcpStatus(null);
      setMcpInstances([]);
      setInstanceApprovals({});
      return;
    }
    let cancelled = false;
    (async () => {
      await fetchMcpStatus();
      if (cancelled) return;
      await fetchInstances();
      // Load skill content for the "Connect your AI tool" panel
      try {
        const content = await getMcpSkillContent();
        if (!cancelled) setSkillContent(content);
      } catch {
        // Skill content unavailable; panel will show gracefully
      }
      // Fetch MCP Bearer token
      try {
        const data = await getMCPToken();
        if (!cancelled) setMcpToken(data);
      } catch {
        // token unavailable; panel degrades gracefully
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [aiMcp]);

  // Check Copilot connection status on mount
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const token = await copilotStatus();
        if (!cancelled) setCopilotToken(token);
      } catch {
        if (!cancelled) setCopilotToken(null);
      } finally {
        if (!cancelled) setCopilotLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Check GitHub governance auth status on mount
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const isAuth = await getAuthStatus();
        if (cancelled) return;
        setGithubAuth(isAuth);
        if (isAuth) {
          try {
            const profile = await getGithubProfile();
            if (!cancelled) setGithubProfile(profile);
          } catch (e) {
            if (isAuthExpired(e)) {
              // Token expired and was cleared; show signed-out state.
              if (!cancelled) {
                setGithubAuth(false);
                setGhError('Your GitHub session has expired. Sign in again to continue.');
              }
            }
            // Other transient errors: auth status is still valid.
          }
        }
      } catch {
        if (!cancelled) setGithubAuth(false);
      } finally {
        if (!cancelled) setGithubLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const handleGithubSignIn = async () => {
    setGhError(null);
    setGhResult(null);
    setGhPolling(true);
    const mySession = ++ghSessionRef.current;
    const isStale = () => ghSessionRef.current !== mySession;
    try {
      const flow = await githubLogin();
      if (isStale()) return;
      setGhDevice(flow);
      try {
        const p = openUrl(flow.verification_uri);
        Promise.resolve(p).catch(() => {});
      } catch {
        /* best-effort */
      }
      const token = await githubLoginPoll(flow.device_code, flow.interval);
      if (isStale()) return;
      if (token) {
        setGhResult('Signed in successfully.');
        setGithubAuth(true);
        try {
          const profile = await getGithubProfile();
          setGithubProfile(profile);
        } catch {
          // Profile fetch failed
        }
      } else {
        setGhResult('Authentication did not complete.');
      }
    } catch (e) {
      const msg = e instanceof Error ? e.message : formatError(e);
      if (!isStale()) setGhError(`Sign-in failed: ${msg}`);
    } finally {
      if (!isStale()) setGhPolling(false);
    }
  };

  const handleGithubSignOut = async () => {
    try {
      await githubLogout();
      setGithubAuth(false);
      setGithubProfile(null);
      setGhDevice(null);
      setGhResult(null);
    } catch (e) {
      showToast(formatError(e), 'error');
    }
  };

  const handleMsaSignIn = async () => {
    setMsaError(null);
    setMsaBusy(true);
    try {
      const creds = await msaLogin();
      setMsaCreds(creds);
    } catch (e) {
      setMsaError(formatError(e));
    } finally {
      setMsaBusy(false);
    }
  };

  const handleMsaSignOut = async () => {
    try {
      await msaLogout();
      setMsaCreds(null);
    } catch (e) {
      showToast(formatError(e), 'error');
    }
  };

  const toggleLaunchMode = async (value: boolean) => {
    setDirectLaunch(value);
    try {
      await ts.update(SETTINGS.launchMode, value ? 'direct' : 'delegation');
    } catch (e) {
      setDirectLaunch(!value);
      showToast(formatError(e), 'error');
    }
  };

  const toggleModrinth = async (value: boolean) => {
    setModrinth(value);
    try {
      await ts.update(SETTINGS.modrinthEnabled, value);
    } catch (e) {
      setModrinth(!value);
      showToast(formatError(e), 'error');
    }
  };

  const toggleAiMcp = async (value: boolean) => {
    setAiMcp(value);
    try {
      await ts.update(SETTINGS.aiMcpEnabled, value);
    } catch (e) {
      setAiMcp(!value);
      showToast(formatError(e), 'error');
    }
  };

  const toggleAiChat = async (value: boolean) => {
    setAiChatEnabled(value);
    try {
      await ts.update(SETTINGS.aiChatEnabled, value);
    } catch (e) {
      setAiChatEnabled(!value);
      showToast(formatError(e), 'error');
    }
  };

  const saveLauncherPath = async () => {
    try {
      // launcher_path setting is stored as the path directly
      await ts.update(SETTINGS.launcherPath, launcherPath);
      showToast('Launcher path saved.', 'success');
    } catch (e) {
      showToast(formatError(e), 'error');
    }
  };

  const refreshJavaRuntimes = useCallback(async () => {
    setJavaRuntimesLoading(true);
    setJavaRuntimesError(null);
    try {
      const runtimes = await listJavaRuntimes();
      setJavaRuntimes(runtimes);
    } catch (e) {
      setJavaRuntimesError(formatError(e));
    } finally {
      setJavaRuntimesLoading(false);
    }
  }, []);

  // Listen for java-runtime-progress events in Settings
  useEffect(() => {
    const unlisten = listen<JavaRuntimeProgressEvent>(
      'java-runtime-progress',
      (event) => {
        // Only track progress for settings operations (empty instance_id)
        if (event.payload.instance_id !== '') return;
        setJavaDownloadPercent(event.payload.percent);
        setJavaDownloadProgress(event.payload.message);
        if (event.payload.stage === 'ready') {
          refreshJavaRuntimes();
        }
      },
    );
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [refreshJavaRuntimes]);

  const handleJavaRuntimeModeChange = async (mode: 'automatic' | 'prompt' | 'manual') => {
    setJavaRuntimeMode(mode);
    try {
      await ts.update(SETTINGS.javaRuntimeMode, mode);
    } catch (e) {
      setJavaRuntimeMode(javaRuntimeMode); // revert
      showToast(formatError(e), 'error');
    }
  };

  const handleGlobalJavaPathBrowse = async () => {
    setGlobalJavaPathError(null);
    setGlobalJavaPathInspected(null);
    try {
      const chosen = await pickOpenFile('Select Java executable', ['exe', 'java']);
      if (chosen) {
        setGlobalJavaPath(chosen);
        // Auto-inspect
        try {
          const info = await inspectJavaExecutable(chosen);
          setGlobalJavaPathInspected(`Java ${info.version} (${info.arch ?? 'unknown arch'}) — ${info.version_string}`);
        } catch {
          setGlobalJavaPathInspected(null);
        }
      }
    } catch (e) {
      setGlobalJavaPathError(formatError(e));
    }
  };

  const handleGlobalJavaPathSave = async () => {
    setGlobalJavaPathError(null);
    setGlobalJavaPathInspected(null);
    try {
      if (globalJavaPath.trim()) {
        const info = await inspectJavaExecutable(globalJavaPath.trim());
        setGlobalJavaPathInspected(`Java ${info.version} (${info.arch ?? 'n/a'}) — ${info.version_string}`);
      }
      await ts.update(SETTINGS.javaPath, globalJavaPath.trim() || null);
      showToast('Java path saved.', 'success');
    } catch (e) {
      setGlobalJavaPathError(formatError(e));
    }
  };

  const handleGlobalJavaPathClear = async () => {
    setGlobalJavaPath('');
    setGlobalJavaPathInspected(null);
    setGlobalJavaPathError(null);
    try {
      await ts.update(SETTINGS.javaPath, null);
      showToast('Java path cleared.', 'success');
    } catch (e) {
      showToast(formatError(e), 'error');
    }
  };

  const handleDownloadJava = async (major: number) => {
    setJavaDownloadBusy(major);
    setJavaDownloadProgress(null);
    setJavaDownloadPercent(null);
    setJavaCancelling(false);
    try {
      await ensureJavaRuntime(major, `settings-java-${major}`);
      setJavaDownloadProgress(`Java ${major} downloaded successfully.`);
      setJavaDownloadPercent(100);
      await refreshJavaRuntimes();
    } catch (e) {
      setJavaDownloadProgress(`Failed: ${formatError(e)}`);
    } finally {
      setJavaDownloadBusy(null);
    }
  };

  const handleCancelJavaDownload = async (major: number) => {
    setJavaCancelling(true);
    try {
      await cancelJavaRuntime(`settings-java-${major}`);
      setJavaDownloadProgress('Cancelling…');
    } catch {
      // Operation may already be done — ignore
    }
  };

  const handleRemoveUnusedJava = async () => {
    if (!window.confirm('Remove unused managed Java runtimes? Only the newest runtime per major version will be kept.')) return;
    setJavaRemoveBusy(true);
    try {
      const removed = await removeUnusedJavaRuntimes();
      showToast(`Removed ${removed} unused runtime${removed === 1 ? '' : 's'}.`, 'success');
      await refreshJavaRuntimes();
    } catch (e) {
      showToast(formatError(e), 'error');
    } finally {
      setJavaRemoveBusy(false);
    }
  };

  const toggleAlwaysPreTouch = async (value: boolean) => {
    setAlwaysPreTouch(value);
    try {
      await ts.update(SETTINGS.alwaysPreTouch, value);
    } catch (e) {
      setAlwaysPreTouch(!value);
      showToast(formatError(e), 'error');
    }
  };

  // --- Launcher path actions ---

  const clearLauncherPathFeedback = () => {
    setLauncherPathError(null);
    setLauncherPathSuccess(null);
  };

  const handleBrowseLauncher = async () => {
    clearLauncherPathFeedback();
    try {
      const chosen = await pickOpenFile('Select Mojang Launcher executable', ['.exe']);
      if (chosen) {
        setLauncherPath(chosen);
      }
    } catch (e) {
      setLauncherPathError(formatError(e));
    }
  };

  const handleAutoDetectLauncher = async () => {
    clearLauncherPathFeedback();
    setLauncherPathDetecting(true);
    try {
      const detected = await detectMojangLauncher();
      setLauncherPath(detected);
      setLauncherPathSuccess(`Detected: ${detected}`);
    } catch (e) {
      setLauncherPathError(`Auto-detect failed: ${formatError(e)}`);
    } finally {
      setLauncherPathDetecting(false);
    }
  };

  const handleTestLauncherPath = async () => {
    clearLauncherPathFeedback();
    if (!launcherPath.trim()) {
      setLauncherPathError('No launcher path entered. Type a path or use Auto-detect.');
      return;
    }
    setLauncherPathTesting(true);
    try {
      await testLauncherPath(launcherPath.trim());
      setLauncherPathSuccess('Path is valid.');
    } catch (e) {
      setLauncherPathError(`Test failed: ${formatError(e)}`);
    } finally {
      setLauncherPathTesting(false);
    }
  };

  // --- MCP helpers ---

  const handleStartServer = async () => {
    try {
      const status = await startMcpServer();
      setMcpStatus(status);
    } catch (e) {
      showToast(formatError(e), 'error');
    }
  };

  const handleStopServer = async () => {
    try {
      await stopMcpServer();
      await fetchMcpStatus();
    } catch (e) {
      showToast(formatError(e), 'error');
    }
  };

  const handleApprovalChange = async (instanceId: string, _tool: string, state: string) => {
    try {
      await setMcpApproval(_tool, instanceId, state);
    } catch (e) {
      showToast(formatError(e), 'error');
    }
  };

  const handleCopilotLogout = async () => {
    try {
      await copilotLogout();
      setCopilotToken(null);
    } catch (e) {
      showToast(formatError(e), 'error');
    }
  };

  return (
    <div className="space-y-6">
      <section>
        <h2 className="text-2xl font-bold mb-2">⚙️ Settings</h2>
        <p className="text-muted-foreground">
          Integration toggles, launcher path, and application preferences.
        </p>
      </section>

      {/* Language Selector — commented out: i18n deferred post-v1 */}

      {/* Advanced Mode Toggle */}
      <div className="rounded-xl border border-border bg-card p-4 space-y-3">
        <h3 className="font-semibold">Advanced mode</h3>
        <label className="flex items-center justify-between">
          <span className="text-sm">Show advanced settings</span>
          <input
            type="checkbox"
            checked={advancedMode}
            onChange={toggleAdvanced}
            className="h-5 w-5 accent-brand-600"
          />
        </label>
        <p className="text-xs text-muted-foreground">
          Reveal JVM arguments, garbage collector settings, custom commands, and other power-user options.
        </p>
      </div>

      {loading ? (
        <p className="text-muted-foreground">Loading settings…</p>
      ) : (
        <>
          {/* Inline error banner for any setting that failed to load */}
          {Object.keys(ts.errors).length > 0 && (
            <div className="rounded-xl border border-destructive/30 bg-destructive/5 p-3 space-y-1">
              <p className="text-xs font-semibold text-destructive">Some settings failed to load</p>
              {Object.entries(ts.errors).map(([key, err]) => (
                <p key={key} className="text-xs text-destructive/80">
                  <code className="bg-destructive/10 px-1 rounded">{key}</code>: {err}
                </p>
              ))}
            </div>
          )}

          <div className="rounded-xl border border-border bg-card p-4 space-y-4">
            <h3 className="font-semibold">External Services</h3>

            <label className="flex items-center justify-between">
              <span className="text-sm">Modrinth Access</span>
              <input
                type="checkbox"
                checked={modrinth}
                onChange={(e) => toggleModrinth(e.target.checked)}
                className="h-5 w-5 accent-brand-600"
              />
            </label>
            <p className="text-xs text-muted-foreground">
              Allow live Modrinth API queries and show Modrinth-sourced curated mods.
            </p>
            {ts.statuses['modrinth_enabled']?.status === 'error' && (
              <p className="text-xs text-destructive">{ts.statuses['modrinth_enabled']?.error}</p>
            )}

            
            <label className="flex items-center justify-between pt-2 border-t border-border">
              <div>
                <span className="text-sm">Integrated AI Assistant</span>
                <p className="text-xs text-muted-foreground mt-0.5">
                  Built-in AI chat powered by GitHub Copilot. Free with your GitHub account — no separate API key needed. Use this for quick crash analysis and mod questions.
                </p>
              </div>
              <input
                type="checkbox"
                checked={aiChatEnabled}
                onChange={(e) => toggleAiChat(e.target.checked)}
                className="h-5 w-5 accent-brand-600"
              />
            </label>
            {ts.statuses['ai_chat_enabled']?.status === 'error' && (
              <p className="text-xs text-destructive">{ts.statuses['ai_chat_enabled']?.error}</p>
            )}
             {(aiMcp || aiChatEnabled) && (
              <div className="rounded-lg bg-muted p-3 space-y-2">
                <h4 className="text-xs font-semibold">Two ways to use AI with Agora</h4>
                <p className="text-xs text-muted-foreground">
                  <strong>MCP Server</strong> — Lets your external AI tool (Claude Desktop, Kilo Code, Opencode, etc.) control Agora directly. The agent can list instances, disable mods, and analyze crashes on its own. Best for users who already have an AI agent set up. No cost — uses your agent's AI provider.
                </p>
                <p className="text-xs text-muted-foreground">
                  <strong>Integrated AI</strong> — A built-in chat in Agora powered by GitHub Copilot. Quick questions, crash analysis, mod help. 50 free chats/month with your GitHub account.
                </p>
              </div>
            )}

            {aiChatEnabled && (
              <div className="pt-2 border-t border-border space-y-3">
                <div className="space-y-1">
                  <label className="text-sm font-medium">GitHub Copilot</label>
                  {copilotLoading ? (
                    <p className="text-xs text-muted-foreground">Checking connection…</p>
                  ) : copilotToken ? (
                    <div className="flex items-center gap-2">
                      <span className="text-xs text-green-600 dark:text-green-400">● Connected as {copilotToken.username} ({copilotToken.plan})</span>
                      <button
                        onClick={handleCopilotLogout}
                        className="text-xs text-muted-foreground hover:text-foreground underline"
                      >
                        Sign out
                      </button>
                    </div>
                  ) : (
                    <p className="text-xs text-muted-foreground">
                      Not connected. Open the AI Assistant chat and click "Connect with GitHub" to activate. 50 free chats/month with any GitHub account.
                    </p>
                  )}
                </div>
                <p className="text-xs text-muted-foreground">
                  GitHub Copilot provides free AI diagnostics — no API key needed. For higher limits or custom models, connect an external AI agent via the MCP server above.
                </p>
              </div>
            )}
            
            <label className="flex items-center justify-between pt-2 border-t border-border">
              <span className="text-sm">AI / MCP Server</span>
              <input
                type="checkbox"
                checked={aiMcp}
                onChange={(e) => toggleAiMcp(e.target.checked)}
                className="h-5 w-5 accent-brand-600"
              />
            </label>
            <p className="text-xs text-muted-foreground">
              Enable the local MCP server for external AI tools.
            </p>
            {ts.statuses['ai_mcp_enabled']?.status === 'error' && (
              <p className="text-xs text-destructive">{ts.statuses['ai_mcp_enabled']?.error}</p>
            )}


            {advancedMode && aiMcp && (
              <div className="pt-2 border-t border-border space-y-3">
                {/* MCP Status */}
                <div className="rounded-lg bg-muted px-3 py-2.5 space-y-2">
                  <div className="flex items-center justify-between">
                    <div className="flex items-center gap-2">
                      <span className={`inline-block h-2.5 w-2.5 rounded-full ${mcpStatus?.running ? 'bg-green-500' : 'bg-gray-400'}`} />
                      <span className="text-sm">
                        {mcpStatus?.running ? (
                          <>
                            Server running on{' '}
                            <code className="text-xs bg-muted px-1.5 py-0.5 rounded">
                              http://127.0.0.1:39741/sse
                            </code>
                          </>
                        ) : (
                          'Server stopped'
                        )}
                      </span>
                    </div>
                    <div className="flex items-center gap-1.5">
                      <button
                        onClick={() => fetchMcpStatus()}
                        className="rounded-md border border-input px-2.5 py-1 text-xs font-medium hover:bg-accent"
                      >
                        Refresh
                      </button>
                      {mcpStatus?.running ? (
                        <button
                          onClick={handleStopServer}
                          className="rounded-md bg-destructive px-2.5 py-1 text-xs font-medium text-destructive-foreground hover:bg-destructive/90"
                        >
                          Stop Server
                        </button>
                      ) : (
                        <button
                          onClick={handleStartServer}
                          className="rounded-md bg-primary px-2.5 py-1 text-xs font-medium text-white hover:bg-brand-700"
                        >
                          Start Server
                        </button>
                      )}
                    </div>
                  </div>
                </div>

                {/* Approval Settings */}
                <div className="rounded-lg bg-muted px-3 py-2.5 space-y-2">
                  <h4 className="text-sm font-semibold">Approval Settings (per instance)</h4>
                  <p className="text-xs text-muted-foreground">
                    Tool: disable_mod — controls whether external AI tools can disable mods without prompting.
                  </p>
              
                  {mcpInstances.length === 0 ? (
                    <p className="text-xs text-muted-foreground">No instances found.</p>
                  ) : (
                    <div className="space-y-1.5">
                      {mcpInstances.map((inst) => {
                        const current = instanceApprovals[inst.instance_id] || 'always_deny';
                        return (
                          <div key={inst.instance_id} className="flex items-center justify-between gap-2">
                            <span className="text-xs truncate flex-1" title={inst.name || inst.instance_id}>
                              {inst.name || inst.instance_id}
                            </span>
                            <select
                              value={current}
                              onChange={(e) => {
                                setInstanceApprovals((prev) => ({ ...prev, [inst.instance_id]: e.target.value }));
                                handleApprovalChange(inst.instance_id, 'disable_mod', e.target.value);
                              }}
                              className="rounded-md border border-input bg-background px-2 py-1 text-xs"
                            >
                              <option value="always_deny">Deny (default)</option>
                              <option value="always_allow">Always Allow</option>
                            </select>
                          </div>
                        );
                      })}
                    </div>
                  )}
                </div>

                {/* Bearer Token */}
                <div className="rounded-lg bg-muted px-3 py-2.5 space-y-2">
                  <h4 className="text-sm font-semibold">Bearer Token</h4>
                  <p className="text-xs text-muted-foreground">
                    All MCP connections require this token. Present it as <code className="bg-muted px-1 py-0.5 rounded">Authorization: Bearer &lt;token&gt;</code> header or <code className="bg-muted px-1 py-0.5 rounded">?token=&lt;token&gt;</code> query parameter.
                  </p>
                  {mcpToken?.token ? (
                    <>
                      <TokenDisplay token={mcpToken.token} />
                      <div className="flex flex-wrap gap-1.5">
                        <CopyButton text={mcpToken.token} label="Copy token" />
                        <CopyButton text={mcpToken.config_snippet} label="Copy MCP config" />
                        <button
                          onClick={async () => {
                            if (!window.confirm('Regenerate token? This invalidates the current token. All AI clients must be updated.')) return;
                            try {
                              const data = await regenerateMCPToken();
                              setMcpToken(data);
                              showToast('Token regenerated successfully.', 'success');
                            } catch (e) {
                              showToast(formatError(e), 'error');
                            }
                          }}
                          className="rounded-md border border-input px-2.5 py-1 text-xs font-medium hover:bg-accent"
                        >
                          Regenerate token
                        </button>
                      </div>
                      <p className="text-xs text-amber-600 dark:text-amber-400">
                        Regenerating invalidates the previous token — all AI clients must be updated with the new one.
                      </p>
                    </>
                  ) : (
                    <p className="text-xs text-muted-foreground">
                      Start the MCP server to generate a token.
                    </p>
                  )}
                  {advancedMode && (
                    <p className="text-xs text-muted-foreground">
                      Token path: <code className="bg-muted px-1 py-0.5 rounded">{'<app_data>/mcp_token'}</code>
                    </p>
                  )}
                </div>

                {/* Connect your AI tool */}
                <details className="rounded-lg bg-muted px-3 py-2.5 space-y-3">
                  <summary className="text-sm font-semibold cursor-pointer select-none">Connect your AI tool</summary>

                  {/* Section 1: Kilo Code */}
                  <div className="space-y-1.5">
                    <h5 className="text-xs font-semibold">Kilo Code (VS Code extension)</h5>
                    <ol className="list-decimal list-inside text-xs text-muted-foreground space-y-0.5">
                      <li>Add the config below to <code className="bg-muted px-1 py-0.5 rounded">.kilo/kilo.json</code> (project root or <code className="bg-muted px-1 py-0.5 rounded">~/.config/kilo/kilo.json</code>).</li>
                      <li>Copy the skill (button below) to <code className="bg-muted px-1 py-0.5 rounded">.kilo/skills/agora-mcp/SKILL.md</code>.</li>
                      <li>Restart VS Code.</li>
                    </ol>
                    <div className="relative">
                      <pre className="text-xs bg-muted rounded-lg p-3 overflow-x-auto text-muted-foreground">{"{\n  \"mcp\": {\n    \"agora-mc\": {\n      \"type\": \"remote\",\n      \"url\": \"http://127.0.0.1:39741/sse\",\n      \"enabled\": true\n    }\n  }\n}"}</pre>
                      <div className="absolute top-2 right-2">
                        <CopyButton
                          text={`{\n  "mcp": {\n    "agora-mc": {\n      "type": "remote",\n      "url": "http://127.0.0.1:39741/sse",\n      "enabled": true\n    }\n  }\n}`}
                          label="Copy"
                        />
                      </div>
                    </div>
                  </div>

                  {/* Section 2: Opencode */}
                  <div className="space-y-1.5 pt-2 border-t border-border">
                    <h5 className="text-xs font-semibold">Opencode</h5>
                    <ol className="list-decimal list-inside text-xs text-muted-foreground space-y-0.5">
                      <li>
                        Add the config below to{' '}
                        <code className="bg-muted px-1 py-0.5 rounded">
                          .opencode/opencode.json
                        </code>
                        {' '}or{' '}
                        <code className="bg-muted px-1 py-0.5 rounded">
                          ~/.config/opencode/opencode.json (C:Users\[User]\.config\opencode)
                        </code>
                        .
                      </li>
                      <li>Copy the skill (button below) to <code className="bg-muted px-1 py-0.5 rounded">.opencode/skills/agora-mcp/SKILL.md</code>.</li>
                      <li>Restart Opencode.</li>
                    </ol>
                    <div className="relative">
                      <pre className="text-xs bg-muted rounded-lg p-3 overflow-x-auto text-muted-foreground">{"{\n  \"mcp\": {\n    \"agora-mc\": {\n      \"type\": \"remote\",\n      \"url\": \"http://127.0.0.1:39741/sse\",\n      \"enabled\": true\n    }\n  }\n}"}</pre>
                      <div className="absolute top-2 right-2">
                        <CopyButton
                          text={`{\n  "mcp": {\n    "agora-mc": {\n      "type": "remote",\n      "url": "http://127.0.0.1:39741/sse",\n      "enabled": true\n    }\n  }\n}`}
                          label="Copy"
                        />
                      </div>
                    </div>
                  </div>

                  {/* Section 3: Claude Desktop */}
                  <div className="space-y-1.5 pt-2 border-t border-border">
                    <h5 className="text-xs font-semibold">Claude Desktop</h5>
                    <ol className="list-decimal list-inside text-xs text-muted-foreground space-y-0.5">
                      <li>
                        Add the config below to{' '}
                        <code className="bg-muted px-1 py-0.5 rounded">
                          {isWindows
                            ? '%APPDATA%\\Claude\\claude_desktop_config.json'
                            : '~/Library/Application Support/Claude/claude_desktop_config.json'}
                        </code>
                        .
                      </li>
                      <li>Restart Claude Desktop.</li>
                    </ol>
                    <div className="relative">
                      <pre className="text-xs bg-muted rounded-lg p-3 overflow-x-auto text-muted-foreground">{"{\n  \"mcpServers\": {\n    \"agora\": {\n      \"url\": \"http://127.0.0.1:39741/sse\",\n      \"transport\": \"sse\"\n    }\n  }\n}"}</pre>
                      <div className="absolute top-2 right-2">
                        <CopyButton
                          text={`{\n  "mcpServers": {\n    "agora": {\n      "url": "http://127.0.0.1:39741/sse",\n      "transport": "sse"\n    }\n  }\n}`}
                          label="Copy"
                        />
                      </div>
                    </div>
                  </div>

                  {/* Section 4: Other MCP clients */}
                  <div className="space-y-1 pt-2 border-t border-border">
                    <h5 className="text-xs font-semibold">Other MCP clients</h5>
                    <div className="text-xs text-muted-foreground space-y-0.5">
                      <p>Server URL: <code className="bg-muted px-1 py-0.5 rounded">http://127.0.0.1:39741/sse</code></p>
                      <p>Transport: <code className="bg-muted px-1 py-0.5 rounded">SSE (Server-Sent Events)</code></p>
                      <p>Authentication: Bearer token (see above)</p>
                      <p>If you get stuck, your AI agent might be able to help you troubleshoot/customize the MCP integration.</p>
                    </div>
                  </div>

                  {/* Section 5: Skill content */}
                  <div className="space-y-1.5 pt-2 border-t border-border">
                    <h5 className="text-xs font-semibold">Skill content</h5>
                    <p className="text-xs text-muted-foreground">
                      The skill teaches your AI agent what the 6 Agora tools do and when to use them. Place it in your agent's skills directory.
                    </p>
                    <div className="flex gap-2">
                      <button
                        onClick={async () => {
                          if (!skillContent) return;
                          await navigator.clipboard.writeText(skillContent);
                          setSkillCopied(true);
                          setTimeout(() => setSkillCopied(false), 2000);
                        }}
                        disabled={!skillContent || skillLoading}
      className="rounded-md bg-primary px-2.5 py-1 text-xs font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
                      >
                        {skillCopied ? 'Copied!' : 'Copy Skill to Clipboard'}
                      </button>
                      <button
                        onClick={async () => {
                          if (!skillContent) return;
                          setSkillLoading(true);
                          try {
                            const blob = new Blob([skillContent], { type: 'text/markdown' });
                            const url = URL.createObjectURL(blob);
                            const a = document.createElement('a');
                            a.href = url;
                            a.download = 'SKILL.md';
                            a.click();
                            URL.revokeObjectURL(url);
                          } finally {
                            setSkillLoading(false);
                          }
                        }}
                        disabled={!skillContent || skillLoading}
                        className="rounded-md border border-input px-2.5 py-1 text-xs font-medium hover:bg-accent disabled:opacity-50"
                      >
                        {skillLoading ? 'Downloading…' : 'Download SKILL.md'}
                      </button>
                    </div>
                  </div>
                </details>
              </div>
            )}
          </div>

          {/* GitHub Account */}
          <div className="rounded-xl border border-border bg-card p-4 space-y-3">
            <h3 className="font-semibold">GitHub Account</h3>
            {githubLoading ? (
              <p className="text-xs text-muted-foreground">Checking connection…</p>
            ) : githubAuth ? (
              <div className="space-y-2">
                <div className="flex items-center gap-2">
                  {githubProfile?.avatar_url && (
                    <img
                      src={githubProfile.avatar_url}
                      alt=""
                      className="h-6 w-6 rounded-full"
                    />
                  )}
                  <span className="text-sm text-green-600 dark:text-green-400">
                    ● Signed in as <strong>{githubProfile?.login ?? 'GitHub user'}</strong>
                  </span>
                </div>
                <p className="text-xs text-muted-foreground">
                  Used for community governance (voting, proposals).
                </p>
                <button
                  onClick={handleGithubSignOut}
                  className="text-xs text-muted-foreground hover:text-foreground underline"
                >
                  Sign out
                </button>
              </div>
            ) : (
              <div className="space-y-3">
                <p className="text-xs text-muted-foreground">
                  Sign in with GitHub to participate in community governance — voting on mod inclusions, proposals, and more. This is optional.
                </p>

                {ghDevice && (
                  <DeviceFlowPanel
                    device={ghDevice}
                    polling={ghPolling}
                    onCancel={() => {
                      ghSessionRef.current += 1;
                      setGhPolling(false);
                      setGhDevice(null);
                    }}
                  />
                )}

                {ghResult && <p className="text-sm text-primary">{ghResult}</p>}
                {ghError && <p className="text-xs text-destructive">{ghError}</p>}

                <button
                  onClick={handleGithubSignIn}
                  disabled={ghPolling}
                  className="rounded-lg bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
                >
                  {ghPolling ? 'Waiting…' : 'Sign in with GitHub'}
                </button>
              </div>
            )}
          </div>

          {/* AI Model Selector
          <div className="rounded-xl border border-border bg-card p-4 space-y-3">
            <h3 className="font-semibold">AI Assistant</h3>
            {modelLoading ? (
              <p className="text-xs text-muted-foreground">Loading models…</p>
            ) : (
              <>
                <div className="space-y-1">
                  <label htmlFor="ai-model-select" className="text-sm">
                    Model
                  </label>
                  <select
                    id="ai-model-select"
                    value={selectedModel}
                    onChange={(e) => handleModelChange(e.target.value)}
                    className="w-full rounded-lg border border-input bg-background px-3 py-2 text-sm"
                  >
                    {aiModels.map((m) => (
                      <option key={m.id} value={m.id}>
                        {m.name}
                      </option>
                    ))}
                  </select>
                </div>
                <p className="text-xs text-muted-foreground">
                  GPT-4.1 Mini is recommended — free (limited usage from GitHub), fast, and probably good enough for crash diagnosis. GPT-4.1 is also available for free and offers a bit more intelligence, but with less available usage. Both models are free with your GitHub account.
                </p>
                <p className="text-xs text-muted-foreground">
                  For newer, smarter, more advanced AI with much higher usage limits and more capabilities to customize Agora, connect an AI agent like Claude Code, Codex, Opencode or countless others via the MCP server above. If you're curious, my personal recommendation is Opencode desktop, which is free, open-source, includes a few free models, and is fairly easy to use, though almost any agent will work for Agora. I personally use Kilo Code (VS Code extension) for Agora development.
                </p>
              </>
            )}
          </div> */}

          {/* Microsoft Account */}
          <div className="rounded-xl border border-border bg-card p-4 space-y-3">
            <h3 className="font-semibold">Microsoft Account</h3>
            {msaLoading ? (
              <p className="text-xs text-muted-foreground">Checking connection…</p>
            ) : msaCreds ? (
              <div className="space-y-2">
                <div className="flex items-center gap-2">
                  <span className="text-sm text-green-600 dark:text-green-400">
                    ● Signed in as <strong>{msaCreds.username}</strong>
                  </span>
                </div>
                <p className="text-xs text-muted-foreground">
                  UUID: {msaCreds.uuid}<br />
                  Expires: {msaCreds.expires}
                </p>
                <p className="text-xs text-muted-foreground">
                  Required for direct launch mode. Used to authenticate with Minecraft services.
                </p>
                <button
                  onClick={handleMsaSignOut}
                  className="text-xs text-muted-foreground hover:text-foreground underline"
                >
                  Sign out
                </button>
              </div>
            ) : (
              <div className="space-y-3">
                <p className="text-xs text-muted-foreground">
                  Sign in with your Microsoft account to enable direct in-app launching (without the Mojang launcher).
                </p>

                {msaError && <p className="text-xs text-destructive">{msaError}</p>}
                <button
                  onClick={handleMsaSignIn}
                  disabled={msaBusy}
                  className="rounded-lg bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
                >
                  {msaBusy ? 'Signing in…' : 'Sign in with Microsoft'}
                </button>
              </div>
            )}
          </div>

          {/* Launch Mode */}
          <div className="rounded-xl border border-border bg-card p-4 space-y-3">
            <h3 className="font-semibold">Launch Mode</h3>
            <label className="flex items-center justify-between">
              <span className="text-sm">Use in-app launcher (direct Java launch)</span>
              <input
                type="checkbox"
                checked={directLaunch}
                onChange={(e) => toggleLaunchMode(e.target.checked)}
                className="h-5 w-5 accent-brand-600"
              />
            </label>
            <p className="text-xs text-muted-foreground">
              <strong>Off (default):</strong> Delegates to the official Mojang launcher — handles auth and JVM execution.
              The Mojang launcher opens with your instance pre-selected via <code className="bg-muted px-1 py-0.5 rounded">--profile</code>.
            </p>
            <p className="text-xs text-muted-foreground">
              <strong>On:</strong> Agora launches Minecraft directly — shows game console output in-app and gives you more control. Requires a Microsoft Account sign-in above for full online play.
            </p>
            <p className="text-xs text-muted-foreground">
              Mojang Metadata, Mojang Content, and Modloader Metadata &amp; Content are <strong>enabled by default</strong> under <strong>Privacy → Launch</strong>. Once files are cached, installed instances can launch with those categories disabled.
            </p>
            {ts.statuses['launch_mode']?.status === 'error' && (
              <p className="text-xs text-destructive">{ts.statuses['launch_mode']?.error}</p>
            )}

          </div>

          {/* Java Runtime Management */}
          <div className="rounded-xl border border-border bg-card p-4 space-y-4">
            <h3 className="font-semibold">Java Runtime Management</h3>
            <p className="text-xs text-muted-foreground">
              Agora can automatically download and manage Java runtimes for Minecraft.
              Managed runtimes are stored in private app-data and never modify your system PATH.
              Each instance uses the exact major version required by the selected Minecraft version.
            </p>

            {/* Runtime mode selector */}
            <div className="space-y-2">
              <label className="text-sm font-medium">Java runtime mode</label>
              <div className="flex flex-wrap gap-2">
                {(['automatic', 'prompt', 'manual'] as const).map((mode) => (
                  <button
                    key={mode}
                    onClick={() => handleJavaRuntimeModeChange(mode)}
                    className={[
                      'rounded-lg border px-3 py-1.5 text-xs font-medium transition-colors',
                      javaRuntimeMode === mode
                        ? 'border-primary bg-primary text-primary-foreground'
                        : 'border-input hover:bg-accent',
                    ].join(' ')}
                  >
                    {mode === 'automatic' ? 'Automatic (recommended)'
                      : mode === 'prompt' ? 'Prompt'
                      : 'Manual'}
                  </button>
                ))}
              </div>
              <p className="text-xs text-muted-foreground">
                {javaRuntimeMode === 'automatic'
                  ? 'Automatically provision the required Java runtime when launching.'
                  : javaRuntimeMode === 'prompt'
                    ? 'Prompt the user when a required Java runtime is missing.'
                    : 'Do not download runtimes. Only use user-specified and system Java installations.'}
              </p>
            </div>

            {/* Global Java path override */}
            <div className="space-y-2">
              <label className="text-sm font-medium">Global Java executable (override)</label>
              <div className="flex gap-2">
                <input
                  value={globalJavaPath}
                  onChange={(e) => {
                    setGlobalJavaPath(e.target.value);
                    setGlobalJavaPathInspected(null);
                    setGlobalJavaPathError(null);
                  }}
                  placeholder="Leave empty to auto-detect"
                  className="flex-1 rounded-lg border border-input bg-background px-3 py-2 text-sm"
                />
                <button
                  onClick={handleGlobalJavaPathBrowse}
                  className="rounded-lg border border-input px-3 py-2 text-sm font-medium hover:bg-accent"
                >
                  Browse…
                </button>
              </div>
              <div className="flex gap-2">
                <button
                  onClick={handleGlobalJavaPathSave}
                  disabled={!globalJavaPath.trim()}
                  className="rounded-lg bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
                >
                  Save
                </button>
                <button
                  onClick={handleGlobalJavaPathClear}
                  disabled={!globalJavaPath}
                  className="rounded-lg border border-input px-3 py-1.5 text-xs font-medium hover:bg-accent disabled:opacity-50"
                >
                  Clear
                </button>
              </div>
              {globalJavaPathInspected && (
                <p className="text-xs text-green-600 dark:text-green-400">{globalJavaPathInspected}</p>
              )}
              {globalJavaPathError && (
                <p className="text-xs text-destructive">{globalJavaPathError}</p>
              )}
            </div>

            {/* Download buttons */}
            <div className="space-y-2">
              <label className="text-sm font-medium">Download managed runtimes</label>
              <div className="flex flex-wrap items-center gap-2">
                {[8, 17, 21].map((major) => (
                  <button
                    key={major}
                    onClick={() => handleDownloadJava(major)}
                    disabled={javaDownloadBusy !== null}
                    className="rounded-lg bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
                  >
                    {javaDownloadBusy === major ? `Downloading Java ${major}…` : `Download Java ${major}`}
                  </button>
                ))}
                {/* Custom major input */}
                <div className="flex items-center gap-1">
                  <input
                    type="number"
                    min="8"
                    max="25"
                    value={customMajorInput}
                    onChange={(e) => setCustomMajorInput(e.target.value)}
                    placeholder="major"
                    className="w-16 rounded-lg border border-input bg-background px-2 py-1.5 text-xs"
                  />
                  <button
                    onClick={() => {
                      const m = parseInt(customMajorInput, 10);
                      if (m >= 8 && m <= 25) handleDownloadJava(m);
                    }}
                    disabled={javaDownloadBusy !== null || !customMajorInput}
                    className="rounded-lg border border-input px-2 py-1.5 text-xs font-medium hover:bg-accent disabled:opacity-50"
                  >
                    Download
                  </button>
                </div>
              </div>
              {javaDownloadProgress && javaDownloadBusy !== null && (
                <div className="space-y-1.5">
                  <div className="flex items-center gap-2">
                    <div className="flex-1 h-2 bg-muted rounded-full overflow-hidden">
                      <div
                        className="h-full bg-primary rounded-full transition-all duration-300"
                        style={{ width: `${Math.min(javaDownloadPercent ?? 0, 100)}%` }}
                      />
                    </div>
                    <button
                      onClick={() => handleCancelJavaDownload(javaDownloadBusy!)}
                      disabled={javaCancelling}
                      className="rounded-lg border border-border px-2 py-1 text-xs font-medium hover:bg-accent disabled:opacity-50 shrink-0"
                    >
                      {javaCancelling ? 'Cancelling…' : 'Cancel'}
                    </button>
                  </div>
                  <p className="text-xs text-muted-foreground">{javaDownloadProgress}</p>
                </div>
              )}
              {javaDownloadProgress && javaDownloadBusy === null && (
                <p className="text-xs text-muted-foreground">{javaDownloadProgress}</p>
              )}
            </div>

            {/* Runtime table */}
            <div className="space-y-2">
              <div className="flex items-center justify-between">
                <label className="text-sm font-medium">Detected Java runtimes</label>
                <div className="flex gap-2">
                  <button
                    onClick={refreshJavaRuntimes}
                    disabled={javaRuntimesLoading}
                    className="rounded-lg border border-input px-2.5 py-1 text-xs font-medium hover:bg-accent disabled:opacity-50"
                  >
                    {javaRuntimesLoading ? 'Refreshing…' : 'Refresh'}
                  </button>
                  <button
                    onClick={handleRemoveUnusedJava}
                    disabled={javaRemoveBusy || javaRuntimesLoading}
                    className="rounded-lg border border-input px-2.5 py-1 text-xs font-medium hover:bg-accent disabled:opacity-50"
                  >
                    {javaRemoveBusy ? 'Removing…' : 'Remove unused'}
                  </button>
                </div>
              </div>
              {javaRuntimesError && (
                <p className="text-xs text-destructive">{javaRuntimesError}</p>
              )}
              {javaRuntimesLoading ? (
                <p className="text-xs text-muted-foreground">Scanning for Java runtimes…</p>
              ) : javaRuntimes.length === 0 ? (
                <p className="text-xs text-muted-foreground">No Java runtimes detected.</p>
              ) : (
                <div className="max-h-48 overflow-y-auto rounded-lg border border-border">
                  <table className="w-full text-xs">
                    <thead className="bg-muted/50">
                      <tr>
                        <th className="px-3 py-1.5 text-left font-medium">Source</th>
                        <th className="px-3 py-1.5 text-left font-medium">Version</th>
                        <th className="px-3 py-1.5 text-left font-medium">Arch</th>
                        <th className="px-3 py-1.5 text-left font-medium">Path</th>
                      </tr>
                    </thead>
                    <tbody>
                      {javaRuntimes.map((rt, idx) => (
                        <tr key={idx} className="border-t border-border">
                          <td className="px-3 py-1.5">
                            <span className={[
                              'inline-block rounded-full px-1.5 py-0.5 text-[10px] font-medium',
                              rt.source === 'Managed' ? 'bg-brand-600/10 text-brand-600 dark:text-brand-400'
                                : rt.source === 'Mojang' ? 'bg-blue-500/10 text-blue-600 dark:text-blue-400'
                                : rt.source === 'System' ? 'bg-green-500/10 text-green-600 dark:text-green-400'
                                : 'bg-muted text-muted-foreground',
                            ].join(' ')}>
                              {rt.source}
                            </span>
                          </td>
                          <td className="px-3 py-1.5 font-medium">{rt.version_string || `Java ${rt.version}`}</td>
                          <td className="px-3 py-1.5">{rt.arch ?? '—'}</td>
                          <td className="px-3 py-1.5 text-muted-foreground truncate max-w-[200px]" title={rt.path}>
                            {rt.path}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </div>
          </div>

          <div className="rounded-xl border border-border bg-card p-4 space-y-3">
            <h3 className="font-semibold">Launcher Path</h3>
            <input
              value={launcherPath}
              onChange={(e) => {
                setLauncherPath(e.target.value);
                clearLauncherPathFeedback();
              }}
              placeholder="Auto-discovered if empty"
              className="w-full rounded-lg border border-input bg-background px-3 py-2 text-sm"
            />
            <div className="flex flex-wrap gap-2">
              <button
                onClick={saveLauncherPath}
                className="rounded-lg bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90"
              >
                Save
              </button>
              <button
                onClick={handleBrowseLauncher}
                className="rounded-lg border border-input px-4 py-2 text-sm font-medium hover:bg-accent"
              >
                Browse…
              </button>
              <button
                onClick={handleAutoDetectLauncher}
                disabled={launcherPathDetecting}
                className="rounded-lg border border-input px-4 py-2 text-sm font-medium hover:bg-accent disabled:opacity-50"
              >
                {launcherPathDetecting ? 'Detecting…' : 'Auto-detect'}
              </button>
              <button
                onClick={handleTestLauncherPath}
                disabled={launcherPathTesting || !launcherPath.trim()}
                className="rounded-lg border border-input px-4 py-2 text-sm font-medium hover:bg-accent disabled:opacity-50"
              >
                {launcherPathTesting ? 'Testing…' : 'Test'}
              </button>
            </div>
            {launcherPathError && (
              <p className="text-xs text-destructive">{launcherPathError}</p>
            )}
            {launcherPathSuccess && (
              <p className="text-xs text-green-600 dark:text-green-400">{launcherPathSuccess}</p>
            )}
            {/* Show persistent setting error from typed settings */}
            {ts.statuses['mojang_launcher_path']?.status === 'error' && (
              <p className="text-xs text-destructive">
                Failed to save: {ts.statuses['mojang_launcher_path']?.error}
              </p>
            )}
            {ts.statuses['mojang_launcher_path']?.status === 'write-pending' && (
              <p className="text-xs text-muted-foreground">Saving…</p>
            )}
            <p className="text-xs text-muted-foreground">
              Override the official Mojang launcher executable location.
            </p>
          </div>

          <div className="rounded-xl border border-border bg-card p-4 space-y-3">
            <h3 className="font-semibold">JVM Defaults</h3>
            <label className="flex items-center justify-between">
              <span className="text-sm">AlwaysPreTouch</span>
              <input
                type="checkbox"
                checked={alwaysPreTouch}
                onChange={(e) => toggleAlwaysPreTouch(e.target.checked)}
                className="h-5 w-5 accent-brand-600"
              />
            </label>
            <p className="text-xs text-muted-foreground">
              Recommended for G1GC, may cause issues with ZGC/Shenandoah.
            </p>
            {ts.statuses['always_pre_touch']?.status === 'error' && (
              <p className="text-xs text-destructive">{ts.statuses['always_pre_touch']?.error}</p>
            )}
          </div>

          {/* <div className="rounded-xl border border-border bg-card p-4 space-y-3">
            <h3 className="font-semibold">Crash Telemetry</h3>
            <label className="flex items-center justify-between">
              <span className="text-sm">Allow anonymous crash telemetry</span>
              <input
                type="checkbox"
                checked={crashTelemetry}
                onChange={(e) => toggleCrashTelemetry(e.target.checked)}
                className="h-5 w-5 accent-brand-600"
              />
            </label>
            <p className="text-xs text-muted-foreground">
              Allow anonymous local crash telemetry to be collected for mod-incompatibility research. Aggregates are never uploaded unless you opt in. Saying no disables all telemetry.
            </p>
            <p className="text-xs text-muted-foreground mt-2">
              Local crash learning (mod isolation & co-crash detection) runs automatically and never leaves your machine. This toggle only controls future anonymous aggregate sharing, which is not yet active.
            </p>
          </div> */}

          <div className="rounded-xl border border-border bg-card p-4 space-y-3">
            <h3 className="font-semibold">Software Updates</h3>
            <button
              onClick={async () => {
                try {
                  const update = await check();
                  if (update?.available) {
                    const ok = await window.confirm(
                      `Update available: ${update.version}\n\n${update.body ?? ''}\n\nDownload and install now?`
                    );
                    if (ok) {
                      await update.downloadAndInstall();
                      await invoke('plugin:process|restart');
                    }
                  } else {
                    showToast('You are running the latest version of Agora.', 'success');
                  }
                } catch (e) {
                  showToast(formatError(e), 'error');
                }
              }}
              className="rounded-lg bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90"
            >
              Check for Updates
            </button>
            <p className="text-xs text-muted-foreground">
              Check for new versions published to GitHub Releases. Updates are downloaded and installed automatically.
            </p>
          </div>

          {advancedMode && (
            <Privacy />
          )}
          {!advancedMode && (
            <p className="text-xs text-muted-foreground">Enable Advanced mode in Settings to see JVM, network, and MCP options.</p>
          )}
        </>
      )}
    </div>
  );
}
