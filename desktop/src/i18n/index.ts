import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';
import LanguageDetector from 'i18next-browser-languagedetector';
import en from './locales/en.json';
import es from './locales/es.json';
import zh from './locales/zh.json';
import hi from './locales/hi.json';
import bn from './locales/bn.json';
import pt from './locales/pt.json';
import ru from './locales/ru.json';
import ja from './locales/ja.json';
import ar from './locales/ar.json';
import de from './locales/de.json';
import ko from './locales/ko.json';
import tr from './locales/tr.json';
import vi from './locales/vi.json';
import fr from './locales/fr.json';
import ta from './locales/ta.json';
import te from './locales/te.json';
import ur from './locales/ur.json';
import it from './locales/it.json';
import nl from './locales/nl.json';
import pl from './locales/pl.json';

i18n
  .use(LanguageDetector)
  .use(initReactI18next)
  .init({
    resources: {
      en: { translation: en },
      es: { translation: es },
      zh: { translation: zh },
      hi: { translation: hi },
      bn: { translation: bn },
      pt: { translation: pt },
      ru: { translation: ru },
      ja: { translation: ja },
      ar: { translation: ar },
      de: { translation: de },
      ko: { translation: ko },
      tr: { translation: tr },
      vi: { translation: vi },
      fr: { translation: fr },
      ta: { translation: ta },
      te: { translation: te },
      ur: { translation: ur },
      it: { translation: it },
      nl: { translation: nl },
      pl: { translation: pl },
    },
    fallbackLng: 'en',
    supportedLngs: [
      'en', 'es', 'zh', 'hi', 'bn', 'pt', 'ru', 'ja', 'ar',
      'de', 'ko', 'tr', 'vi', 'fr', 'ta', 'te', 'ur', 'it', 'nl', 'pl',
    ],
    interpolation: {
      escapeValue: false, // React already escapes by default.
    },
    detection: {
      order: ['localStorage', 'navigator'],
      caches: ['localStorage'],
      lookupLocalStorage: 'agora_language',
    },
  });

export default i18n;
