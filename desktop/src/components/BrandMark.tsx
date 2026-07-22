interface BrandMarkProps {
  className?: string;
  compact?: boolean;
}

export function BrandMark({ className = '', compact = false }: BrandMarkProps) {
  return (
    <div className={`flex items-center gap-3 ${className}`.trim()}>
      <img
        src="/brand/agora-launcher-icon.png"
        alt=""
        aria-hidden="true"
        className="h-10 w-10 rounded-[0.7rem] object-cover shadow-sm ring-1 ring-midnight/10"
      />
      {!compact && (
        <div className="min-w-0">
          <p className="truncate text-base font-semibold tracking-[0.08em] text-foreground">Agora Launcher</p>
          <p className="mt-0.5 text-[10px] font-medium uppercase tracking-[0.2em] text-primary">Open to all</p>
        </div>
      )}
    </div>
  );
}
