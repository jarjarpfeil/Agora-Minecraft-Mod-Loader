import { useState, useCallback, useEffect } from 'react';
import {
  checkInstanceHealth,
  disableModForTest,
  launchInstance,
  getSetting,
  setSetting,
  type HealthReport,
  type HealthBlocker,
  type HealthWarning,
} from '@/lib/tauri';
import { Button } from '@/components/ui/button';
import { Card } from '@/components/ui/card';
import { Switch } from '@/components/ui/switch';

interface HealthDialogProps {
  instanceId: string;
  instanceName: string;
  onClose: () => void;
  onLaunch: () => void;
}

function silenceKey(item: HealthWarning | HealthBlocker): string {
  return `health_silenced_${item.kind}_${item.mod_id ?? 'global'}`;
}

export function HealthDialog({ instanceId, instanceName, onClose, onLaunch }: HealthDialogProps) {
  const [report, setReport] = useState<HealthReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [silenced, setSilenced] = useState<Set<string>>(new Set());
  const [fixing, setFixing] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const r = await checkInstanceHealth(instanceId);
        if (!cancelled) setReport(r);
        // Load silenced settings
        const keys: string[] = [...r.warnings, ...r.blockers].map(silenceKey);
        const silencedSet = new Set<string>();
        for (const k of keys) {
          const val = await getSetting(k);
          if (val === 'true') silencedSet.add(k);
        }
        if (!cancelled) setSilenced(silencedSet);
      } catch (e: any) {
        if (!cancelled) setError(e?.message || String(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, [instanceId]);

  const handleFixDisable = useCallback(async (filename: string, key: string) => {
    setFixing(key);
    try {
      await disableModForTest(instanceId, filename);
      // Re-run health after fix
      const r = await checkInstanceHealth(instanceId);
      setReport(r);
    } catch (e: any) {
      setError(e?.message || String(e));
    } finally {
      setFixing(null);
    }
  }, [instanceId]);

  const handleSilence = useCallback(async (key: string, current: boolean) => {
    const next = !current;
    await setSetting(key, String(next));
    setSilenced(prev => {
      const next2 = new Set(prev);
      if (next) next2.add(key); else next2.delete(key);
      return next2;
    });
  }, []);

  const handleLaunchAnyway = useCallback(async () => {
    try {
      await launchInstance(instanceId);
      onLaunch();
    } catch (e: any) {
      setError(e?.message || String(e));
    }
  }, [instanceId, onLaunch]);

  if (loading) {
    return (
      <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
        <Card className="w-full max-w-lg p-6 bg-card border-border">
          <p className="text-muted-foreground">Scanning instance for conflicts...</p>
        </Card>
      </div>
    );
  }

  if (error) {
    return (
      <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
        <Card className="w-full max-w-lg p-6 bg-card border-border">
          <p className="text-destructive mb-4">Health check failed: {error}</p>
          <Button onClick={onClose} variant="outline">Close</Button>
        </Card>
      </div>
    );
  }

  if (!report) return null;

  const activeBlockers = report.blockers.filter(b => {
    const k = silenceKey(b);
    return !silenced.has(k);
  });

  const activeWarnings = report.warnings.filter(w => {
    const k = silenceKey(w);
    return !silenced.has(k);
  });

  // After silencing, compute effective score
  const effectiveScore = activeBlockers.length > 0 ? 'red' as const
    : activeWarnings.length > 0 ? 'yellow' as const
    : 'green' as const;

  const scoreColors = {
    green: { bg: 'bg-green-500/10', border: 'border-green-500', text: 'text-green-600 dark:text-green-400', label: 'Ready to Launch' },
    yellow: { bg: 'bg-amber-500/10', border: 'border-amber-500', text: 'text-amber-600 dark:text-amber-400', label: 'Warnings' },
    red: { bg: 'bg-destructive/10', border: 'border-destructive', text: 'text-destructive', label: 'Launch Blocked' },
  } as const;

  const sc = scoreColors[effectiveScore];

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
      <Card className="w-full max-w-lg p-6 bg-card border-border">
        {/* Score badge */}
        <div className={`rounded-lg border ${sc.border} ${sc.bg} p-3 mb-4`}>
          <div className="flex items-center gap-2">
            <div className={`text-lg font-bold ${sc.text}`}>
              {effectiveScore === 'green' ? '●' : effectiveScore === 'yellow' ? '◐' : '○'}
            </div>
            <div>
              <p className={`font-semibold ${sc.text}`}>{sc.label}</p>
              <p className="text-xs text-muted-foreground">{instanceName}</p>
            </div>
          </div>
        </div>

        {/* Blockers */}
        {report.blockers.length > 0 && (
          <div className="mb-3">
            <h4 className="text-sm font-semibold mb-2">Blockers ({report.blockers.length})</h4>
            <div className="space-y-2">
              {report.blockers.map((b, i) => {
                const key = silenceKey(b);
                const isSilenced = silenced.has(key);
                return (
                  <div key={i} className={`rounded border p-2 text-sm ${isSilenced ? 'border-border bg-muted/50 opacity-60' : 'border-destructive bg-destructive/10'}`}>
                    <p className={isSilenced ? 'text-muted-foreground line-through' : 'text-destructive'}>{b.message}</p>
                    <div className="flex items-center gap-2 mt-1">
                      {b.suggested_action && (
                        <span className="text-xs text-muted-foreground">{b.suggested_action}</span>
                      )}
                    </div>
                    <div className="flex items-center gap-3 mt-2">
                      {/* Disable button for mods we can disable */}
                      {(b.kind === 'missing_required_dependency' || b.kind === 'incompatible_mod' || b.kind === 'curated_conflict') && b.mod_id && (
                        <Button
                          size="sm"
                          variant="outline"
                          disabled={fixing === key}
                          onClick={() => handleFixDisable(b.mod_id!, key)}
                        >
                          {fixing === key ? 'Fixing...' : 'Disable'}
                        </Button>
                      )}
                    </div>
                  </div>
                );
              })}
            </div>
          </div>
        )}

        {/* Warnings */}
        {report.warnings.length > 0 && (
          <div className="mb-3">
            <h4 className="text-sm font-semibold mb-2">Warnings ({report.warnings.length})</h4>
            <div className="space-y-2">
              {report.warnings.map((w, i) => {
                const key = silenceKey(w);
                const isSilenced = silenced.has(key);
                return (
                  <div key={i} className={`rounded border p-2 text-sm ${isSilenced ? 'border-border bg-muted/50 opacity-60' : 'border-amber-500 bg-amber-500/10'}`}>
                    <p className={isSilenced ? 'text-muted-foreground line-through' : 'text-amber-600 dark:text-amber-400'}>{w.message}</p>
                    <div className="flex items-center justify-between mt-2">
                      <div className="flex items-center gap-2 text-xs">
                        <Switch
                          checked={!isSilenced}
                          onCheckedChange={() => handleSilence(key, isSilenced)}
                          aria-label="Show this warning"
                        />
                        <span className="text-muted-foreground">
                          {isSilenced ? 'Muted' : 'Show on next launch'}
                        </span>
                      </div>
                      {w.suggested_action && !isSilenced && (
                        <span className="text-xs text-muted-foreground">{w.suggested_action}</span>
                      )}
                    </div>
                  </div>
                );
              })}
            </div>
          </div>
        )}

        {/* All clear */}
        {effectiveScore === 'green' && (
          <p className="text-sm text-green-600 dark:text-green-400 mb-4">
            No issues found. Instance is ready to launch.
          </p>
        )}

        {/* Actions */}
        <div className="flex gap-2 justify-end">
          <Button variant="outline" onClick={onClose}>Cancel</Button>
          {effectiveScore === 'red' && activeBlockers.length > 0 ? (
            <Button variant="destructive" onClick={handleLaunchAnyway}>Launch Anyway</Button>
          ) : effectiveScore === 'yellow' || effectiveScore === 'green' ? (
            <Button onClick={handleLaunchAnyway}>Launch Anyway</Button>
          ) : null}
        </div>
      </Card>
    </div>
  );
}
