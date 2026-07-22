import { createContext, useContext, useEffect, useLayoutEffect, useState } from 'react';
import { getWindowsAccentColor } from '@/lib/tauri';

export type ColorMode = 'light' | 'dark' | 'system';
export type AccentMode = 'agora' | 'system' | 'custom';
export type FontFamily = 'system' | 'readable' | 'rounded' | 'serif' | 'mono' | 'playful' | 'typewriter';
export type CustomColorMode = 'theme' | 'custom';
export type Density = 'compact' | 'comfortable' | 'spacious';
export type CornerStyle = 'square' | 'soft' | 'round';
export type MotionPreference = 'system' | 'reduced' | 'full';

export interface UiPreferences {
  version: 1;
  colorMode: ColorMode;
  accentMode: AccentMode;
  customAccent: string;
  surfaceMode: CustomColorMode;
  customSurface: string;
  surfaceOpacity: number;
  navMode: CustomColorMode;
  customNav: string;
  navOpacity: number;
  backgroundMode: CustomColorMode;
  customBackground: string;
  textMode: CustomColorMode;
  customText: string;
  backgroundTextMode: CustomColorMode;
  customBackgroundText: string;
  fontFamily: FontFamily;
  fontScale: number;
  density: Density;
  cornerStyle: CornerStyle;
  motion: MotionPreference;
  highContrast: boolean;
  backgroundEffects: boolean;
}

interface UiPreferencesValues {
  preferences: UiPreferences;
  setPreferences: (update: Partial<UiPreferences>) => void;
  resetPreferences: () => void;
}

const STORAGE_KEY = 'agora-ui-preferences';
const LEGACY_STORAGE_KEY = 'agora-theme';
const HEX_COLOR = /^#[0-9a-f]{6}$/i;

export const DEFAULT_UI_PREFERENCES: UiPreferences = {
  version: 1,
  colorMode: 'system',
  accentMode: 'agora',
  customAccent: '#247786',
  surfaceMode: 'theme',
  customSurface: '#ffffff',
  surfaceOpacity: 1,
  navMode: 'theme',
  customNav: '#ffffff',
  navOpacity: 0.95,
  backgroundMode: 'theme',
  customBackground: '#f8f7f3',
  textMode: 'theme',
  customText: '#11233a',
  backgroundTextMode: 'theme',
  customBackgroundText: '#11233a',
  fontFamily: 'system',
  fontScale: 1,
  density: 'comfortable',
  cornerStyle: 'soft',
  motion: 'system',
  highContrast: false,
  backgroundEffects: true,
};

const UiPreferencesContext = createContext<UiPreferencesValues | null>(null);

function isOneOf<T extends string>(value: unknown, values: readonly T[]): value is T {
  return typeof value === 'string' && values.includes(value as T);
}

function validatePreferences(value: unknown): UiPreferences | null {
  if (!value || typeof value !== 'object') return null;
  const candidate = value as Partial<UiPreferences>;
  if (candidate.version !== 1) return null;

  const fontScale = typeof candidate.fontScale === 'number'
    ? Math.min(1.25, Math.max(0.85, candidate.fontScale))
    : DEFAULT_UI_PREFERENCES.fontScale;
  const surfaceOpacity = typeof candidate.surfaceOpacity === 'number'
    ? Math.min(1, Math.max(0.35, candidate.surfaceOpacity))
    : DEFAULT_UI_PREFERENCES.surfaceOpacity;
  const navOpacity = typeof candidate.navOpacity === 'number'
    ? Math.min(1, Math.max(0.35, candidate.navOpacity))
    : DEFAULT_UI_PREFERENCES.navOpacity;

  return {
    version: 1,
    colorMode: isOneOf(candidate.colorMode, ['light', 'dark', 'system'])
      ? candidate.colorMode
      : DEFAULT_UI_PREFERENCES.colorMode,
    accentMode: isOneOf(candidate.accentMode, ['agora', 'system', 'custom'])
      ? candidate.accentMode
      : DEFAULT_UI_PREFERENCES.accentMode,
    customAccent: typeof candidate.customAccent === 'string' && HEX_COLOR.test(candidate.customAccent)
      ? candidate.customAccent
      : DEFAULT_UI_PREFERENCES.customAccent,
    surfaceMode: isOneOf(candidate.surfaceMode, ['theme', 'custom'])
      ? candidate.surfaceMode
      : DEFAULT_UI_PREFERENCES.surfaceMode,
    customSurface: typeof candidate.customSurface === 'string' && HEX_COLOR.test(candidate.customSurface)
      ? candidate.customSurface
      : DEFAULT_UI_PREFERENCES.customSurface,
    surfaceOpacity,
    navMode: isOneOf(candidate.navMode, ['theme', 'custom'])
      ? candidate.navMode
      : DEFAULT_UI_PREFERENCES.navMode,
    customNav: typeof candidate.customNav === 'string' && HEX_COLOR.test(candidate.customNav)
      ? candidate.customNav
      : DEFAULT_UI_PREFERENCES.customNav,
    navOpacity,
    backgroundMode: isOneOf(candidate.backgroundMode, ['theme', 'custom'])
      ? candidate.backgroundMode
      : DEFAULT_UI_PREFERENCES.backgroundMode,
    customBackground: typeof candidate.customBackground === 'string' && HEX_COLOR.test(candidate.customBackground)
      ? candidate.customBackground
      : DEFAULT_UI_PREFERENCES.customBackground,
    textMode: isOneOf(candidate.textMode, ['theme', 'custom'])
      ? candidate.textMode
      : DEFAULT_UI_PREFERENCES.textMode,
    customText: typeof candidate.customText === 'string' && HEX_COLOR.test(candidate.customText)
      ? candidate.customText
      : DEFAULT_UI_PREFERENCES.customText,
    backgroundTextMode: isOneOf(candidate.backgroundTextMode, ['theme', 'custom'])
      ? candidate.backgroundTextMode
      : DEFAULT_UI_PREFERENCES.backgroundTextMode,
    customBackgroundText: typeof candidate.customBackgroundText === 'string' && HEX_COLOR.test(candidate.customBackgroundText)
      ? candidate.customBackgroundText
      : DEFAULT_UI_PREFERENCES.customBackgroundText,
    fontFamily: isOneOf(candidate.fontFamily, ['system', 'readable', 'rounded', 'serif', 'mono', 'playful', 'typewriter'])
      ? candidate.fontFamily
      : DEFAULT_UI_PREFERENCES.fontFamily,
    fontScale,
    density: isOneOf(candidate.density, ['compact', 'comfortable', 'spacious'])
      ? candidate.density
      : DEFAULT_UI_PREFERENCES.density,
    cornerStyle: isOneOf(candidate.cornerStyle, ['square', 'soft', 'round'])
      ? candidate.cornerStyle
      : DEFAULT_UI_PREFERENCES.cornerStyle,
    motion: isOneOf(candidate.motion, ['system', 'reduced', 'full'])
      ? candidate.motion
      : DEFAULT_UI_PREFERENCES.motion,
    highContrast: candidate.highContrast === true,
    backgroundEffects: candidate.backgroundEffects !== false,
  };
}

function hslStringToHex(value: string): string | null {
  const match = value.match(/^hsl\(\s*([\d.]+)[,\s]+([\d.]+)%[,\s]+([\d.]+)%\s*\)$/i);
  if (!match) return null;
  const h = Number(match[1]) / 360;
  const s = Number(match[2]) / 100;
  const l = Number(match[3]) / 100;
  const hueToRgb = (p: number, q: number, t: number) => {
    let normalized = t;
    if (normalized < 0) normalized += 1;
    if (normalized > 1) normalized -= 1;
    if (normalized < 1 / 6) return p + (q - p) * 6 * normalized;
    if (normalized < 1 / 2) return q;
    if (normalized < 2 / 3) return p + (q - p) * (2 / 3 - normalized) * 6;
    return p;
  };
  const q = l < 0.5 ? l * (1 + s) : l + s - l * s;
  const p = 2 * l - q;
  const channels = s === 0
    ? [l, l, l]
    : [hueToRgb(p, q, h + 1 / 3), hueToRgb(p, q, h), hueToRgb(p, q, h - 1 / 3)];
  return `#${channels.map((channel) => Math.round(channel * 255).toString(16).padStart(2, '0')).join('')}`;
}

function loadPreferences(): UiPreferences {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored) return validatePreferences(JSON.parse(stored)) ?? DEFAULT_UI_PREFERENCES;

    const legacy = localStorage.getItem(LEGACY_STORAGE_KEY);
    if (legacy) {
      const parsed = JSON.parse(legacy) as { theme?: unknown; accentColor?: unknown };
      const customAccent = typeof parsed.accentColor === 'string'
        ? hslStringToHex(parsed.accentColor)
        : null;
      return {
        ...DEFAULT_UI_PREFERENCES,
        colorMode: isOneOf(parsed.theme, ['light', 'dark', 'system']) ? parsed.theme : 'system',
        accentMode: customAccent ? 'custom' : 'agora',
        customAccent: customAccent ?? DEFAULT_UI_PREFERENCES.customAccent,
      };
    }
  } catch {
    // Corrupt preferences are ignored and replaced with safe defaults.
  }
  return DEFAULT_UI_PREFERENCES;
}

function hexToHsl(value: string): { h: number; s: number; l: number } | null {
  if (!HEX_COLOR.test(value)) return null;
  const red = Number.parseInt(value.slice(1, 3), 16) / 255;
  const green = Number.parseInt(value.slice(3, 5), 16) / 255;
  const blue = Number.parseInt(value.slice(5, 7), 16) / 255;
  const max = Math.max(red, green, blue);
  const min = Math.min(red, green, blue);
  const delta = max - min;
  let hue = 0;
  if (delta !== 0) {
    if (max === red) hue = 60 * (((green - blue) / delta) % 6);
    else if (max === green) hue = 60 * ((blue - red) / delta + 2);
    else hue = 60 * ((red - green) / delta + 4);
  }
  if (hue < 0) hue += 360;
  const lightness = (max + min) / 2;
  const saturation = delta === 0 ? 0 : delta / (1 - Math.abs(2 * lightness - 1));
  return { h: Math.round(hue), s: Math.round(saturation * 100), l: Math.round(lightness * 100) };
}

function parseAccent(value: string | null): { h: number; s: number; l: number } | null {
  if (!value) return null;
  const hex = hexToHsl(value);
  if (hex) return hex;
  const match = value.match(/^hsl\(\s*([\d.]+)[,\s]+([\d.]+)%[,\s]+([\d.]+)%\s*\)$/i);
  if (!match) return null;
  return { h: Number(match[1]), s: Number(match[2]), l: Number(match[3]) };
}

export function ThemeProvider({ children }: { children: React.ReactNode }) {
  const [preferences, setPreferencesState] = useState<UiPreferences>(loadPreferences);
  const [systemAccent, setSystemAccent] = useState<string | null>(null);
  const [systemDark, setSystemDark] = useState(() => window.matchMedia('(prefers-color-scheme: dark)').matches);
  const effectiveDark = preferences.colorMode === 'dark'
    || (preferences.colorMode === 'system' && systemDark);

  useLayoutEffect(() => {
    document.documentElement.classList.toggle('dark', effectiveDark);
  }, [effectiveDark]);

  useEffect(() => {
    const mediaQuery = window.matchMedia('(prefers-color-scheme: dark)');
    const updateSystemMode = () => setSystemDark(mediaQuery.matches);
    mediaQuery.addEventListener('change', updateSystemMode);
    return () => mediaQuery.removeEventListener('change', updateSystemMode);
  }, []);

  useEffect(() => {
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(preferences));
      localStorage.removeItem(LEGACY_STORAGE_KEY);
    } catch {
      // Storage can be unavailable or full; preferences still work for this session.
    }
  }, [preferences]);

  useEffect(() => {
    let cancelled = false;
    getWindowsAccentColor()
      .then((color) => {
        if (!cancelled) setSystemAccent(color);
      })
      .catch(() => {
        if (!cancelled) setSystemAccent(null);
      });
    return () => { cancelled = true; };
  }, []);

  useLayoutEffect(() => {
    const root = document.documentElement;
    const selectedAccent = preferences.accentMode === 'custom'
      ? preferences.customAccent
      : preferences.accentMode === 'system'
        ? systemAccent
        : null;
    const accent = parseAccent(selectedAccent);
    const dark = effectiveDark;

    if (accent) {
      const primaryLightness = dark ? Math.max(52, accent.l) : Math.min(42, accent.l);
      root.style.setProperty('--primary', `${accent.h} ${Math.max(45, accent.s)}% ${primaryLightness}%`);
      root.style.setProperty('--primary-foreground', primaryLightness > 58 ? '213 76% 8%' : '36 24% 98%');
      root.style.setProperty('--ring', `${accent.h} ${Math.max(45, accent.s)}% ${dark ? 62 : 45}%`);
      root.style.setProperty('--accent', `${accent.h} ${Math.max(25, accent.s * 0.65)}% ${dark ? 20 : 90}%`);
      root.style.setProperty('--accent-foreground', `${accent.h} ${Math.max(40, accent.s)}% ${dark ? 82 : 22}%`);
      root.style.setProperty('--brand', `${accent.h} ${Math.max(45, accent.s)}% ${primaryLightness}%`);
    } else {
      for (const property of ['--primary', '--primary-foreground', '--ring', '--accent', '--accent-foreground', '--brand']) {
        root.style.removeProperty(property);
      }
    }

    const background = preferences.backgroundMode === 'custom'
      ? hexToHsl(preferences.customBackground)
      : null;
    const text = preferences.textMode === 'custom'
      ? hexToHsl(preferences.customText)
      : null;
    const backgroundText = preferences.backgroundTextMode === 'custom'
      ? hexToHsl(preferences.customBackgroundText)
      : null;
    const surface = preferences.surfaceMode === 'custom'
      ? hexToHsl(preferences.customSurface)
      : null;
    const nav = preferences.navMode === 'custom'
      ? hexToHsl(preferences.customNav)
      : null;
    if (background) {
      root.style.setProperty('--background', `${background.h} ${background.s}% ${background.l}%`);
    } else {
      root.style.removeProperty('--background');
    }
    if (text) {
      const value = `${text.h} ${text.s}% ${text.l}%`;
      root.style.setProperty('--foreground', value);
      root.style.setProperty('--card-foreground', value);
      root.style.setProperty('--popover-foreground', value);
      root.style.setProperty('--secondary-foreground', value);
      root.style.setProperty('--muted-foreground', value);
    } else {
      root.style.removeProperty('--foreground');
      root.style.removeProperty('--card-foreground');
      root.style.removeProperty('--popover-foreground');
      root.style.removeProperty('--secondary-foreground');
      root.style.removeProperty('--muted-foreground');
    }
    if (backgroundText) {
      root.style.setProperty('--background-foreground', `${backgroundText.h} ${backgroundText.s}% ${backgroundText.l}%`);
    } else {
      root.style.removeProperty('--background-foreground');
    }
    if (surface) {
      const value = `${surface.h} ${surface.s}% ${surface.l}%`;
      const nestedLightness = dark ? Math.min(96, surface.l + 6) : Math.max(4, surface.l - 6);
      root.style.setProperty('--card', value);
      root.style.setProperty('--popover', value);
      root.style.setProperty('--muted', `${surface.h} ${Math.max(8, surface.s * 0.7)}% ${nestedLightness}%`);
      root.style.setProperty('--secondary', `${surface.h} ${Math.max(8, surface.s * 0.55)}% ${nestedLightness}%`);
    } else {
      root.style.removeProperty('--card');
      root.style.removeProperty('--popover');
      root.style.removeProperty('--muted');
      root.style.removeProperty('--secondary');
    }
    if (nav) {
      root.style.setProperty('--nav-surface', `${nav.h} ${nav.s}% ${nav.l}%`);
    } else {
      root.style.removeProperty('--nav-surface');
    }

    root.dataset.font = preferences.fontFamily;
    root.dataset.density = preferences.density;
    root.dataset.motion = preferences.motion;
    root.dataset.contrast = preferences.highContrast ? 'high' : 'normal';
    root.dataset.effects = preferences.backgroundEffects ? 'on' : 'off';
    root.style.setProperty('--font-scale', String(preferences.fontScale));
    root.style.setProperty('--surface-opacity', String(preferences.surfaceOpacity));
    root.style.setProperty('--nav-opacity', String(preferences.navOpacity));
    root.style.setProperty('--radius', preferences.cornerStyle === 'square' ? '0.125rem' : preferences.cornerStyle === 'round' ? '1rem' : '0.75rem');
  }, [preferences, systemAccent, effectiveDark]);

  const setPreferences = (update: Partial<UiPreferences>) => {
    setPreferencesState((current) => validatePreferences({ ...current, ...update, version: 1 }) ?? current);
  };

  return (
    <UiPreferencesContext.Provider
      value={{ preferences, setPreferences, resetPreferences: () => setPreferencesState(DEFAULT_UI_PREFERENCES) }}
    >
      {children}
    </UiPreferencesContext.Provider>
  );
}

export function useUiPreferences(): UiPreferencesValues {
  const context = useContext(UiPreferencesContext);
  if (!context) throw new Error('useUiPreferences must be used within ThemeProvider');
  return context;
}
