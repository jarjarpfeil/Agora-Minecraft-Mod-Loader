import { useEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { open as openUrl } from '@tauri-apps/plugin-shell';
import {
  cancelJavaRuntime,
  ensureJavaRuntime,
  formatError,
  getSetting,
  githubLogin,
  githubLoginPoll,
  setSetting,
  type DeviceFlowResponse,
  type JavaRuntimeProgressEvent,
} from '../lib/tauri';
import { useRegistryState } from '../lib/useRegistryState';
import { RegistryStatusView } from '../components/registry-status-view';
import { DeviceFlowPanel } from '../components/DeviceFlowPanel';

type Step = 'welcome' | 'services' | 'java' | 'github' | 'registry';

interface OnboardingProps {
  onComplete: () => void;
}

export function Onboarding({ onComplete }: OnboardingProps) {
  const [step, setStep] = useState<Step>('welcome');
  const [services, setServices] = useState({ modrinth: false, aiMcp: false, aiChat: false });
  const [servicesLoading, setServicesLoading] = useState(true);
  // Persisted across Back/Forward so a registry auto-download triggered on
  // the first entry is not re-triggered when the user revisits the step.
  const registryAutoDownloaded = useRef(false);

  useEffect(() => {
    let cancelled = false;
    Promise.allSettled([
      getSetting('modrinth_enabled'),
      getSetting('ai_mcp_enabled'),
      getSetting('ai_chat_enabled'),
    ]).then(([modrinth, aiMcp, aiChat]) => {
      if (cancelled) return;
      setServices({
        modrinth: modrinth.status === 'fulfilled' ? parseBooleanSetting(modrinth.value) : false,
        aiMcp: aiMcp.status === 'fulfilled' ? parseBooleanSetting(aiMcp.value) : false,
        aiChat: aiChat.status === 'fulfilled' ? parseBooleanSetting(aiChat.value) : false,
      });
      setServicesLoading(false);
    });
    return () => { cancelled = true; };
  }, []);

  const finish = async () => {
    try {
      await setSetting('onboarding_complete', true);
    } catch {
      // best-effort persistence; still let the user proceed
    } finally {
      onComplete();
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm">
        <div className="w-full max-w-xl rounded-2xl border border-border bg-card shadow-xl">
        <div className="p-6 sm:p-8">
          {step === 'welcome' && <WelcomeStep onContinue={() => setStep('services')} />}
          {step === 'services' && (
            <ServicesStep
              values={services}
              loading={servicesLoading}
              onChange={setServices}
              onContinue={() => setStep('java')}
              onBack={() => setStep('welcome')}
            />
          )}
          {step === 'java' && (
            <JavaStep onContinue={() => setStep('github')} onBack={() => setStep('services')} />
          )}
          {step === 'github' && (
            <GithubStep onContinue={() => setStep('registry')} onBack={() => setStep('java')} />
          )}
          {step === 'registry' && (
            <RegistryStep onFinish={finish} onBack={() => setStep('github')} hasAutoDownloaded={registryAutoDownloaded} />
          )}
        </div>
      </div>
    </div>
  );
}

function parseBooleanSetting(value: unknown): boolean {
  return value === true || value === 1 || value === 'true' || value === '1';
}

function Stepper({ current }: { current: Step }) {
  const steps: { id: Step; label: string }[] = [
    { id: 'welcome', label: 'Welcome' },
    { id: 'services', label: 'Services' },
    { id: 'java', label: 'Java' },
    { id: 'github', label: 'GitHub' },
    { id: 'registry', label: 'Registry' },
  ];
  const currentIndex = steps.findIndex((s) => s.id === current);
  return (
    <div className="mb-6 flex items-center gap-2">
      {steps.map((s, i) => (
        <div key={s.id} className="flex items-center gap-2">
          <span
            className={`h-2 w-2 rounded-full ${
              i <= currentIndex ? 'bg-primary' : 'bg-muted'
            }`}
          />
          <span
            className={`text-xs ${
              i === currentIndex ? 'font-semibold' : 'text-muted-foreground'
            }`}
          >
            {s.label}
          </span>
          {i < steps.length - 1 && (
            <span className="mx-1 h-px w-6 bg-gray-300 dark:bg-gray-600" />
          )}
        </div>
      ))}
    </div>
  );
}

function WelcomeStep({ onContinue }: { onContinue: () => void }) {
  return (
    <div>
      <Stepper current="welcome" />
      <h2 className="text-2xl font-bold mb-2">Welcome to Agora</h2>
      <p className="text-muted-foreground mb-4">
        A decentralized, ad-free, open-source Minecraft mod launcher and discovery platform.
      </p>
      <p className="text-sm mb-6">
        Agora returns platform control to the community. The GitHub repository itself is the
        database — flat-file manifests are compiled into a signed SQLite registry. Agora can launch
        directly with optional in-app Microsoft authentication, while delegation to the official
        Mojang launcher remains available as the default fallback. GitHub governance sign-in and
        GitHub Copilot sign-in are separate optional accounts.
      </p>
      <div className="flex justify-end">
        <button
          onClick={onContinue}
          className="rounded-lg bg-primary px-5 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90"
        >
          Get Started
        </button>
      </div>
    </div>
  );
}

function ServicesStep({
  values,
  loading,
  onChange,
  onContinue,
  onBack,
}: {
  values: { modrinth: boolean; aiMcp: boolean; aiChat: boolean };
  loading: boolean;
  onChange: (value: { modrinth: boolean; aiMcp: boolean; aiChat: boolean }) => void;
  onContinue: () => void;
  onBack: () => void;
}) {
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const handleContinue = async () => {
    setSaving(true);
    setError(null);
    try {
      await setSetting('modrinth_enabled', values.modrinth);
      await setSetting('ai_mcp_enabled', values.aiMcp);
      await setSetting('ai_chat_enabled', values.aiChat);
      onContinue();
    } catch (e) {
      setError(formatError(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div>
      <Stepper current="services" />
      <h2 className="text-2xl font-bold mb-2">Connect External Services</h2>
      <p className="text-muted-foreground mb-6">
        Optional integrations. All are disabled by default and can be changed later in Settings.
      </p>

      <div className="space-y-4">
        <ServiceToggle
          title="Modrinth Access"
          description="Allow live Modrinth API queries and show Modrinth-sourced curated mods alongside the Agora registry."
          checked={values.modrinth}
          onChange={(modrinth) => onChange({ ...values, modrinth })}
        />
        <ServiceToggle
          title="AI / MCP Server"
          description="Enable the local MCP server for external AI tools to interact with Agora."
          checked={values.aiMcp}
          onChange={(aiMcp) => onChange({ ...values, aiMcp })}
        />
        <ServiceToggle
          title="Integrated AI Assistant"
          description="Built-in AI chat using free GitHub Models. Get instant crash analysis and mod help without any external setup."
          checked={values.aiChat}
          onChange={(aiChat) => onChange({ ...values, aiChat })}
        />
      </div>

      <p className="mt-3 text-xs text-muted-foreground">
        <strong>MCP Server</strong> connects your existing AI agent to Agora.{' '}
        <strong>Integrated AI</strong> gives you a built-in chat — simpler, no setup, but less
        powerful. You can use either, both, or neither.
      </p>

      {error && <p className="mt-4 text-xs text-destructive">{error}</p>}

      <div className="mt-8 flex justify-between">
        <button
          onClick={onBack}
          className="rounded-lg px-4 py-2 text-sm font-medium text-muted-foreground hover:underline"
        >
          Back
        </button>
        <button
          onClick={handleContinue}
          disabled={saving || loading}
          className="rounded-lg bg-primary px-5 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
        >
          {loading ? 'Loading…' : saving ? 'Saving…' : 'Continue'}
        </button>
      </div>
    </div>
  );
}

function ServiceToggle({
  title,
  description,
  checked,
  onChange,
}: {
  title: string;
  description: string;
  checked: boolean;
  onChange: (value: boolean) => void;
}) {
  return (
    <div className="rounded-xl border border-border bg-card p-4">
      <div className="flex items-center justify-between gap-4">
        <span className="font-medium text-sm">{title}</span>
        <button
          type="button"
          role="switch"
          aria-checked={checked}
          onClick={() => onChange(!checked)}
          className={`relative inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors ${
            checked ? 'bg-primary' : 'bg-muted'
          }`}
        >
          <span
            className={`inline-block h-5 w-5 transform rounded-full bg-white shadow transition-transform ${
              checked ? 'translate-x-5' : 'translate-x-0.5'
            }`}
          />
        </button>
      </div>
      <p className="mt-2 text-xs text-muted-foreground">{description}</p>
    </div>
  );
}

function JavaStep({
  onContinue,
  onBack,
}: {
  onContinue: () => void;
  onBack: () => void;
}) {
  const [checked, setChecked] = useState(true);
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<string | null>(null);
  const [percent, setPercent] = useState<number | null>(null);
  const [cancelling, setCancelling] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState(false);

  const OPERATION_ID = 'onboarding-java-21';

  // Listen for java-runtime-progress events for onboarding
  useEffect(() => {
    const unlisten = listen<JavaRuntimeProgressEvent>(
      'java-runtime-progress',
      (event) => {
        // Only track onboarding progress
        if (event.payload.instance_id !== '') return;
        setProgress(event.payload.message || `Java ${event.payload.major}: ${event.payload.stage}`);
        setPercent(event.payload.percent);
        if (event.payload.stage === 'ready') {
          setDone(true);
          setProgress('Java 21 is ready.');
        }
      },
    );
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const handleCancelJava = async () => {
    setCancelling(true);
    setProgress('Cancelling…');
    try {
      await cancelJavaRuntime(OPERATION_ID);
    } catch {
      // Operation may already be complete
    }
    // Allow continue without Java even if cancel API fails
    setBusy(false);
    setCancelling(false);
    setProgress(null);
    setPercent(null);
    onContinue();
  };

  const handleContinue = async () => {
    if (!checked) {
      onContinue();
      return;
    }
    setBusy(true);
    setError(null);
    setProgress('Preparing Java 21 runtime…');
    setPercent(0);
    try {
      await ensureJavaRuntime(21, OPERATION_ID);
      setProgress('Java 21 is ready.');
      setPercent(100);
      setDone(true);
      setTimeout(() => onContinue(), 800);
    } catch (e) {
      setError(formatError(e));
      setProgress(null);
      setPercent(null);
      // Allow Continue anyway
    } finally {
      setBusy(false);
    }
  };

  return (
    <div>
      <Stepper current="java" />
      <h2 className="text-2xl font-bold mb-2">Prepare Java for Minecraft</h2>
      <p className="text-muted-foreground mb-6">
        Modern Minecraft (1.17+) requires Java 17 or higher. Agora can download Java 21 — the
        latest long-term support version — so your instances work out of the box.
      </p>

      <div className="rounded-xl border border-border bg-card p-4">
        <div className="flex items-center justify-between gap-4">
          <div>
            <p className="font-medium text-sm">Prepare Java 21 for modern Minecraft</p>
            <p className="text-xs text-muted-foreground mt-1">
              Downloads and manages a private Java 21 runtime in Agora's app data directory.
              Older exact versions download automatically when needed for specific instances.
            </p>
          </div>
          <button
            type="button"
            role="switch"
            aria-checked={checked}
            onClick={() => {
              if (!busy) setChecked(!checked);
            }}
            className={`relative inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors ${
              checked ? 'bg-primary' : 'bg-muted'
            }`}
          >
            <span
              className={`inline-block h-5 w-5 transform rounded-full bg-white shadow transition-transform ${
                checked ? 'translate-x-5' : 'translate-x-0.5'
              }`}
            />
          </button>
        </div>
      </div>

      {busy && progress && !done && (
        <div className="rounded-lg bg-muted px-3 py-3 mt-4 space-y-2">
          <div className="flex items-center gap-2">
            <div className="flex-1 h-2 bg-background rounded-full overflow-hidden">
              <div
                className="h-full bg-primary rounded-full transition-all duration-300"
                style={{ width: `${Math.min(percent ?? 0, 100)}%` }}
              />
            </div>
            <button
              onClick={handleCancelJava}
              disabled={cancelling}
              className="rounded-lg border border-border px-2 py-1 text-xs font-medium hover:bg-accent disabled:opacity-50 shrink-0"
            >
              {cancelling ? 'Cancelling…' : 'Cancel'}
            </button>
          </div>
          <p className="text-xs text-muted-foreground">{progress}</p>
        </div>
      )}

      {done && progress && (
        <div className="rounded-lg bg-muted px-3 py-2 mt-4">
          <p className="text-xs text-muted-foreground">{progress}</p>
        </div>
      )}

      {error && (
        <div className="rounded-lg bg-destructive/10 px-3 py-2 mt-4">
          <p className="text-xs text-destructive">{error}</p>
          <p className="text-xs text-muted-foreground mt-1">
            You can continue without Java and download it later from Settings.
          </p>
        </div>
      )}

      <div className="mt-8 flex justify-between">
        <button
          onClick={onBack}
          disabled={busy}
          className="rounded-lg px-4 py-2 text-sm font-medium text-muted-foreground hover:underline disabled:opacity-50"
        >
          Back
        </button>
        <button
          onClick={handleContinue}
          disabled={busy && !done}
          className="rounded-lg bg-primary px-5 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
        >
          {busy ? 'Downloading…' : done ? 'Continuing…' : 'Continue'}
        </button>
      </div>
    </div>
  );
}

function GithubStep({
  onContinue,
  onBack,
}: {
  onContinue: () => void;
  onBack: () => void;
}) {
  const [device, setDevice] = useState<DeviceFlowResponse | null>(null);
  const [polling, setPolling] = useState(false);
  const [result, setResult] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Per-sign-in-attempt guard. Each call to `signIn` captures the current
  // value; if a later `signIn` call starts (or the user navigates away and
  // back), the earlier attempt sees the changed value and aborts.
  //
  // NOTE: do NOT use an unmount-ref pattern here. React <StrictMode> (active
  // in dev) re-runs effect cleanups on every render in development, which
  // flips an unmount ref to true mid-await and aborts the OAuth flow even
  // though the component is still mounted. Using a changing counter avoids
  // that false positive — only a *new* sign-in attempt invalidates the
  // in-flight one.
  const sessionIdRef = useRef(0);

  const signIn = async () => {
    setError(null);
    setResult(null);
    setPolling(true);
    const mySession = ++sessionIdRef.current;
    const isStale = () => sessionIdRef.current !== mySession;
    try {
      const flow = await githubLogin();
      if (isStale()) return;
      setDevice(flow);

      // Auto-launch the user's default browser at the verification URL.
      // Wrapped in its own try/catch AND fire-and-forget. If the shell plugin
      // throws synchronously, the inner catch absorbs it so the outer flow
      // continues to githubLoginPoll. URL+code remain displayed for manual
      // fallback.
      try {
        const p = openUrl(flow.verification_uri);
        Promise.resolve(p).catch(() => {
          /* best-effort: URL shown in panel below */
        });
      } catch {
        // URL and code remain visible for manual fallback.
      }

      const token = await githubLoginPoll(flow.device_code, flow.interval);
      if (isStale()) return;
      if (token) {
        setResult('Signed in successfully.');
        setTimeout(() => {
          if (!isStale()) onContinue();
        }, 800);
      } else {
        setResult('Authentication did not complete.');
      }
    } catch (e) {
      const msg = e instanceof Error ? e.message : formatError(e);
      if (!isStale()) setError(`Sign-in failed: ${msg}`);
    } finally {
      if (!isStale()) setPolling(false);
    }
  };

  return (
    <div>
      <Stepper current="github" />
      <h2 className="text-2xl font-bold mb-2">Connect GitHub</h2>
      <p className="text-muted-foreground mb-6">
        Sign in with GitHub to participate in community governance (voting, proposals). This is
        optional and can be completed later in Settings.
      </p>

      {device && (
        <DeviceFlowPanel
          device={device}
          polling={polling}
          className="mb-4"
          onCancel={() => {
            sessionIdRef.current += 1;
            setPolling(false);
            setDevice(null);
          }}
        />
      )}

      {result && <p className="mb-4 text-sm text-primary">{result}</p>}
      {error && <p className="mb-4 text-xs text-destructive">{error}</p>}

      <div className="flex justify-between">
        <button
          onClick={() => {
            // Invalidate the polling session before navigating away so
            // an in-flight poll cannot auto-advance onboarding later.
            sessionIdRef.current += 1;
            onBack();
          }}
          className="rounded-lg px-4 py-2 text-sm font-medium text-muted-foreground hover:underline"
        >
          Back
        </button>
        <div className="flex gap-2">
          {!polling && (
            <button
              onClick={() => {
                // Invalidate the polling session before navigating away.
                sessionIdRef.current += 1;
                onContinue();
              }}
              className="rounded-lg px-4 py-2 text-sm font-medium text-muted-foreground hover:underline"
            >
              I'll do this later
            </button>
          )}
          <button
            onClick={signIn}
            disabled={polling}
          className="rounded-lg bg-primary px-5 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
          >
            {polling ? 'Waiting…' : 'Sign in with GitHub'}
          </button>
        </div>
      </div>
    </div>
  );
}

function RegistryStep({
  onFinish,
  onBack,
  hasAutoDownloaded,
}: {
  onFinish: () => void;
  onBack: () => void;
  /** Shared with the parent so the flag survives Back/Forward navigation. */
  hasAutoDownloaded: { current: boolean };
}) {
  const { state, status, loading, error, actions } = useRegistryState();
  const syncRegistry = actions.sync;

  // Auto-download once when we first detect the registry is missing.
  // The effect must react to state changes because on the first render
  // state is 'loading' or 'unknown', and the download should fire when
  // it transitions to 'missing'.
  useEffect(() => {
    if (
      !hasAutoDownloaded.current &&
      state === 'missing' &&
      !loading &&
      !status?.has_cached_db
    ) {
      hasAutoDownloaded.current = true;
      syncRegistry();
    }
  }, [state, loading, status?.has_cached_db, syncRegistry, hasAutoDownloaded]);

  return (
    <div>
      <Stepper current="registry" />
      <h2 className="text-2xl font-bold mb-2">Download Registry</h2>
      <p className="text-muted-foreground mb-6">
        Agora needs the curated registry database to show mods, packs, shaders, and more.
      </p>

      <RegistryStatusView
        variant="fullscreen"
        state={state}
        status={status}
        error={error}
        actions={actions}
        onContinue={onFinish}
        allowMissingContinue
        missingWarning="The registry is required to browse curated content. You can continue but the catalog will be empty until the registry is downloaded."
      />

      <div className="mt-8 flex justify-between">
        <button
          onClick={onBack}
          disabled={loading}
          className="rounded-lg px-4 py-2 text-sm font-medium text-muted-foreground hover:underline disabled:opacity-50"
        >
          Back
        </button>
      </div>
    </div>
  );
}
