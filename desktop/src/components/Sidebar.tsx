import { useRef } from 'react';
import type { LucideIcon } from 'lucide-react';
import { ChevronLeft, ChevronRight, Command } from 'lucide-react';
import { BrandMark } from './BrandMark';

type Tab = 'home' | 'browse' | 'instances' | 'governance' | 'ai' | 'settings';

interface SidebarProps {
  tabs: { id: Tab; label: string; icon: LucideIcon }[];
  activeTab: Tab;
  onSelectTab: (tab: Tab) => void;
  onOpenCommandPalette?: () => void;
  collapsed: boolean;
  width: number;
  onCollapsedChange: (collapsed: boolean) => void;
  onWidthChange: (width: number) => void;
  onWidthCommit: (width: number) => void;
}

const MIN_WIDTH = 180;
const MAX_WIDTH = 420;
const DEFAULT_WIDTH = 256;

function clampWidth(width: number) {
  return Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, width));
}

export function Sidebar({
  tabs,
  activeTab,
  onSelectTab,
  onOpenCommandPalette,
  collapsed,
  width,
  onCollapsedChange,
  onWidthChange,
  onWidthCommit,
}: SidebarProps) {
  const latestWidth = useRef(width);

  const startResize = (event: React.PointerEvent<HTMLDivElement>) => {
    event.preventDefault();
    const startX = event.clientX;
    const startWidth = width;
    latestWidth.current = width;
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';

    const move = (moveEvent: PointerEvent) => {
      const nextWidth = clampWidth(startWidth + moveEvent.clientX - startX);
      latestWidth.current = nextWidth;
      onWidthChange(nextWidth);
    };
    const finish = () => {
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', finish);
      onWidthCommit(latestWidth.current);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', finish, { once: true });
  };

  const resizeWithKeyboard = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
    event.preventDefault();
    const amount = event.shiftKey ? 32 : 8;
    const nextWidth = clampWidth(width + (event.key === 'ArrowRight' ? amount : -amount));
    onWidthChange(nextWidth);
    onWidthCommit(nextWidth);
  };

  return (
    <aside
      className="decorative-shell relative flex shrink-0 flex-col border-r border-border bg-card/95 shadow-[4px_0_24px_hsl(var(--midnight)/0.04)] backdrop-blur"
      style={{ width: collapsed ? 64 : width }}
      data-testid="sidebar"
    >
      <div className={`border-b border-border ${collapsed ? 'p-3' : 'p-4'}`}>
        <BrandMark compact={collapsed} className={collapsed ? 'justify-center' : ''} />
      </div>

      <button
        onClick={() => onCollapsedChange(!collapsed)}
        className="absolute -right-3 top-20 z-10 flex h-6 w-6 items-center justify-center rounded-full border border-border bg-card text-muted-foreground shadow-sm hover:bg-accent"
        aria-label={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}
      >
        {collapsed ? <ChevronRight className="h-3.5 w-3.5" /> : <ChevronLeft className="h-3.5 w-3.5" />}
      </button>

      {!collapsed && (
        <div
          role="separator"
          aria-label="Resize sidebar"
          aria-orientation="vertical"
          aria-valuemin={MIN_WIDTH}
          aria-valuemax={MAX_WIDTH}
          aria-valuenow={Math.round(width)}
          tabIndex={0}
          onPointerDown={startResize}
          onKeyDown={resizeWithKeyboard}
          onDoubleClick={() => {
            onWidthChange(DEFAULT_WIDTH);
            onWidthCommit(DEFAULT_WIDTH);
          }}
          className="absolute inset-y-0 -right-1 z-[5] w-2 cursor-col-resize touch-none focus:outline-none focus:ring-2 focus:ring-inset focus:ring-ring"
        />
      )}

      <nav className="flex-1 space-y-1 p-3" aria-label="Main navigation">
        {tabs.map((tab) => {
          const isActive = activeTab === tab.id;
          const Icon = tab.icon;
          return (
            <button
              key={tab.id}
              onClick={() => onSelectTab(tab.id)}
              aria-current={isActive ? 'page' : undefined}
              title={collapsed ? tab.label : undefined}
              className={[
                'flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-sm font-medium transition-colors',
                collapsed ? 'justify-center px-0' : '',
                isActive
                  ? 'bg-primary text-primary-foreground shadow-sm'
                  : 'text-muted-foreground hover:bg-accent hover:text-accent-foreground',
              ].join(' ')}
            >
              <Icon className="h-[18px] w-[18px] shrink-0" aria-hidden="true" />
              {!collapsed && tab.label}
            </button>
          );
        })}
      </nav>

      {!collapsed && (
        <div className="space-y-1 border-t border-border p-3">
          <button
            onClick={onOpenCommandPalette}
            className="flex w-full items-center gap-3 rounded-lg px-3 py-2 text-sm font-medium text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
            aria-label="Open command palette"
          >
            <Command className="h-4 w-4" aria-hidden="true" />
            <span>Quick actions</span>
            <kbd className="ml-auto rounded border border-border bg-background px-1.5 py-0.5 font-mono text-[10px]">Ctrl K</kbd>
          </button>
        </div>
      )}

      {!collapsed && (
        <div className="border-t border-border p-4 text-xs text-muted-foreground">v0.1.0 · Community curated</div>
      )}
    </aside>
  );
}
