import { type RegistryState, type RegistryActions } from '../lib/useRegistryState';
import type { RegistryStatus } from '../lib/tauri';

export type ViewVariant = 'card' | 'banner' | 'fullscreen';

interface BaseProps {
  state: RegistryState;
  status: RegistryStatus | null;
  error: string | null;
  actions: RegistryActions;
}

interface CardProps extends BaseProps {
  variant: 'card';
  /** Title shown above the status text in card mode. */
  title?: string;
}

interface BannerProps extends BaseProps {
  variant: 'banner';
}

interface FullscreenProps extends BaseProps {
  variant: 'fullscreen';
  /** Called when the user chooses to continue (ready / offline). */
  onContinue?: () => void;
  /** Text for the continue button. Overrides the default per-state label. */
  continueLabel?: string;
  /** Show the continue button even in `missing` state (with warning). */
  allowMissingContinue?: boolean;
  /** Warning text shown when continuing without a cached database. */
  missingWarning?: string;
}

type Props = CardProps | BannerProps | FullscreenProps;

function DefaultCard({
  title,
  state,
  status,
  error,
  actions,
}: CardProps) {
  const isSyncing = state === 'loading';

  return (
    <div className="rounded-xl border border-border bg-card p-4">
      <div className="flex items-center justify-between gap-4">
        <div className="min-w-0">
          {title && <h3 className="font-semibold text-sm">{title}</h3>}
          <StatusText state={state} status={status} error={error} />
        </div>
        <ActionButton state={state} isSyncing={isSyncing} onAction={actions.sync} />
      </div>
      {error && state !== 'missing' && (
        <p className="mt-2 text-xs text-destructive">{error}</p>
      )}
    </div>
  );
}

function BannerView({ state, status: _status, error, actions }: BannerProps) {
  if (state === 'ready') return null;

  const isSyncing = state === 'loading';

  return (
    <div className="flex items-center justify-between gap-2 rounded-md bg-amber-50 dark:bg-amber-900/20 px-3 py-2 text-xs">
      <span className="text-amber-700 dark:text-amber-300">
        {error && state === 'missing'
          ? 'Registry not available.'
          : error && state === 'offline'
            ? 'Using cached registry. Updates unavailable.'
            : isSyncing
              ? 'Refreshing registry…'
              : 'Offline mode'}
      </span>
      <button
        onClick={actions.sync}
        disabled={isSyncing}
        className="shrink-0 rounded px-2 py-0.5 font-medium text-amber-700 dark:text-amber-300 hover:bg-amber-100 dark:hover:bg-amber-800/40 disabled:opacity-50"
      >
        {isSyncing ? 'Refreshing…' : 'Retry'}
      </button>
    </div>
  );
}

function FullscreenView({
  state,
  status,
  error,
  actions,
  onContinue,
  continueLabel,
  allowMissingContinue,
  missingWarning,
}: FullscreenProps) {
  const isSyncing = state === 'loading';

  return (
    <div className="rounded-xl border border-border bg-card p-4">
      {isSyncing && (
        <p className="text-sm">
          {state === 'loading' && status?.has_cached_db
            ? 'Checking for updates…'
            : 'Downloading the latest registry…'}
        </p>
      )}

      {!isSyncing && state === 'ready' && (
        <>
          <p className="text-sm font-medium">Registry ready.</p>
          {status?.message && (
            <p className="text-xs text-muted-foreground mt-1">{status.message}</p>
          )}
          {status?.cached_tag && (
            <p className="text-xs text-muted-foreground">Version: {status.cached_tag}</p>
          )}
          {onContinue && (
            <div className="mt-4 flex justify-end">
              <button
                onClick={onContinue}
                className="rounded-lg bg-primary px-5 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90"
              >
                {continueLabel ?? 'Finish'}
              </button>
            </div>
          )}
        </>
      )}

      {!isSyncing && state === 'offline' && (
        <>
          <p className="text-sm">Using cached registry.</p>
          {status?.cached_tag && (
            <p className="text-xs text-muted-foreground">Cached: {status.cached_tag}</p>
          )}
          {error && <p className="text-xs text-destructive mt-1">{error}</p>}
          <div className="mt-4 flex justify-between">
            {onContinue && (
              <button
                onClick={onContinue}
                className="rounded-lg px-4 py-2 text-sm font-medium text-muted-foreground hover:underline"
              >
                {continueLabel ?? 'Continue Offline'}
              </button>
            )}
            <button
              onClick={actions.sync}
              disabled={isSyncing}
              className="rounded-lg bg-primary px-5 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
            >
              Retry Download
            </button>
          </div>
        </>
      )}

      {!isSyncing && state === 'missing' && (
        <>
          <p className="text-sm">No registry database found.</p>
          {error && <p className="text-xs text-destructive mt-1">{error}</p>}
          {missingWarning && (
            <p className="mt-2 text-xs text-amber-600 dark:text-amber-400">
              {missingWarning}
            </p>
          )}
          <div className="mt-4 flex justify-end gap-2">
            {allowMissingContinue && onContinue && (
              <button
                onClick={onContinue}
                className="rounded-lg px-4 py-2 text-sm font-medium text-muted-foreground hover:underline"
              >
                {continueLabel ?? 'Continue Without Catalog'}
              </button>
            )}
            <button
              onClick={actions.sync}
              disabled={isSyncing}
              className="rounded-lg bg-primary px-5 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
            >
              Retry
            </button>
          </div>
        </>
      )}

      {!isSyncing && state === 'unknown' && (
        <>
          <p className="text-sm text-muted-foreground">Checking registry status…</p>
          {error && <p className="text-xs text-destructive mt-1">{error}</p>}
          <div className="mt-4 flex justify-end">
            <button
              onClick={actions.sync}
              className="rounded-lg bg-primary px-5 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90"
            >
              Download Registry
            </button>
          </div>
        </>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

function StatusText({
  state,
  status,
  error,
}: {
  state: RegistryState;
  status: RegistryStatus | null;
  error: string | null;
}) {
  if (state === 'loading') {
    return (
      <p className="text-xs text-muted-foreground mt-1">
        {status?.has_cached_db ? 'Checking for updates…' : 'Downloading registry…'}
      </p>
    );
  }

  if (state === 'unknown') {
    return (
      <p className="text-xs text-muted-foreground mt-1">Loading…</p>
    );
  }

  if (state === 'ready') {
    return (
      <>
        <p className="text-xs text-muted-foreground mt-1">
          {status?.message ?? 'Using cached registry.'}
        </p>
        {status?.cached_tag && (
          <p className="text-xs text-muted-foreground">
            Cached: {status.cached_tag}
            {status.cached_schema_version != null &&
              ` · schema v${status.cached_schema_version}`}
          </p>
        )}
        {status?.latest_tag && status.latest_tag !== status.cached_tag && (
          <p className="text-xs text-amber-600 dark:text-amber-400">
            Latest: {status.latest_tag}
          </p>
        )}
      </>
    );
  }

  if (state === 'offline') {
    return (
      <>
        <p className="text-xs text-muted-foreground mt-1">
          Using cached registry (offline).
        </p>
        {status?.cached_tag && (
          <p className="text-xs text-muted-foreground">
            Cached: {status.cached_tag}
          </p>
        )}
      </>
    );
  }

  // missing
  return (
    <p className="text-xs text-muted-foreground mt-1">
      {error
        ? 'Unable to download the registry. Connect to the internet and try again.'
        : 'No registry database found.'}
    </p>
  );
}

function ActionButton({
  state,
  isSyncing,
  onAction,
}: {
  state: RegistryState;
  isSyncing: boolean;
  onAction: () => void;
}) {
  if (state === 'loading') {
    return (
      <button
        disabled
        className="rounded-lg bg-primary/50 px-3 py-1.5 text-xs font-medium text-primary-foreground whitespace-nowrap"
      >
        {statusTextLoading()}
      </button>
    );
  }

  if (state === 'ready') {
    return (
      <button
        onClick={onAction}
        disabled={isSyncing}
        className="rounded-lg bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50 whitespace-nowrap"
      >
        Check for Updates
      </button>
    );
  }

  if (state === 'offline') {
    return (
      <button
        onClick={onAction}
        disabled={isSyncing}
        className="rounded-lg bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50 whitespace-nowrap"
      >
        Check for Updates
      </button>
    );
  }

  // missing
  return (
    <button
      onClick={onAction}
      disabled={isSyncing}
      className="rounded-lg bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50 whitespace-nowrap"
    >
      Download Registry
    </button>
  );
}

function statusTextLoading(): string {
  return 'Working…';
}

// ---------------------------------------------------------------------------
// Default export dispatches to variant
// ---------------------------------------------------------------------------

export function RegistryStatusView(props: Props) {
  switch (props.variant) {
    case 'card':
      return <DefaultCard {...props} />;
    case 'banner':
      return <BannerView {...props} />;
    case 'fullscreen':
      return <FullscreenView {...props} />;
  }
}
