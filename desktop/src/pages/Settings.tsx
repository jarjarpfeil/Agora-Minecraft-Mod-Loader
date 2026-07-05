import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { check } from '@tauri-apps/plugin-updater';
import { invoke } from '@tauri-apps/api/core';
import { open as openUrl } from '@tauri-apps/plugin-shell';
import {
  copilotStatus,
  copilotLogout,
  formatError,
  getAuthStatus,
  getGithubProfile,
  getMcpSkillContent,
  getMcpStatus,
  getSetting,
  githubLogin,
  githubLoginPoll,
  githubLogout,
  listInstances,
  setMcpApproval,
  setSetting,
  startMcpServer,
  stopMcpServer,
} from '../lib/tauri';
import type { CopilotToken, DeviceFlowResponse, GithubProfile, InstanceRow, McpStatus } from '../lib/tauri';
import { Privacy } from './Privacy';
import { useAdvancedMode } from '../components/AdvancedModeContext';

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

export function Settings() {
  const { t, i18n } = useTranslation();
  const [modrinth, setModrinth] = useState(false);
  const [aiMcp, setAiMcp] = useState(false);
  const [aiChatEnabled, setAiChatEnabled] = useState(false);
  const [launcherPath, setLauncherPath] = useState('');
  const [alwaysPreTouch, setAlwaysPreTouch] = useState(true);
  const [loading, setLoading] = useState(true);

  // MCP server state
  const [mcpStatus, setMcpStatus] = useState<McpStatus | null>(null);
  const [mcpInstances, setMcpInstances] = useState<InstanceRow[]>([]);
  const [instanceApprovals, setInstanceApprovals] = useState<Record<string, string>>({});

  // Skill content state
  const [skillContent, setSkillContent] = useState<string | null>(null);
  const [skillLoading, setSkillLoading] = useState(false);
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

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const [m, a, c, p, apt] = await Promise.all([
          getSetting('modrinth_enabled'),
          getSetting('ai_mcp_enabled'),
          getSetting('ai_chat_enabled'),
          getSetting('mojang_launcher_path'),
          getSetting('jvm_always_pre_touch'),
        ]);
        if (cancelled) return;
        setModrinth(Boolean(m));
        setAiMcp(Boolean(a));
        setAiChatEnabled(Boolean(c));
        if (typeof p === 'string') setLauncherPath(p);
        if (typeof apt === 'boolean') setAlwaysPreTouch(apt);
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
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
          } catch {
            // Profile fetch failed; auth status is still valid
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
      alert(formatError(e));
    }
  };

  const toggleModrinth = async (value: boolean) => {
    setModrinth(value);
    try {
      await setSetting('modrinth_enabled', value);
    } catch (e) {
      setModrinth(!value);
      alert(formatError(e));
    }
  };

  const toggleAiMcp = async (value: boolean) => {
    setAiMcp(value);
    try {
      await setSetting('ai_mcp_enabled', value);
    } catch (e) {
      setAiMcp(!value);
      alert(formatError(e));
    }
  };

  const toggleAiChat = async (value: boolean) => {
    setAiChatEnabled(value);
    try {
      await setSetting('ai_chat_enabled', value);
    } catch (e) {
      setAiChatEnabled(!value);
      alert(formatError(e));
    }
  };

  const saveLauncherPath = async () => {
    try {
      await setSetting('mojang_launcher_path', launcherPath);
    } catch (e) {
      alert(formatError(e));
    }
  };

  const toggleAlwaysPreTouch = async (value: boolean) => {
    setAlwaysPreTouch(value);
    try {
      await setSetting('jvm_always_pre_touch', value);
    } catch (e) {
      setAlwaysPreTouch(!value);
      alert(formatError(e));
    }
  };

  // --- MCP helpers ---

  const handleStartServer = async () => {
    try {
      await startMcpServer();
      await fetchMcpStatus();
    } catch (e) {
      alert(formatError(e));
    }
  };

  const handleStopServer = async () => {
    try {
      await stopMcpServer();
      await fetchMcpStatus();
    } catch (e) {
      alert(formatError(e));
    }
  };

  const handleApprovalChange = async (instanceId: string, _tool: string, state: string) => {
    try {
      await setMcpApproval(_tool, instanceId, state);
    } catch (e) {
      alert(formatError(e));
    }
  };

  const handleCopilotLogout = async () => {
    try {
      await copilotLogout();
      setCopilotToken(null);
    } catch (e) {
      alert(formatError(e));
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

      {/* Language Selector */}
      <div className="rounded-xl border border-border bg-card p-4 space-y-3">
        <h3 className="font-semibold">{t('language.label')}</h3>
        <label className="flex items-center justify-between">
          <span className="text-sm">{t('language.label')}</span>
          <select
            value={i18n.language}
            onChange={(e) => i18n.changeLanguage(e.target.value)}
            className="w-full rounded-lg border border-input bg-background px-3 py-2 text-sm"
          >
            <option value="en">{t('language.en')}</option>
            <option value="es">{t('language.es')}</option>
            <option value="zh">{t('language.zh')}</option>
            <option value="hi">{t('language.hi')}</option>
            <option value="bn">{t('language.bn')}</option>
            <option value="pt">{t('language.pt')}</option>
            <option value="ru">{t('language.ru')}</option>
            <option value="ja">{t('language.ja')}</option>
            <option value="ar">{t('language.ar')}</option>
            <option value="de">{t('language.de')}</option>
            <option value="ko">{t('language.ko')}</option>
            <option value="tr">{t('language.tr')}</option>
            <option value="vi">{t('language.vi')}</option>
            <option value="fr">{t('language.fr')}</option>
            <option value="ta">{t('language.ta')}</option>
            <option value="te">{t('language.te')}</option>
            <option value="ur">{t('language.ur')}</option>
            <option value="it">{t('language.it')}</option>
            <option value="nl">{t('language.nl')}</option>
            <option value="pl">{t('language.pl')}</option>
          </select>
        </label>
      </div>

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

           

            <label className="flex items-center justify-between pt-2 border-t border-border">
              <div>
                <span className="text-sm">Integrated AI Assistant</span>
                <p className="text-xs text-muted-foreground mt-0.5">
                  Built-in AI chat powered by GitHub Models (GPT-4.1 Mini). Free with your GitHub account — no separate API key needed. Use this for quick crash analysis and mod questions.
                </p>
              </div>
              <input
                type="checkbox"
                checked={aiChatEnabled}
                onChange={(e) => toggleAiChat(e.target.checked)}
                className="h-5 w-5 accent-brand-600"
              />
            </label>
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
                  <h4 className="text-sm font-semibold">Approval Settings</h4>
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
                      <p>No authentication required</p>
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
                  <div className="rounded-lg border border-border bg-muted p-3 space-y-2">
                    <p className="text-xs">Opening your browser… If it didn't open, click the button below:</p>
                    <p className="text-sm font-semibold text-primary dark:text-primary break-all">
                      {ghDevice.verification_uri}
                    </p>
                    <p className="text-sm">
                      Code:{' '}
                      <span className="font-mono font-bold tracking-widest">{ghDevice.user_code}</span>
                    </p>
                    <button
                      type="button"
                      onClick={() => {
                        openUrl(ghDevice.verification_uri).catch(() => {});
                      }}
                      disabled={ghPolling}
                      className="rounded-lg border border-border px-3 py-1.5 text-xs font-medium hover:bg-accent disabled:opacity-50"
                    >
                      Open in browser
                    </button>
                    {ghPolling && (
                      <p className="text-xs text-muted-foreground">Waiting for authorization…</p>
                    )}
                  </div>
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

          <div className="rounded-xl border border-border bg-card p-4 space-y-3">
            <h3 className="font-semibold">Launcher Path</h3>
            <input
              value={launcherPath}
              onChange={(e) => setLauncherPath(e.target.value)}
              placeholder="Auto-discovered if empty"
              className="w-full rounded-lg border border-input bg-background px-3 py-2 text-sm"
            />
            <button
              onClick={saveLauncherPath}
              className="rounded-lg bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90"
            >
              Save
            </button>
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
                    window.alert('You are running the latest version of Agora.');
                  }
                } catch (e) {
                  alert(formatError(e));
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
