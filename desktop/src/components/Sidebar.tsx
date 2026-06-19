type Tab = 'home' | 'browse' | 'modrinth' | 'instances' | 'governance' | 'settings';

interface SidebarProps {
  tabs: { id: Tab; label: string; icon: string }[];
  activeTab: Tab;
  onSelectTab: (tab: Tab) => void;
}

export function Sidebar({ tabs, activeTab, onSelectTab }: SidebarProps) {
  return (
    <aside className="w-64 flex flex-col border-r border-gray-200 dark:border-gray-700 bg-[rgb(var(--surface))]">
      <div className="p-6 border-b border-gray-200 dark:border-gray-700">
        <h1 className="text-lg font-bold tracking-tight">Agora</h1>
        <p className="text-xs text-[rgb(var(--muted))] mt-1">Boutique mod discovery</p>
      </div>
      <nav className="flex-1 p-3 space-y-1">
        {tabs.map((tab) => {
          const isActive = activeTab === tab.id;
          return (
            <button
              key={tab.id}
              onClick={() => onSelectTab(tab.id)}
              className={[
                'w-full flex items-center gap-3 rounded-lg px-3 py-2.5 text-sm font-medium transition-colors',
                isActive
                  ? 'bg-brand-100 text-brand-900 dark:bg-brand-900 dark:text-brand-100'
                  : 'text-[rgb(var(--text))] hover:bg-gray-100 dark:hover:bg-gray-800',
              ].join(' ')}
            >
              <span className="text-lg" aria-hidden="true">{tab.icon}</span>
              {tab.label}
            </button>
          );
        })}
      </nav>
      <div className="p-4 text-xs text-[rgb(var(--muted))] border-t border-gray-200 dark:border-gray-700">
        v0.1.0 · Community curated
      </div>
    </aside>
  );
}
