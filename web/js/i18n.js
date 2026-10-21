// Локализация: строки в коде — по-русски, словарь en.json переводит их.
// Нет перевода — показывается русская строка (а не ключ).

let lang = 'ru';
let dict = {};

export function resolveLang(pref) {
  if (pref === 'ru' || pref === 'en') return pref;
  const nav = (navigator.languages && navigator.languages[0]) || navigator.language || 'ru';
  return nav.toLowerCase().startsWith('ru') ? 'ru' : 'en';
}

export async function initI18n(pref) {
  lang = resolveLang(pref);
  dict = {};
  if (lang !== 'ru') {
    try {
      const r = await fetch(`i18n/${lang}.json`, { cache: 'no-cache' });
      if (r.ok) dict = await r.json();
    } catch (_) { /* остаёмся на русском */ }
  }
  document.documentElement.lang = lang;
  return lang;
}

export function currentLang() {
  return lang;
}

/** Перевод строки; `{name}` заменяются значениями из vars. */
export function t(s, vars) {
  let r = lang === 'ru' ? s : (dict[s] ?? s);
  if (vars) r = r.replace(/\{(\w+)\}/g, (_, k) => (vars[k] ?? '') + '');
  return r;
}

/** Число со словом: tn(5, 'задание', 'задания', 'заданий'). */
export function tn(n, one, few, many) {
  if (lang !== 'ru') return `${n} ${t(Math.abs(n) === 1 ? one : many)}`;
  const a = Math.abs(n) % 100;
  const b = a % 10;
  const w = a > 10 && a < 20 ? many : b > 1 && b < 5 ? few : b === 1 ? one : many;
  return `${n} ${w}`;
}

export function locale() {
  return lang === 'ru' ? 'ru-RU' : 'en-US';
}
