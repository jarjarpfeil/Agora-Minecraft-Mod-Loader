import { useEffect, useRef, useState } from 'react';
import {
  checkRegistryUpdate,
  githubLogin,
  githubLoginPoll,
  setSetting,
  type DeviceFlowResponse,
  type RegistryStatus,
} from '../lib/tauri';

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
      <div className="w-full max-w-xl rounded-2xl border border-gray-200 dark:border-gray-700 surface shadow-xl">
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
              i <= currentIndex ? 'bg-brand-600' : 'bg-gray-300 dark:bg-gray-600'
            }`}
          />
          <span
            className={`text-xs ${
              i === currentIndex ? 'font-semibold' : 'text-[rgb(var(--muted))]'
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
      <p className="text-[rgb(var(--muted))] mb-4">
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
          className="rounded-lg bg-brand-600 px-5 py-2 text-sm font-medium text-white hover:bg-brand-700"
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
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const handleContinue = async () => {
    setSaving(true);
    setError(null);
    try {
      await setSetting('modrinth_enabled', modrinth);
      await setSetting('ai_mcp_enabled', aiMcp);
      onContinue();
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div>
      <Stepper current="services" />
      <h2 className="text-2xl font-bold mb-2">Connect External Services</h2>
      <p className="text-[rgb(var(--muted))] mb-6">
        Optional integrations. Both are disabled by default and can be changed later in Settings.
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
      </div>

      {error && <p className="mt-4 text-xs text-red-600 dark:text-red-300">{error}</p>}

      <div className="mt-8 flex justify-between">
        <button
          onClick={onBack}
          className="rounded-lg px-4 py-2 text-sm font-medium text-[rgb(var(--muted))] hover:underline"
        >
          Back
        </button>
        <button
          onClick={handleContinue}
          disabled={saving}
          className="rounded-lg bg-brand-600 px-5 py-2 text-sm font-medium text-white hover:bg-brand-700 disabled:opacity-50"
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
    <div className="rounded-xl border border-gray-200 dark:border-gray-700 surface p-4">
      <div className="flex items-center justify-between gap-4">
        <span className="font-medium text-sm">{title}</span>
        <button
          type="button"
          role="switch"
          aria-checked={checked}
          onClick={() => onChange(!checked)}
          className={`relative inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors ${
            checked ? 'bg-brand-600' : 'bg-gray-300 dark:bg-gray-600'
          }`}
        >
          <span
            className={`inline-block h-5 w-5 transform rounded-full bg-white shadow transition-transform ${
              checked ? 'translate-x-5' : 'translate-x-0.5'
            }`}
          />
        </button>
      </div>
      <p className="mt-2 text-xs text-[rgb(var(--muted))]">{description}</p>
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
  const cancelledRef = useRef(false);

  useEffect(() => {
    return () => {
      cancelledRef.current = true;
    };
  }, []);

  const signIn = async () => {
    setError(null);
    setResult(null);
    setPolling(true);
    try {
      const flow = await githubLogin();
      if (cancelledRef.current) return;
      setDevice(flow);
      const token = await githubLoginPoll(flow.device_code, flow.interval);
      if (cancelledRef.current) return;
      if (token) {
        setResult('Signed in successfully.');
        setTimeout(() => {
          if (!cancelledRef.current) onContinue();
        }, 800);
      } else {
        setResult('Authentication did not complete.');
      }
    } catch (e) {
      if (!cancelledRef.current) setError(String(e));
    } finally {
      if (!cancelledRef.current) setPolling(false);
    }
  };

  return (
    <div>
      <Stepper current="github" />
      <h2 className="text-2xl font-bold mb-2">Connect GitHub</h2>
      <p className="text-[rgb(var(--muted))] mb-6">
        Sign in with GitHub to participate in community governance (voting, proposals). This is
        optional and can be completed later in Settings.
      </p>

      {device && (
        <div className="rounded-xl border border-gray-200 dark:border-gray-700 surface p-4 mb-4">
          <p className="text-sm">Open this URL and enter the code:</p>
          <p className="mt-1 text-sm font-semibold break-all text-brand-600 dark:text-brand-400">
            {device.verification_uri}
          </p>
          <p className="mt-2 text-sm">
            Code:{' '}
            <span className="font-mono font-bold tracking-widest">{device.user_code}</span>
          </p>
          {polling && (
            <p className="mt-2 text-xs text-[rgb(var(--muted))]">Waiting for authorization…</p>
          )}
        </div>
      )}

      {result && <p className="mb-4 text-sm text-green-600 dark:text-green-400">{result}</p>}
      {error && <p className="mb-4 text-xs text-red-600 dark:text-red-300">{error}</p>}

      <div className="flex justify-between">
        <button
          onClick={onBack}
          className="rounded-lg px-4 py-2 text-sm font-medium text-[rgb(var(--muted))] hover:underline"
          disabled={polling}
        >
          Back
        </button>
        <div className="flex gap-2">
          <button
            onClick={onContinue}
            disabled={polling}
            className="rounded-lg px-4 py-2 text-sm font-medium text-[rgb(var(--muted))] hover:underline disabled:opacity-50"
          >
            I'll do this later
          </button>
          <button
            onClick={signIn}
            disabled={polling}
            className="rounded-lg bg-brand-600 px-5 py-2 text-sm font-medium text-white hover:bg-brand-700 disabled:opacity-50"
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
  const [status, setStatus] = useState<RegistryStatus | null>(null);
  const [downloading, setDownloading] = useState(false);
  const [done, setDone] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const download = async () => {
    setDownloading(true);
    setError(null);
    try {
      const result = await checkRegistryUpdate(true);
      setStatus(result);
      setDone(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setDownloading(false);
    }
  };

  useEffect(() => {
    download();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <div>
      <Stepper current="registry" />
      <h2 className="text-2xl font-bold mb-2">Download Registry</h2>
      <p className="text-[rgb(var(--muted))] mb-6">
        Agora needs the curated registry database to show mods, packs, shaders, and more.
      </p>

      <div className="rounded-xl border border-gray-200 dark:border-gray-700 surface p-4">
        {downloading && <p className="text-sm">Downloading the latest registry…</p>}
        {!downloading && status && (
          <>
            <p className="text-sm font-medium">Registry ready.</p>
            <p className="text-xs text-[rgb(var(--muted))] mt-1">{status.message}</p>
            {status.cached_tag && (
              <p className="text-xs text-[rgb(var(--muted))]">Version: {status.cached_tag}</p>
            )}
          </>
        )}
        {!downloading && error && (
          <p className="text-xs text-red-600 dark:text-red-300">{error}</p>
        )}
      </div>

      <div className="mt-8 flex justify-between">
        <button
          onClick={onBack}
          disabled={downloading}
          className="rounded-lg px-4 py-2 text-sm font-medium text-[rgb(var(--muted))] hover:underline disabled:opacity-50"
        >
          Back
        </button>
        <button
          onClick={onFinish}
          disabled={downloading}
          className="rounded-lg bg-brand-600 px-5 py-2 text-sm font-medium text-white hover:bg-brand-700 disabled:opacity-50"
        >
          {done ? 'Finish' : 'Skip & Finish'}
        </button>
      </div>
    </div>
  );
}
