import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import ReactMarkdown from 'react-markdown';
import rehypeRaw from 'rehype-raw';
import rehypeSanitize from 'rehype-sanitize';

import {
  aiChat,
  ChatMessage,
  copilotLogin,
  copilotLoginPoll,
  copilotLogout,
  copilotStatus,
  copilotTryGovernanceToken,
  CopilotDeviceFlowResponse,
  CopilotToken,
  formatError,
} from '../lib/tauri';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface AiAssistantProps {
  instanceId?: string | null;
  crashLog?: string | null;
  crashSignatures?: string | null;
  suspects?: string | null;
  onClose: () => void;
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export function AiAssistant({
  instanceId,
  crashLog,
  crashSignatures,
  suspects,
  onClose,
}: AiAssistantProps) {
  const [copilotToken, setCopilotToken] = useState<CopilotToken | null>(null);
  const [copilotLoading, setCopilotLoading] = useState(true);
  const [flowResponse, setFlowResponse] = useState<CopilotDeviceFlowResponse | null>(null);
  const [polling, setPolling] = useState(false);
  const [countdown, setCountdown] = useState(0);
  const [rateLimited, setRateLimited] = useState(false);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [input, setInput] = useState('');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const scrollRef = useRef<HTMLDivElement>(null);

  // --- Check Copilot status on mount, silently trying the governance token ---
  useEffect(() => {
    (async () => {
      setCopilotLoading(true);
      try {
        const token = await copilotStatus();
        if (token) {
          setCopilotToken(token);
        } else {
          // No stored Copilot token — try reusing the governance token.
          try {
            const govToken = await copilotTryGovernanceToken();
            setCopilotToken(govToken);
          } catch {
            setCopilotToken(null);
          }
        }
      } catch { setCopilotToken(null); }
      setCopilotLoading(false);
    })();
  }, []);

  // --- Countdown timer for device code ---
  useEffect(() => {
    if (!flowResponse) return;
    const expiresAt = Date.now() + flowResponse.expires_in * 1000;
    const timer = setInterval(() => {
      const remaining = Math.max(0, Math.floor((expiresAt - Date.now()) / 1000));
      setCountdown(remaining);
    }, 1000);
    return () => clearInterval(timer);
  }, [flowResponse]);

  // --- Auto-scroll to bottom on new messages ---
  useEffect(() => {
    if (scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [messages, loading]);

  // --- Build context for first message only ---
  const context = useMemo(
    () =>
      messages.length === 0
        ? {
            instance_id: instanceId ?? null,
            crash_log: crashLog ?? null,
            crash_signatures: crashSignatures ?? null,
            suspects: suspects ?? null,
          }
        : null,
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [instanceId, crashLog, crashSignatures, suspects, messages.length],
  );

  // --- Device code flow ---
  const startDeviceFlow = useCallback(async () => {
    setError(null);
    setFlowResponse(null);
    setPolling(false);
    try {
      const flow = await copilotLogin();
      setFlowResponse(flow);
      setCountdown(flow.expires_in);
      setPolling(true);
      const token = await copilotLoginPoll(flow.device_code, flow.interval);
      setCopilotToken(token);
      setFlowResponse(null);
      setPolling(false);
    } catch (e) {
      setError(formatError(e));
      setFlowResponse(null);
      setPolling(false);
    }
  }, []);

  // --- Send handler ---
  const handleSend = useCallback(async () => {
    const trimmed = input.trim();
    if (!trimmed || loading) return;

    const userMsg: ChatMessage = { role: 'user', content: trimmed };
    const updated = [...messages, userMsg];
    setMessages(updated);
    setInput('');
    setLoading(true);
    setError(null);

    try {
      const response = await aiChat(updated, context);
      setMessages((prev) => [
        ...prev,
        { role: 'assistant', content: response.content },
      ]);
    } catch (e) {
      const err = e as Record<string, unknown>;
      if (err?.code === 'ERR_AI_RATE_LIMIT') {
        setRateLimited(true);
        setError('You\'ve reached your free monthly limit (50 Copilot requests). Your limit resets next month.');
      } else {
        setError(formatError(e));
      }
    } finally {
      setLoading(false);
    }
  }, [input, loading, messages, context]);

  // --- Keyboard shortcut: Enter to send, Shift+Enter for newline ---
  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  // --- Retry handler ---
  const handleRetry = useCallback(async () => {
    if (messages.length === 0) return;

    // Re-send the last user message (drop any failed assistant response)
    const userMessages = messages.filter((m) => m.role === 'user');
    if (userMessages.length === 0) return;

    const messagesToSend = userMessages;

    setMessages(messagesToSend);
    setLoading(true);
    setError(null);

    try {
      const response = await aiChat(messagesToSend, context);
      setMessages((prev) => [...prev, { role: 'assistant', content: response.content }]);
    } catch (e) {
      const err = e as Record<string, unknown>;
      if (err?.code === 'ERR_AI_RATE_LIMIT') {
        setRateLimited(true);
        setError('You\'ve reached your free monthly limit (50 Copilot requests). Your limit resets next month.');
      } else {
        setError(formatError(e));
      }
    } finally {
      setLoading(false);
    }
  }, [messages, context]);

  // --- Copilot auth gate ---
  if (copilotLoading) {
    return (
      <div className="flex h-full w-full flex-col items-center justify-center rounded-xl border border-border bg-card">
        <p className="text-sm text-muted-foreground">Loading…</p>
      </div>
    );
  }

  // --- Device code flow (not authenticated) ---
  if (copilotToken === null) {
    return (
      <div className="flex h-full w-full flex-col rounded-xl border border-border bg-card">
        <div className="flex items-center justify-between border-b border-border px-4 py-3">
          <h2 className="text-sm font-semibold">Agora Instance Assistant</h2>
          <button
            onClick={onClose}
            className="text-muted-foreground hover:text-foreground"
            aria-label="Close"
          >
            &times;
          </button>
        </div>
        <div className="flex flex-1 flex-col items-center justify-center gap-4 px-6 py-8 text-center">
          <div className="text-3xl" aria-hidden="true">&#129302;</div>
          <h3 className="text-base font-medium text-foreground">
            Need help optimizing your mods?
          </h3>
          <p className="max-w-xs text-sm text-muted-foreground">
            Activate your built-in assistant to diagnose crashes, resolve conflicts,
            and get mod recommendations — powered by GitHub Copilot.
          </p>
          {flowResponse === null ? (
            <>
              <button
                onClick={startDeviceFlow}
                className="mt-2 rounded-lg bg-primary px-6 py-2.5 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90"
              >
                Connect with GitHub
              </button>
              <p className="text-xs text-muted-foreground">
                Free — 50 diagnostic chats/month
              </p>
              <div className="flex w-full items-center gap-2 px-8 py-2">
                <div className="h-px flex-1 bg-border" />
                <span className="text-xs text-muted-foreground">OR</span>
                <div className="h-px flex-1 bg-border" />
              </div>
              <button
                onClick={startDeviceFlow}
                className="text-xs text-muted-foreground underline hover:text-foreground"
              >
                Sign in with different account
              </button>
            </>
          ) : (
            <div className="flex flex-col items-center gap-4">
              <p className="text-sm text-muted-foreground">
                Enter the following code on GitHub:
              </p>
              <div className="rounded-lg border border-border bg-muted px-8 py-4">
                <span className="select-all text-2xl font-bold tracking-widest text-foreground">
                  {flowResponse.user_code}
                </span>
              </div>
              <div className="flex gap-3">
                <button
                  onClick={() => navigator.clipboard.writeText(flowResponse.user_code)}
                  className="rounded-lg border border-border bg-background px-4 py-2 text-sm text-foreground transition-colors hover:bg-muted"
                >
                  Copy code
                </button>
                <button
                  onClick={() => window.open(flowResponse.verification_uri, '_blank')}
                  className="rounded-lg bg-primary px-4 py-2 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90"
                >
                  Open GitHub Activation Page
                </button>
              </div>
              <p className="text-xs text-muted-foreground">
                {polling
                  ? `Waiting for approval… Code expires in ${countdown}s`
                  : `Code expires in ${countdown}s`}
              </p>
              {error && (
                <p className="text-xs text-destructive">{error}</p>
              )}
            </div>
          )}
        </div>
      </div>
    );
  }

  // --- Main chat UI ---
  return (
    <div className="flex h-full w-full flex-col overflow-hidden rounded-xl border border-border bg-card">
      {/* Header */}
      <div className="flex items-center justify-between border-b border-border px-4 py-3">
        <h2 className="text-sm font-semibold">AI Assistant</h2>
        <div className="flex items-center gap-2">
          {copilotToken && (
            <span className="text-[11px] text-muted-foreground">
              Connected as {copilotToken.username} ({copilotToken.plan})
            </span>
          )}
          {copilotToken && (
            <button
              onClick={async () => {
                await copilotLogout();
                setCopilotToken(null);
                setMessages([]);
                setError(null);
                setRateLimited(false);
              }}
              className="text-[11px] text-muted-foreground underline hover:text-foreground"
            >
              Sign out
            </button>
          )}
          <button
            onClick={onClose}
            className="text-muted-foreground hover:text-foreground"
            aria-label="Close"
          >
            &times;
          </button>
        </div>
      </div>

      {/* Privacy note */}
      {messages.length === 0 && (
        <div className="border-b border-border px-4 py-2">
          <p className="text-[11px] text-muted-foreground">
            Your crash data is sent to GitHub Copilot for analysis.
            Free tier: 50 diagnostic chats per month.
          </p>
        </div>
      )}

      {/* Messages list */}
      <div
        ref={scrollRef}
        className="flex flex-1 flex-col gap-3 overflow-y-auto p-4"
      >
        {messages.length === 0 && !loading && (
          <div className="flex flex-1 items-center justify-center">
            <p className="text-xs text-muted-foreground">
              Ask about crashes, mods, or anything Agora-related.
            </p>
          </div>
        )}

        {messages.map((msg, i) =>
          msg.role === 'user' ? (
            <div key={i} className="flex justify-end">
              <div className="max-w-[80%] rounded-xl bg-primary px-4 py-2 text-sm text-primary-foreground">
                {msg.content}
              </div>
            </div>
          ) : (
            <div key={i} className="flex justify-start">
              <div className="max-w-[80%] rounded-xl bg-card px-4 py-2 text-sm text-card-foreground">
                <div className="prose prose-sm prose-neutral dark:prose-invert max-w-none break-words">
                  <ReactMarkdown rehypePlugins={[rehypeRaw, rehypeSanitize]}>
                    {msg.content}
                  </ReactMarkdown>
                </div>
              </div>
            </div>
          ),
        )}

        {/* Loading indicator */}
        {loading && (
          <div className="flex justify-start">
            <div className="rounded-xl bg-card">
              <span className="text-sm text-muted-foreground">
                Thinking
                <span className="inline-flex gap-0.5">
                  <span className="dot1">.</span>
                  <span className="dot2">.</span>
                  <span className="dot3">.</span>
                </span>
              </span>
            </div>
          </div>
        )}

        {/* Error display */}
        {error && (
          <div className="rounded-lg border border-destructive/20 bg-destructive/10 px-4 py-3">
            <p className="text-sm text-destructive">{error}</p>
            <button
              onClick={handleRetry}
              className="mt-2 text-xs text-destructive underline hover:text-destructive/80"
            >
              Retry
            </button>
          </div>
        )}
      </div>

      {/* Rate limit banner */}
      {rateLimited && (
        <div className="border-t border-border px-4 py-3">
          <div className="rounded-lg border border-amber-500/20 bg-amber-500/10 px-4 py-3">
            <p className="text-sm text-amber-600 dark:text-amber-400">
              You've reached your free monthly limit (50 Copilot requests).
              Your limit resets next month.
            </p>
          </div>
        </div>
      )}

      {/* Input area */}
      <div className="border-t border-border p-3">
        <div className="flex gap-2">
          <textarea
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder={rateLimited ? 'Monthly limit reached' : "Ask about crashes, mods, or anything…"}
            rows={2}
            disabled={rateLimited}
            className="flex-1 resize-none rounded-lg border border-input bg-background px-3 py-2 text-sm text-foreground outline-none placeholder-muted-foreground focus:border-primary focus:ring-1 focus:ring-ring disabled:cursor-not-allowed disabled:opacity-50"
          />
          <button
            onClick={handleSend}
            disabled={loading || !input.trim() || rateLimited}
            className="self-end rounded-lg bg-primary px-4 py-2 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90 disabled:cursor-not-allowed disabled:opacity-50"
          >
            Send
          </button>
        </div>
      </div>
    </div>
  );
}
