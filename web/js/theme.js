// Темы каталога: эталонная applyTheme, вычисляемые переменные акцента, миниатюры.

import { h } from './ui.js';

export function toRgb(hex) {
  let x = String(hex).replace('#', '');
  if (x.length === 3) x = x.split('').map((c) => c + c).join('');
  const n = parseInt(x, 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

export function toHex(rgb) {
  return '#' + rgb.map((v) => {
    const s = Math.max(0, Math.min(255, Math.round(v))).toString(16);
    return s.length < 2 ? '0' + s : s;
  }).join('');
}

export function mix(a, b, t) {
  const x = toRgb(a);
  const y = toRgb(b);
  return toHex([x[0] + (y[0] - x[0]) * t, x[1] + (y[1] - x[1]) * t, x[2] + (y[2] - x[2]) * t]);
}

export function luminance(hex) {
  const c = toRgb(hex).map((v) => {
    v /= 255;
    return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4);
  });
  return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
}

function rgbToHsl([r, g, b]) {
  r /= 255; g /= 255; b /= 255;
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  let hh = 0;
  let s = 0;
  const l = (max + min) / 2;
  if (max !== min) {
    const d = max - min;
    s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
    if (max === r) hh = (g - b) / d + (g < b ? 6 : 0);
    else if (max === g) hh = (b - r) / d + 2;
    else hh = (r - g) / d + 4;
    hh /= 6;
  }
  return [hh * 360, s, l];
}

function hslToRgb([hh, s, l]) {
  hh = ((hh % 360) + 360) % 360 / 360;
  if (s === 0) return [l * 255, l * 255, l * 255];
  const q = l < 0.5 ? l * (1 + s) : l + s - l * s;
  const p = 2 * l - q;
  const f = (tt) => {
    if (tt < 0) tt += 1;
    if (tt > 1) tt -= 1;
    if (tt < 1 / 6) return p + (q - p) * 6 * tt;
    if (tt < 1 / 2) return q;
    if (tt < 2 / 3) return p + (q - p) * (2 / 3 - tt) * 6;
    return p;
  };
  return [f(hh + 1 / 3) * 255, f(hh) * 255, f(hh - 1 / 3) * 255];
}

/** Второй цвет: тон акцента + 38°, насыщенность ≥ 0.55; светлота ≤ 0.42 (светлый) или ≥ 0.62 (тёмный). */
export function companion(base, dark) {
  const [hh, s, l] = rgbToHsl(toRgb(base));
  const nl = dark ? Math.max(l, 0.62) : Math.min(l, 0.42);
  return toHex(hslToRgb([hh + 38, Math.max(s, 0.55), nl]));
}

function setMeta(name, content) {
  let m = document.querySelector(`meta[name="${name}"]`);
  if (!m) {
    m = h('meta', { name });
    document.head.append(m);
  }
  m.setAttribute('content', content);
}

/** Эталонная функция каталога: атрибуты темы и вычисляемые переменные акцента. */
export function applyTheme(theme, dark, accentOverride) {
  const root = document.documentElement;
  root.setAttribute('data-skin', theme.id);
  root.classList.toggle('theme--dark', dark);
  const base = accentOverride || (dark ? theme.accentDark : theme.accentLight) || theme.accent || '#007aff';
  const tint = dark ? mix(base, '#ffffff', 0.18) : base;
  const press = dark ? mix(tint, '#ffffff', 0.22) : mix(tint, '#000000', 0.18);
  const rgb = toRgb(tint);
  root.style.setProperty('--tint', tint);
  root.style.setProperty('--tint-press', press);
  root.style.setProperty('--tint-soft', 'rgba(' + rgb.join(',') + ',' + (dark ? 0.24 : 0.14) + ')');
  root.style.setProperty('--on-tint', luminance(tint) > 0.5 ? '#101010' : '#ffffff');
  const own = (dark ? theme.companionDark : theme.companionLight) || theme.companion;
  const mono = theme.id === 'grid' || theme.id === 'board';
  root.style.setProperty('--warn', own && !accentOverride ? own : mono ? mix(tint, dark ? '#ffffff' : '#000000', 0.35) : companion(base, dark));
  const bars = dark ? theme.barsDark : theme.barsLight;
  if (bars) setMeta('theme-color', bars[1] >= 1 ? bars[0] : mix(bars[2], bars[0], bars[1]));
  return { tint, warn: root.style.getPropertyValue('--warn') };
}

let themes = [];
export async function loadThemes() {
  try {
    const r = await fetch('css/themes/index.json', { cache: 'no-cache' });
    themes = await r.json();
  } catch (_) {
    themes = [];
  }
  return themes;
}

export function allThemes() {
  return themes;
}

export function findTheme(id) {
  return themes.find((x) => x.id === id) || themes.find((x) => x.id === 'fluent') || themes[0] || { id: 'fluent', accent: '#0078d4', file: 'css/themes/fluent.css' };
}

function themeLink() {
  let l = document.getElementById('theme-css');
  if (!l) {
    l = h('link', { rel: 'stylesheet', id: 'theme-css' });
    // После base.css и перед app.css: порядок важен при равной специфичности.
    const app = document.querySelector('link[href$="css/app.css"]');
    document.head.insertBefore(l, app);
  }
  return l;
}

const media = window.matchMedia('(prefers-color-scheme: dark)');

/** Тёмный ли режим по настройке (system | light | dark). */
export function isDark(scheme) {
  if (scheme === 'dark') return true;
  if (scheme === 'light') return false;
  return media.matches;
}

let current = null;
/**
 * Применяет оформление из настроек ui: тема, режим, акцент, сплошные панели,
 * прозрачность фона под размытием окна.
 */
export function applyUi(ui, systemAccent) {
  current = ui;
  const theme = findTheme(ui.skin);
  const link = themeLink();
  if (link.getAttribute('href') !== theme.file) link.setAttribute('href', theme.file);
  let accent = null;
  if (ui.accent === 'system') accent = systemAccent || null;
  else if (/^#[0-9a-f]{6}$/i.test(ui.accent || '')) accent = ui.accent;
  applyTheme(theme, isDark(ui.scheme), accent);
  const root = document.documentElement;
  root.classList.toggle('bars--solid', !!ui.solid_bars);
  root.style.setProperty('--backdrop-opacity', String(ui.backdrop_opacity ?? 0.55));
}

media.addEventListener('change', () => {
  if (current && current.scheme === 'system') applyUi(current, window.__LMQ_ACCENT__);
});

/** Миниатюра темы: маленький экран с токенами самой темы. */
export function thumbnail(theme, dark, accent) {
  const mode = dark ? 'dark' : 'light';
  const tokens = (theme.preview && theme.preview[mode]) || {};
  const base = accent || (dark ? theme.accentDark : theme.accentLight) || theme.accent || '#007aff';
  const tint = dark ? mix(base, '#ffffff', 0.18) : base;
  const style = { '--tint': tint, '--on-tint': luminance(tint) > 0.5 ? '#101010' : '#ffffff' };
  for (const [k, v] of Object.entries(tokens)) style[k] = v;
  const el = h(
    'span.sp',
    { 'aria-hidden': 'true' },
    h('span.sp__nav', theme.name),
    h(
      'span.sp__body',
      h('span.sp__card', h('span.sp__line.sp__line--accent'), h('span.sp__line'), h('span.sp__line.sp__line--thin'), h('span.sp__btn', h('span.sp__btn-dot'))),
      h('span.sp__card', h('span.sp__line'), h('span.sp__line.sp__line--thin')),
    ),
    h('span.sp__tab', h('span.sp__tabitem', h('span.sp__tabdot.sp__tabdot--on')), h('span.sp__tabitem', h('span.sp__tabdot')), h('span.sp__tabitem', h('span.sp__tabdot')), h('span.sp__tabitem', h('span.sp__tabdot'))),
  );
  for (const [k, v] of Object.entries(style)) el.style.setProperty(k, v);
  return el;
}
