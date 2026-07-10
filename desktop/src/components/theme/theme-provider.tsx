import { createContext, useContext, useEffect, useLayoutEffect, useState } from "react";
import { getWindowsAccentColor } from "@/lib/tauri";

type Theme = "light" | "dark" | "system";

interface ThemeValues {
  theme: Theme;
  accentColor: string | null;
  setTheme: (theme: Theme) => void;
  setAccentColor: (color: string | null) => void;
}

const ThemeContext = createContext<ThemeValues | null>(null);

const STORAGE_KEY = "agora-theme";

function loadStored(): { theme: Theme; accentColor: string | null } | null {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) return JSON.parse(raw);
  } catch {
    /* corrupted — ignore */
  }
  return null;
}

function storeStored(data: { theme: Theme; accentColor: string | null }) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(data));
  } catch {
    /* quota — ignore */
  }
}

/* Tauri invoke helper — wrapped in try/catch. Falls back to null (amber default). */
async function fetchWindowsAccentColor(): Promise<string | null> {
  try {
    return await getWindowsAccentColor();
  } catch {
    return null;
  }
}

export function ThemeProvider({ children }: { children: React.ReactNode }) {
  // Synchronous initialization from localStorage (runs before first paint).
  const [theme, setThemeState] = useState<Theme>(() => {
    const stored = loadStored();
    return stored?.theme ?? "system";
  });

  const [accentColor, setAccentColorState] = useState<string | null>(() => {
    const stored = loadStored();
    return stored?.accentColor ?? null;
  });

  // Apply light/dark class before first paint so there is no flash.
  useLayoutEffect(() => {
    const mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");
    const isDark =
      theme === "dark" ||
      (theme === "system" && mediaQuery.matches);
    document.documentElement.classList.toggle("dark", isDark);
  }, [theme]);

  // Listen for OS theme changes after mount.
  useEffect(() => {
    const mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");
    const handler = () => {
      if (theme === "system") {
        document.documentElement.classList.toggle("dark", mediaQuery.matches);
      }
    };
    mediaQuery.addEventListener("change", handler);
    return () => mediaQuery.removeEventListener("change", handler);
  }, [theme]);

  // Persist theme + accent changes to localStorage.
  useEffect(() => {
    storeStored({ theme, accentColor });
  }, [theme, accentColor]);

  // Fetch Windows accent color as optional enhancement — never blocks rendering
  // and never overwrites a stored custom accent.
  useEffect(() => {
    let cancelled = false;
    fetchWindowsAccentColor().then((color) => {
      if (!cancelled && color) {
        // Use functional updater so a stored custom accent is never overwritten.
        setAccentColorState((prev) => prev ?? color);
      }
    });
    return () => {
      cancelled = true;
    };
  }, []);

  // Parse an accent value into raw HSL components.
  // The Windows backend returns "hsl(210 50% 40%)", but Tailwind
  // references --accent as hsl(var(--accent)), so we must store only
  // the space-separated components: "210 50% 40%".
  const accentCssValue = accentColor
    ? accentColor.replace(/^hsl\(/i, "").replace(/\)$/, "")
    : null;

  // Apply accent before first paint so the stored custom accent
  // is visible immediately with no flash.
  useLayoutEffect(() => {
    const root = document.documentElement;
    if (accentCssValue) {
      root.style.setProperty("--accent", accentCssValue);
    } else {
      root.style.removeProperty("--accent");
    }
  }, [accentCssValue]);

  const setTheme = (t: Theme) => setThemeState(t);
  const setAccentColor = (c: string | null) => setAccentColorState(c);

  return (
    <ThemeContext.Provider
      value={{ theme, accentColor, setTheme, setAccentColor }}
    >
      {children}
    </ThemeContext.Provider>
  );
}

export function useTheme(): ThemeValues {
  const ctx = useContext(ThemeContext);
  if (!ctx) throw new Error("useTheme must be used within ThemeProvider");
  return ctx;
}
