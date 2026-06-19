/** @type {import('tailwindcss').Config} */
import typography from '@tailwindcss/typography';

export default {
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
  ],
  darkMode: 'media',
  theme: {
    extend: {
      colors: {
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
      },
    },
  },
  plugins: [
    typography,
  ],
};
