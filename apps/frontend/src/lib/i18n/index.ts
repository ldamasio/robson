import { browser } from '$app/environment';
import { init, register, getLocaleFromNavigator, locale } from 'svelte-i18n';

register('pt-BR', () => import('./pt-BR.json'));
register('en', () => import('./en.json'));

export function detectLocale(): string {
  if (!browser) return 'pt-BR';
  const host = window.location.hostname;
  const cookieLocale = document.cookie
    .split('; ')
    .find((c) => c.startsWith('locale='))
    ?.split('=')[1];
  if (cookieLocale === 'pt-BR' || cookieLocale === 'en') return cookieLocale;
  if (host.endsWith('.ia.br')) return 'pt-BR';
  if (host.endsWith('.ch')) return 'en';
  return getLocaleFromNavigator() ?? 'pt-BR';
}

init({
  fallbackLocale: 'en',
  initialLocale: detectLocale()
});

export { locale };
