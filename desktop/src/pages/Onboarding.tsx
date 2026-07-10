import { useEffect, useRef, useState } from 'react';
import { open as openUrl } from '@tauri-apps/plugin-shell';
import {
  formatError,
  githubLogin,
  githubLoginPoll,
  setSetting,
  type DeviceFlowResponse,
} from '../lib/tauri';
import { useRegistryState } from '../lib/useRegistryState';
import { RegistryStatusView } from '../components/registry-status-view';

type Step = 'welcome' | 'services' | 'github' | 'registry';

interface OnboardingProps {
  onComplete: () => void;
}

export function Onboarding({ onComplete }: OnboardingProps) {
  const [step, setStep] = useState<Step>('welcome');

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
            <ServicesStep onContinue={() => setStep('github')} onBack={() => setStep('welcome')} />
          )}
          {step === 'github' && (
            <GithubStep onContinue={() => setStep('registry')} onBack={() => setStep('services')} />
          )}
          {step === 'registry' && (
            <RegistryStep onFinish={finish} onBack={() => setStep('github')} />
          )}
        </div>
      </div>
    </div>
  );
}

function Stepper({ current }: { current: Step }) {
  const steps: { id: Step; label: string }[] = [
    { id: 'welcome', label: 'Welcome' },
    { id: 'services', label: 'Services' },
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
        database — flat-file manifests are compiled into a signed SQLite registry, and the app
        delegates authentication and game execution to the official Mojang launcher. No backend
        services, no Microsoft/Xbox auth inside the app, just curated quality over infinite
        inventory.
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
  onContinue,
  onBack,
}: {
  onContinue: () => void;
  onBack: () => void;
}) {
  const [modrinth, setModrinth] = useState(false);
  const [aiMcp, setAiMcp] = useState(false);
  const [aiChat, setAiChat] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const handleContinue = async () => {
    setSaving(true);
    setError(null);
    try {
      await setSetting('modrinth_enabled', modrinth);
      await setSetting('ai_mcp_enabled', aiMcp);
      await setSetting('ai_chat_enabled', aiChat);
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
          checked={modrinth}
          onChange={setModrinth}
        />
        <ServiceToggle
          title="AI / MCP Server"
          description="Enable the local MCP server for external AI tools to interact with Agora."
          checked={aiMcp}
          onChange={setAiMcp}
        />
        <ServiceToggle
          title="Integrated AI Assistant"
          description="Built-in AI chat using free GitHub Models. Get instant crash analysis and mod help without any external setup."
          checked={aiChat}
          onChange={setAiChat}
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
          disabled={saving}
          className="rounded-lg bg-primary px-5 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
        >
          {saving ? 'Saving…' : 'Continue'}
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
      console.log('[onboarding] signIn starting, calling githubLogin()');
      const flow = await githubLogin();
      if (isStale()) {
        console.log('[onboarding] session superseded after githubLogin; aborting');
        return;
      }
      setDevice(flow);
      console.log('[onboarding] setDevice done, user_code=', flow.user_code);

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
      } catch (syncErr) {
        console.warn('[onboarding] openUrl synchronous throw:', syncErr);
      }

      const token = await githubLoginPoll(flow.device_code, flow.interval);
      if (isStale()) {
        console.log('[onboarding] session superseded after poll; aborting');
        return;
      }
      if (token) {
        setResult('Signed in successfully.');
        setTimeout(() => {
          if (!isStale()) onContinue();
        }, 800);
      } else {
        setResult('Authentication did not complete.');
      }
    } catch (e) {
      console.error('[onboarding] signIn failed:', e);
      const msg = e instanceof Error ? e.message : formatError(e);
      if (!isStale()) setError(`Sign-in failed: ${msg}`);
    } finally {
      console.log('[onboarding] signIn finally: clearing polling state');
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
        <div className="rounded-xl border border-border bg-card p-4 mb-4">
          <p className="text-sm">Opening your browser… If it didn't open, click the button below:</p>
          <p className="mt-1 text-sm font-semibold break-all text-primary dark:text-primary">
            {device.verification_uri}
          </p>
          <p className="mt-2 text-sm">
            Code:{' '}
            <span className="font-mono font-bold tracking-widest">{device.user_code}</span>
          </p>
          <button
            type="button"
            onClick={() => {
              openUrl(device.verification_uri).catch(() => {
                /* best-effort: URL shown above for manual copy */
              });
            }}
            disabled={polling}
            className="mt-3 rounded-lg border border-border px-3 py-1.5 text-xs font-medium hover:bg-accent disabled:opacity-50"
          >
            Open in browser
          </button>
          {polling && (
            <p className="mt-2 text-xs text-muted-foreground">Waiting for authorization…</p>
          )}
        </div>
      )}

      {result && <p className="mb-4 text-sm text-primary">{result}</p>}
      {error && <p className="mb-4 text-xs text-destructive">{error}</p>}

      <div className="flex justify-between">
        <button
          onClick={onBack}
          className="rounded-lg px-4 py-2 text-sm font-medium text-muted-foreground hover:underline"
          disabled={polling}
        >
          Back
        </button>
        <div className="flex gap-2">
          <button
            onClick={onContinue}
            disabled={polling}
            className="rounded-lg px-4 py-2 text-sm font-medium text-muted-foreground hover:underline disabled:opacity-50"
          >
            I'll do this later
          </button>
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
}: {
  onFinish: () => void;
  onBack: () => void;
}) {
  const { state, status, loading, error, actions } = useRegistryState();
  const hasAutoDownloaded = useRef(false);

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
      actions.sync();
    }
  }, [state, loading, status?.has_cached_db, actions]);

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
