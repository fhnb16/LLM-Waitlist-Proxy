// LMQ — веб-панель. Каркас: шапка, вкладки (снизу на телефоне, сбоку с 1024px),
// маршруты по #/…, живые события сервера.

import { api, events, desktop, desktopCall, setUnauthorizedHandler } from './api.js';
import { initI18n, t } from './i18n.js';
import { applyUi, loadThemes } from './theme.js';
import { h, clear, icon, modal, navBtn, toast, errorToast } from './ui.js';
import { store, emit, reloadSettings } from './store.js';

const TABS = [
  { id: 'overview', label: 'Обзор', icon: 'overview' },
  { id: 'queue', label: 'Очередь', icon: 'queue' },
  { id: 'models', label: 'Модели', icon: 'models' },
  { id: 'engines', label: 'Движки', icon: 'engines' },
  { id: 'clients', label: 'Клиенты', icon: 'clients', extra: true },
  { id: 'logs', label: 'Журнал', icon: 'logs', extra: true },
  { id: 'sandbox', label: 'Песочница', icon: 'sandbox', extra: true },
  { id: 'debug', label: 'Отладка', icon: 'debug', extra: true },
  { id: 'settings', label: 'Настройки', icon: 'settings' },
  { id: 'more', label: 'Ещё', icon: 'more', more: true },
];

const SCREENS = {
  overview: () => import('./screens/overview.js'),
  queue: () => import('./screens/queue.js'),
  models: () => import('./screens/models.js'),
  engines: () => import('./screens/engines.js'),
  clients: () => import('./screens/clients.js'),
  logs: () => import('./screens/logs.js'),
  sandbox: () => import('./screens/sandbox.js'),
  debug: () => import('./screens/debug.js'),
  settings: () => import('./screens/settings.js'),
};

let shell = null;
let current = { destroy: [] };
let renderSeq = 0;

function buildShell() {
  const root = document.getElementById('app');
  clear(root);
  const navLeft = h('div.nav-bar__side', { id: 'nav-left' });
  const navTitle = h('h1.nav-bar__title', 'LMQ');
  const navRight = h('div.nav-bar__side.nav-bar__side--right');
  const screen = h('main.app__screen', { id: 'screen', tabindex: '-1' });
  const tabs = h('nav.tab-bar', { 'aria-label': t('Разделы') });
  const badges = {};
  tabs.append(
    h('div.app__brand', h('img.app__brand-icon', { src: 'img/icon.svg', alt: '' }), 'LMQ', h('span.app__brand-ver', store.overview ? 'v' + store.overview.version : '')),
  );
  for (const tab of TABS) {
    const badge = h('span.tab-bar__badge', { hidden: true });
    badges[tab.id] = badge;
    const b = h(
      'button.tab-bar__item',
      {
        type: 'button',
        class: [tab.extra ? 'app__tab--extra' : '', tab.more ? 'app__tab--more' : ''].join(' '),
        dataset: { tab: tab.id },
        on: { click: () => (tab.more ? openMore() : go('#/' + tab.id)) },
      },
      icon(tab.icon),
      h('span.tab-bar__label', t(tab.label)),
      badge,
    );
    tabs.append(b);
  }
  root.append(
    h('header.nav-bar', navLeft, navTitle, navRight),
    h('div.app__views', screen),
    tabs,
  );
  shell = { navLeft, navTitle, navRight, screen, tabs, badges };
}

function openMore() {
  const items = TABS.filter((x) => x.extra);
  const extra = [
    { id: 'settings/schedule', label: 'Расписание', icon: 'schedule' },
    { id: 'settings/appearance', label: 'Оформление', icon: 'palette' },
    { id: 'settings/about', label: 'О программе', icon: 'info' },
  ];
  const m = modal({
    title: t('Разделы'),
    body: h('div.sheet', [...items, ...extra].map((it) =>
      h('button.sheet__item', { type: 'button', on: { click: () => { m.close(); go('#/' + it.id); } } }, icon(it.icon, 'nav-bar__icon'), t(it.label)),
    )),
  });
}

export function go(hash) {
  if (location.hash === hash) route();
  else location.hash = hash;
}

function parseHash() {
  const raw = (location.hash || '#/overview').replace(/^#\/?/, '');
  const [path, query] = raw.split('?');
  const parts = path.split('/').filter(Boolean).map(decodeURIComponent);
  const params = new URLSearchParams(query || '');
  return { parts: parts.length ? parts : ['overview'], params };
}

async function route() {
  if (!shell) return;
  const seq = ++renderSeq;
  for (const fn of current.destroy) {
    try {
      fn();
    } catch (_) { /* экран уже закрыт */ }
  }
  current = { destroy: [] };
  const { parts, params } = parseHash();
  const name = SCREENS[parts[0]] ? parts[0] : 'overview';
  for (const b of shell.tabs.querySelectorAll('.tab-bar__item')) {
    const active = b.dataset.tab === name;
    b.classList.toggle('tab-bar__item--active', active);
    if (active) b.setAttribute('aria-current', 'page');
    else b.removeAttribute('aria-current');
  }
  clear(shell.navLeft);
  clear(shell.navRight);
  shell.navLeft.classList.remove('has-back');
  const screen = shell.screen;
  clear(screen);
  screen.className = 'app__screen';
  const ctx = {
    parts,
    params,
    root: screen,
    go,
    title(text) {
      shell.navTitle.textContent = text;
      document.title = text + ' — LMQ';
    },
    back(hash, label = t('Назад')) {
      clear(shell.navLeft);
      shell.navLeft.classList.add('has-back');
      shell.navLeft.append(
        h('button.nav-bar__btn.nav-bar__btn--back', { type: 'button', on: { click: () => go(hash) } },
          icon('back', 'nav-bar__chevron'),
          h('span.nav-bar__btn-label', label)),
      );
    },
    actions(...nodes) {
      clear(shell.navRight);
      shell.navRight.append(...nodes.filter(Boolean));
    },
    onDestroy(fn) {
      current.destroy.push(fn);
    },
    alive() {
      return seq === renderSeq;
    },
  };
  try {
    const mod = await SCREENS[name]();
    if (seq !== renderSeq) return;
    await mod.render(ctx);
  } catch (e) {
    console.error(e);
    if (seq === renderSeq) {
      clear(screen);
      screen.append(h('div.state', h('p.state__title', t('Экран не открылся')), h('p.state__text', e.message || String(e))));
    }
  }
  if (seq === renderSeq) window.scrollTo(0, 0);
}

function updateBadges() {
  if (!shell || !store.overview) return;
  const c = store.overview.counts || {};
  const set = (id, n, kind) => {
    const b = shell.badges[id];
    if (!b) return;
    b.hidden = !n;
    b.textContent = n > 99 ? '99+' : String(n);
    b.title = kind || '';
  };
  set('queue', c.dead || 0, t('не доставлено'));
  set('engines', (store.overview.engines || []).filter((e) => e.status === 'failed' || e.status === 'crashed').length);
  const ver = shell.tabs.querySelector('.app__brand-ver');
  if (ver) ver.textContent = 'v' + store.overview.version;
}

function connect() {
  const stop = events((type, data) => {
    if (type === 'tick') {
      store.overview = data;
      updateBadges();
    }
    if (type === 'config') {
      reloadSettings()
        .then((s) => applyUi(s.file.ui, store.systemAccent))
        .catch(() => {});
    }
    emit(type, data);
  }, (ok) => {
    store.connected = ok;
    emit('connection', ok);
  });
  window.addEventListener('beforeunload', stop);
}

async function showLogin() {
  const root = document.getElementById('app');
  clear(root);
  const inp = h('input.field__input', { type: 'password', autocomplete: 'current-password', placeholder: t('токен администратора') });
  const err = h('p.group__note.group__note--warn', { hidden: true });
  const submit = async (e) => {
    e.preventDefault();
    err.hidden = true;
    try {
      await api('/admin/api/login', { method: 'POST', body: { token: inp.value.trim() } });
      location.reload();
    } catch (ex) {
      err.textContent = t(ex.message);
      err.hidden = false;
      inp.select();
    }
  };
  root.append(
    h('div.login', h('form.login__panel', { on: { submit } },
      h('img.login__logo', { src: 'img/icon.svg', alt: '' }),
      h('h1.login__title', 'LMQ'),
      h('p.login__sub', t('Сервер очереди для UniversalBot')),
      h('div.list', h('label.field.field--stack', h('span.field__label', t('Токен администратора')), inp)),
      err,
      h('button.btn.btn--filled', { type: 'submit' }, t('Войти')),
      h('p.footnote', { style: { textAlign: 'center' } }, t('Токен показывается при первом запуске сервера. Новый: lmq-server admin-token reset')),
    )),
  );
  setTimeout(() => inp.focus(), 50);
}

async function boot() {
  let lang = 'auto';
  try {
    const session = await api('/admin/api/session');
    if (!session.authenticated) {
      await initI18n('auto');
      await loadThemes();
      applyUi({ skin: 'fluent', scheme: 'system', accent: '' });
      await showLogin();
      return;
    }
    await loadThemes();
    const s = await reloadSettings();
    lang = s.file.ui.language || 'auto';
    await initI18n(lang);
    if (desktop) {
      try {
        store.systemAccent = await desktopCall('system_accent');
        window.__LMQ_ACCENT__ = store.systemAccent;
      } catch (_) { /* без системного акцента */ }
      document.documentElement.classList.toggle('vibrancy', s.file.ui.backdrop !== 'none');
    }
    applyUi(s.file.ui, store.systemAccent);
    try {
      store.overview = await api('/admin/api/overview');
    } catch (_) { /* придёт с первым tick */ }
  } catch (e) {
    await initI18n('auto');
    await loadThemes();
    applyUi({ skin: 'fluent', scheme: 'system', accent: '' });
    const root = document.getElementById('app');
    clear(root);
    root.append(h('div.state', h('p.state__title', t('Сервер недоступен')), h('p.state__text', e.message), h('button.btn.btn--inline.state__action', { type: 'button', on: { click: () => location.reload() } }, t('Повторить'))));
    return;
  }
  setUnauthorizedHandler(() => {
    toast(t('Сессия закончилась — войдите снова'), 'warn');
    setTimeout(() => location.reload(), 1200);
  });
  buildShell();
  updateBadges();
  connect();
  window.addEventListener('hashchange', route);
  if (!location.hash) history.replaceState(null, '', '#/overview');
  await route();
}

window.addEventListener('unhandledrejection', (e) => {
  if (e.reason && e.reason.name === 'AbortError') return;
  errorToast(e.reason);
});

boot();

export { navBtn };
