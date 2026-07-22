/** @type {import('tailwindcss').Config} */
import typography from '@tailwindcss/typography';

export default {
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
  ],
  darkMode: 'class',
  theme: {
    extend: {
      colors: {
        midnight: "hsl(var(--midnight))",
        aegean: "hsl(var(--aegean))",
        "sea-blue": "hsl(var(--sea-blue))",
        "civic-gold": "hsl(var(--civic-gold))",
        stone: "hsl(var(--stone))",
        brand: {
          50: '#f4f7fb',
          100: '#e8eff6',
          200: '#cbdceb',
          300: '#9ebfd6',
          400: '#6a9ebf',
          500: '#4682a9',
          600: '#35688a',
          700: '#2d5470',
          800: '#28475e',
          900: '#253d4f',
        },
        background: "hsl(var(--background))",
        "background-foreground": "hsl(var(--background-foreground))",
        foreground: "hsl(var(--foreground))",
        card: {
          DEFAULT: "hsl(var(--card))",
          foreground: "hsl(var(--card-foreground))",
        },
        popover: {
          DEFAULT: "hsl(var(--popover))",
          foreground: "hsl(var(--popover-foreground))",
        },
        primary: {
          DEFAULT: "hsl(var(--primary))",
          foreground: "hsl(var(--primary-foreground))",
        },
        secondary: {
          DEFAULT: "hsl(var(--secondary))",
          foreground: "hsl(var(--secondary-foreground))",
        },
        muted: {
          DEFAULT: "hsl(var(--muted))",
          foreground: "hsl(var(--muted-foreground))",
        },
        accent: {
          DEFAULT: "hsl(var(--accent))",
          foreground: "hsl(var(--accent-foreground))",
        },
        destructive: {
          DEFAULT: "hsl(var(--destructive))",
          foreground: "hsl(var(--destructive-foreground))",
        },
        border: "hsl(var(--border))",
        input: "hsl(var(--input))",
        ring: "hsl(var(--ring))",
      },
      borderRadius: {
        radius: "var(--radius)",
      },
    },
  },
  plugins: [
    typography,
  ],
};
