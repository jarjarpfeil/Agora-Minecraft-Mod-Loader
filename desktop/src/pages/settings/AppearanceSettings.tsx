import {
  DEFAULT_UI_PREFERENCES,
  useUiPreferences,
  type UiPreferences,
} from '../../components/theme/theme-provider';

const selectClass = 'rounded-md border border-input bg-background px-2.5 py-1.5 text-sm focus:outline-none focus:ring-2 focus:ring-ring';

const PRESETS: Record<string, { label: string; preferences: UiPreferences }> = {
  agora: { label: 'Agora default', preferences: DEFAULT_UI_PREFERENCES },
  night: {
    label: 'Agora night',
    preferences: { ...DEFAULT_UI_PREFERENCES, colorMode: 'dark' },
  },
  civic: {
    label: 'Civic gold',
    preferences: {
      ...DEFAULT_UI_PREFERENCES,
      colorMode: 'dark', accentMode: 'custom', customAccent: '#c28b28',
      surfaceMode: 'custom', customSurface: '#17263b', surfaceOpacity: 0.92,
      navMode: 'custom', customNav: '#0d1929', navOpacity: 0.96,
      backgroundMode: 'custom', customBackground: '#091321',
      textMode: 'custom', customText: '#f4ead4',
      backgroundTextMode: 'custom', customBackgroundText: '#e8d9bb',
      fontFamily: 'serif',
    },
  },
  ender: {
    label: 'Ender night',
    preferences: {
      ...DEFAULT_UI_PREFERENCES,
      colorMode: 'dark', accentMode: 'custom', customAccent: '#a855f7',
      surfaceMode: 'custom', customSurface: '#21162d', surfaceOpacity: 0.9,
      navMode: 'custom', customNav: '#160d20', navOpacity: 0.94,
      backgroundMode: 'custom', customBackground: '#0d0812',
      textMode: 'custom', customText: '#f0e7f7',
      backgroundTextMode: 'custom', customBackgroundText: '#dac8e8',
    },
  },
  terminal: {
    label: 'Compact terminal',
    preferences: {
      ...DEFAULT_UI_PREFERENCES,
      colorMode: 'dark', accentMode: 'custom', customAccent: '#45d483',
      surfaceMode: 'custom', customSurface: '#102018', surfaceOpacity: 0.94,
      navMode: 'custom', customNav: '#07110c', navOpacity: 0.98,
      backgroundMode: 'custom', customBackground: '#050a07',
      textMode: 'custom', customText: '#d9fbe7',
      backgroundTextMode: 'custom', customBackgroundText: '#a8daba',
      fontFamily: 'mono', density: 'compact', cornerStyle: 'square', backgroundEffects: false,
    },
  },
  readable: {
    label: 'High readability',
    preferences: {
      ...DEFAULT_UI_PREFERENCES,
      fontFamily: 'readable', fontScale: 1.1, highContrast: true,
      motion: 'reduced', backgroundEffects: false, density: 'spacious',
    },
  },
};

export function AppearanceSettings({ onResetLayout }: { onResetLayout: () => void }) {
  const { preferences, setPreferences, resetPreferences } = useUiPreferences();

  return (
    <div id="settings-appearance" className="scroll-mt-24 rounded-xl border border-border bg-card p-4 space-y-4" data-testid="appearance-settings">
      <div>
        <h3 className="font-semibold">Appearance</h3>
        <p className="mt-1 text-xs text-muted-foreground">Color, readability, spacing, and motion preferences apply immediately.</p>
      </div>

      <label className="block space-y-1 text-sm">
        <span className="font-medium">Appearance preset</span>
        <select
          aria-label="Appearance preset"
          defaultValue=""
          onChange={(event) => {
            const preset = PRESETS[event.target.value];
            if (preset) setPreferences(preset.preferences);
            event.currentTarget.value = '';
          }}
          className={`${selectClass} block w-full sm:w-72`}
        >
          <option value="" disabled>Choose a preset…</option>
          {Object.entries(PRESETS).map(([id, preset]) => <option key={id} value={id}>{preset.label}</option>)}
        </select>
      </label>

      <div className="grid gap-4 sm:grid-cols-2">
        <label className="space-y-1 text-sm">
          <span className="font-medium">Color mode</span>
          <select
            aria-label="Color mode"
            value={preferences.colorMode}
            onChange={(event) => setPreferences({ colorMode: event.target.value as typeof preferences.colorMode })}
            className={`${selectClass} block w-full`}
          >
            <option value="system">Follow system</option>
            <option value="light">Light</option>
            <option value="dark">Dark</option>
          </select>
        </label>

        <label className="space-y-1 text-sm">
          <span className="font-medium">Accent</span>
          <select
            aria-label="Accent source"
            value={preferences.accentMode}
            onChange={(event) => setPreferences({ accentMode: event.target.value as typeof preferences.accentMode })}
            className={`${selectClass} block w-full`}
          >
            <option value="agora">Agora teal</option>
            <option value="system">Windows accent</option>
            <option value="custom">Custom</option>
          </select>
        </label>
      </div>

      {preferences.accentMode === 'custom' && (
        <label className="flex items-center justify-between gap-4 text-sm">
          <span>
            <span className="block font-medium">Custom accent color</span>
            <span className="text-xs text-muted-foreground">Used for primary actions, selection, focus, and hover surfaces.</span>
          </span>
          <input
            type="color"
            aria-label="Custom accent color"
            value={preferences.customAccent}
            onChange={(event) => setPreferences({ customAccent: event.target.value })}
            className="h-9 w-14 cursor-pointer rounded border border-input bg-background p-1"
          />
        </label>
      )}

      <details className="group rounded-lg border border-border bg-muted">
        <summary aria-label="Toggle custom colors" className="cursor-pointer select-none px-3 py-2.5 text-sm font-semibold">
          Custom colors
          <span className="ml-2 text-xs font-normal text-muted-foreground">Block, navigation, background, and text colors</span>
        </summary>
        <div className="grid gap-4 border-t border-border p-3 lg:grid-cols-2">
          <div className="space-y-3 rounded-lg border border-border bg-card p-3">
          <label className="flex items-center justify-between gap-3 text-sm">
            <span>
              <span className="block font-medium">Custom block color</span>
              <span className="text-xs text-muted-foreground">Cards, panels, dialogs, and content blocks.</span>
            </span>
            <input type="checkbox" aria-label="Use custom block color" checked={preferences.surfaceMode === 'custom'} onChange={(event) => setPreferences({ surfaceMode: event.target.checked ? 'custom' : 'theme' })} className="h-5 w-5 accent-primary" />
          </label>
          {preferences.surfaceMode === 'custom' && (
            <input type="color" aria-label="Block color" value={preferences.customSurface} onChange={(event) => setPreferences({ customSurface: event.target.value })} className="h-9 w-full cursor-pointer rounded border border-input bg-background p-1" />
          )}
          <label className="block space-y-1 text-xs">
            <span className="flex justify-between"><span>Block opacity</span><span>{Math.round(preferences.surfaceOpacity * 100)}%</span></span>
            <input type="range" aria-label="Block opacity" min="0.35" max="1" step="0.05" value={preferences.surfaceOpacity} onChange={(event) => setPreferences({ surfaceOpacity: Number(event.target.value) })} className="w-full accent-primary" />
          </label>
          </div>
          <div className="space-y-3 rounded-lg border border-border bg-card p-3">
          <label className="flex items-center justify-between gap-3 text-sm">
            <span>
              <span className="block font-medium">Custom navigation color</span>
              <span className="text-xs text-muted-foreground">Sidebar background and translucency.</span>
            </span>
            <input type="checkbox" aria-label="Use custom navigation color" checked={preferences.navMode === 'custom'} onChange={(event) => setPreferences({ navMode: event.target.checked ? 'custom' : 'theme' })} className="h-5 w-5 accent-primary" />
          </label>
          {preferences.navMode === 'custom' && (
            <input type="color" aria-label="Navigation color" value={preferences.customNav} onChange={(event) => setPreferences({ customNav: event.target.value })} className="h-9 w-full cursor-pointer rounded border border-input bg-background p-1" />
          )}
          <label className="block space-y-1 text-xs">
            <span className="flex justify-between"><span>Navigation opacity</span><span>{Math.round(preferences.navOpacity * 100)}%</span></span>
            <input type="range" aria-label="Navigation opacity" min="0.35" max="1" step="0.05" value={preferences.navOpacity} onChange={(event) => setPreferences({ navOpacity: Number(event.target.value) })} className="w-full accent-primary" />
          </label>
          </div>
          <div className="space-y-2 rounded-lg border border-border bg-muted p-3">
          <label className="flex items-center justify-between gap-3 text-sm">
            <span>
              <span className="block font-medium">Custom background</span>
              <span className="text-xs text-muted-foreground">Override the page background color.</span>
            </span>
            <input type="checkbox" aria-label="Use custom background" checked={preferences.backgroundMode === 'custom'} onChange={(event) => setPreferences({ backgroundMode: event.target.checked ? 'custom' : 'theme' })} className="h-5 w-5 accent-primary" />
          </label>
          {preferences.backgroundMode === 'custom' && (
            <input type="color" aria-label="Background color" value={preferences.customBackground} onChange={(event) => setPreferences({ customBackground: event.target.value })} className="h-9 w-full cursor-pointer rounded border border-input bg-background p-1" />
          )}
          </div>
          <div className="space-y-2 rounded-lg border border-border bg-muted p-3">
          <label className="flex items-center justify-between gap-3 text-sm">
            <span>
              <span className="block font-medium">Custom block text</span>
              <span className="text-xs text-muted-foreground">Primary text inside cards and controls.</span>
            </span>
            <input type="checkbox" aria-label="Use custom text color" checked={preferences.textMode === 'custom'} onChange={(event) => setPreferences({ textMode: event.target.checked ? 'custom' : 'theme' })} className="h-5 w-5 accent-primary" />
          </label>
          {preferences.textMode === 'custom' && (
            <input type="color" aria-label="Block text color" value={preferences.customText} onChange={(event) => setPreferences({ customText: event.target.value })} className="h-9 w-full cursor-pointer rounded border border-input bg-background p-1" />
          )}
          </div>
          <div className="space-y-2 rounded-lg border border-border bg-muted p-3 lg:col-span-2">
          <label className="flex items-center justify-between gap-3 text-sm">
            <span>
              <span className="block font-medium">Custom background text</span>
              <span className="text-xs text-muted-foreground">Headings and text directly on the page background.</span>
            </span>
            <input type="checkbox" aria-label="Use custom background text color" checked={preferences.backgroundTextMode === 'custom'} onChange={(event) => setPreferences({ backgroundTextMode: event.target.checked ? 'custom' : 'theme' })} className="h-5 w-5 accent-primary" />
          </label>
          {preferences.backgroundTextMode === 'custom' && (
            <input type="color" aria-label="Background text color" value={preferences.customBackgroundText} onChange={(event) => setPreferences({ customBackgroundText: event.target.value })} className="h-9 w-full cursor-pointer rounded border border-input bg-background p-1" />
          )}
          </div>
        </div>
      </details>

      <div className="grid gap-4 sm:grid-cols-3">
        <label className="space-y-1 text-sm">
          <span className="font-medium">Font</span>
          <select aria-label="Interface font" value={preferences.fontFamily} onChange={(event) => setPreferences({ fontFamily: event.target.value as typeof preferences.fontFamily })} className={`${selectClass} block w-full`}>
            <option value="system">System</option>
            <option value="readable">High readability</option>
            <option value="rounded">Rounded</option>
            <option value="serif">Bookish serif</option>
            <option value="mono">Terminal mono</option>
            <option value="playful">Playful</option>
            <option value="typewriter">Typewriter</option>
          </select>
        </label>
        <label className="space-y-1 text-sm">
          <span className="font-medium">Density</span>
          <select aria-label="Information density" value={preferences.density} onChange={(event) => setPreferences({ density: event.target.value as typeof preferences.density })} className={`${selectClass} block w-full`}>
            <option value="compact">Compact</option>
            <option value="comfortable">Comfortable</option>
            <option value="spacious">Spacious</option>
          </select>
        </label>
        <label className="space-y-1 text-sm">
          <span className="font-medium">Corners</span>
          <select aria-label="Corner style" value={preferences.cornerStyle} onChange={(event) => setPreferences({ cornerStyle: event.target.value as typeof preferences.cornerStyle })} className={`${selectClass} block w-full`}>
            <option value="square">Square</option>
            <option value="soft">Soft</option>
            <option value="round">Round</option>
          </select>
        </label>
      </div>

      <label className="block space-y-1 text-sm">
        <span className="flex justify-between font-medium"><span>Text scale</span><span>{Math.round(preferences.fontScale * 100)}%</span></span>
        <input
          type="range"
          aria-label="Text scale"
          min="0.85"
          max="1.25"
          step="0.05"
          value={preferences.fontScale}
          onChange={(event) => setPreferences({ fontScale: Number(event.target.value) })}
          className="w-full accent-primary"
        />
      </label>

      <div className="grid gap-3 sm:grid-cols-2">
        <label className="flex items-center justify-between gap-3 text-sm">
          <span>High contrast</span>
          <input type="checkbox" aria-label="High contrast" checked={preferences.highContrast} onChange={(event) => setPreferences({ highContrast: event.target.checked })} className="h-5 w-5 accent-primary" />
        </label>
        <label className="flex items-center justify-between gap-3 text-sm">
          <span>Decorative background effects</span>
          <input type="checkbox" aria-label="Decorative background effects" checked={preferences.backgroundEffects} onChange={(event) => setPreferences({ backgroundEffects: event.target.checked })} className="h-5 w-5 accent-primary" />
        </label>
        <label className="space-y-1 text-sm sm:col-span-2">
          <span className="font-medium">Motion</span>
          <select aria-label="Motion preference" value={preferences.motion} onChange={(event) => setPreferences({ motion: event.target.value as typeof preferences.motion })} className={`${selectClass} block w-full sm:w-64`}>
            <option value="system">Follow system</option>
            <option value="reduced">Reduce motion</option>
            <option value="full">Full motion</option>
          </select>
          <span className="block text-xs text-muted-foreground">Controls nonessential animations, transitions, and smooth scrolling throughout the app.</span>
        </label>
      </div>

      <div className="flex flex-wrap gap-2">
        <button type="button" onClick={resetPreferences} className="rounded-md border border-input px-3 py-1.5 text-sm font-medium hover:bg-accent">
          Reset appearance
        </button>
        <button type="button" onClick={onResetLayout} className="rounded-md border border-input px-3 py-1.5 text-sm font-medium hover:bg-accent">
          Reset layout
        </button>
      </div>
    </div>
  );
}
