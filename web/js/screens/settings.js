import { api, desktop } from '../api.js';
import { t } from '../i18n.js';
import { h, clear, hero, group, list, cell, cstate, field, input, combo, numberInput, select, switchEl, icon, toast, errorToast, callout, clone, navBtn, getPath, setPath, state } from '../ui.js';
import { store, saveSettings, isLocked } from '../store.js';

const LEVELS = ['error', 'warn', 'info', 'debug', 'trace'].map((v) => ({ value: v, label: v }));

/** Разделы, собранные из полей: путь в settings.yaml, тип, подпись, подсказка, переменная окружения. */
const SECTIONS = [
  {
    id: 'server', title: 'Сервер', icon: 'app',
    note: 'Смена адреса или лимитов перезапускает слушатель внутри процесса.',
    fields: [
      { path: 'server.listen', label: 'Адрес API', env: 'LISTEN', placeholder: '0.0.0.0:8800', hint: 'Наружу — только через обратный прокси с HTTPS.' },
      { path: 'server.admin_listen', label: 'Адрес панели', env: 'ADMIN_LISTEN', placeholder: '127.0.0.1:8801', hint: 'Пусто — панель выключена.' },
      { path: 'server.max_body_mb', label: 'Предел тела, МБ', env: 'MAX_BODY_MB', type: 'number', min: 1, max: 512 },
      { path: 'server.max_messages', label: 'Сообщений в задании', type: 'number', min: 1 },
      { path: 'server.max_string_kb', label: 'Длина текста, КБ', type: 'number', min: 1, hint: 'Картинки base64 не считаются.' },
      { path: 'server.body_read_timeout', label: 'Чтение тела, с', type: 'number', min: 1, hint: 'Защита от медленных клиентов.' },
      { path: 'server.admin_cors_origins', label: 'CORS панели', type: 'list', placeholder: 'https://admin.example.com', hint: 'Через запятую. Окно приложения разрешено всегда.' },
    ],
  },
  {
    id: 'queue', title: 'Очередь и лимиты', icon: 'queue',
    fields: [
      { path: 'queue.max_queue', label: 'Всего в очереди', env: 'MAX_QUEUE', type: 'number', min: 1 },
      { path: 'queue.max_queue_per_chat', label: 'На один чат', env: 'MAX_QUEUE_PER_CHAT', type: 'number', min: 1, hint: 'Сверх — 429 с Retry-After.' },
      { path: 'queue.max_wait', label: 'Ждать не дольше, с', env: 'MAX_WAIT', type: 'number', min: 1, hint: 'Не начатое за это время задание — expired, бот промолчит.' },
      { path: 'queue.upstream_timeout', label: 'Ответ модели, с', env: 'UPSTREAM_TIMEOUT', type: 'number', min: 1 },
      { path: 'queue.sync_timeout', label: 'Синхронный режим, с', env: 'SYNC_TIMEOUT', type: 'number', min: 1, hint: '«Проверить» и «Песочница» в боте.' },
      { path: 'queue.upstream_retries', label: 'Повторов при ошибке', env: 'UPSTREAM_RETRIES', type: 'number', min: 0, max: 10 },
      { path: 'queue.supersede_same_chat', label: 'Вытеснять старое задание чата', env: 'SUPERSEDE_SAME_CHAT', type: 'bool', hint: 'Не начатое задание чата завершается superseded, если пришло новое.' },
      { path: 'queue.private_first', label: 'Личные чаты раньше групп', env: 'PRIVATE_FIRST', type: 'bool' },
    ],
  },
  {
    id: 'callback', title: 'Доставка боту', icon: 'clients',
    note: 'callback.url каждого задания проверяется по белому списку хостов (защита от SSRF).',
    fields: [
      { path: 'callback.url', label: 'Адрес колбэка', env: 'BOT_CALLBACK_URL', placeholder: 'https://bot.example.com/webhook/llm', hint: 'Если задание пришло без callback.' },
      { path: 'callback.token', label: 'Токен колбэка', env: 'BOT_CALLBACK_TOKEN', type: 'secret', hint: 'Заголовок X-Callback-Token.' },
      { path: 'callback.allowed_hosts', label: 'Разрешённые хосты', env: 'CALLBACK_ALLOWED_HOSTS', type: 'list', placeholder: 'bot.example.com, *.example.org', hint: 'Пусто — только хост адреса колбэка.' },
      { path: 'callback.allow_http_private', label: 'http:// для localhost и частных сетей', env: 'CALLBACK_ALLOW_HTTP_PRIVATE', type: 'bool' },
      { path: 'callback.retries', label: 'Повторов доставки', env: 'CALLBACK_RETRIES', type: 'number', min: 0, max: 50, hint: 'Паузы 2, 5, 15, 30, 60 с.' },
      { path: 'callback.timeout', label: 'Таймаут, с', type: 'number', min: 1, max: 120 },
    ],
  },
  {
    id: 'storage', title: 'Хранение и журнал', icon: 'logs',
    fields: [
      { path: 'storage.db_path', label: 'База данных', env: 'DB_PATH', placeholder: 'queue.sqlite в папке данных', hint: 'Меняется после перезапуска.' },
      { path: 'storage.models_config', label: 'Внешний файл моделей', env: 'MODELS_CONFIG', placeholder: 'config.yaml', hint: 'Формат §5.4. Пусто — модели из настроек.' },
      { path: 'storage.retention_hours', label: 'Хранить задания, ч', env: 'RETENTION_HOURS', type: 'number', min: 1 },
      { path: 'storage.stats_retention_days', label: 'Хранить статистику, дн', env: 'STATS_RETENTION_DAYS', type: 'number', min: 1 },
      { path: 'logging.level', label: 'Уровень журнала', env: 'LOG_LEVEL', type: 'select', options: LEVELS },
      { path: 'logging.content', label: 'Писать тексты сообщений', env: 'LOG_CONTENT', type: 'bool', hint: 'Персональные данные. Оставьте выключенным.' },
      { path: 'logging.keep_days', label: 'Файлы журнала, дн', type: 'number', min: 1 },
      { path: 'logging.requests', label: 'Подробный журнал запросов', type: 'bool', hint: 'Строение запроса бота, что ушло модели, ответы апстрима и бота. Тексты — только вместе с «Писать тексты».' },
      { path: 'metrics.enabled', label: 'Метрики Prometheus', type: 'bool', hint: 'GET /metrics на адресе API.' },
      { path: 'metrics.token', label: 'Токен метрик', env: 'METRICS_TOKEN', type: 'secret', hint: 'Пусто — /metrics только с localhost.' },
    ],
  },
  {
    id: 'network', title: 'Сеть и маршруты', icon: 'clients',
    note: 'Через какой интерфейс или прокси ходит каждое направление. Например: модели — через VPN (happ-tun или socks5-прокси VPN-клиента), бот — напрямую (Ethernet). Пусто — системный маршрут.',
    fields: [
      { path: 'network.upstream.interface', label: 'Модели: интерфейс', type: 'iface', placeholder: 'системный маршрут', hint: 'Имя (Ethernet, happ-tun, eth0) или IP. У апстрима можно задать свой.' },
      { path: 'network.upstream.proxy', label: 'Модели: прокси', type: 'proxy', placeholder: 'как в системе', hint: 'socks5://, socks5h://, http://. none — напрямую.' },
      { path: 'network.callback.interface', label: 'Бот: интерфейс', type: 'iface', placeholder: 'системный маршрут', hint: 'Колбэки на адрес бота.' },
      { path: 'network.callback.proxy', label: 'Бот: прокси', type: 'proxy', placeholder: 'как в системе' },
      { path: 'network.telegram.interface', label: 'Telegram: интерфейс', type: 'iface', placeholder: 'системный маршрут', hint: 'Только для протокола 1.' },
      { path: 'network.telegram.proxy', label: 'Telegram: прокси', type: 'proxy', placeholder: 'как в системе' },
    ],
  },
  {
    id: 'breaker', title: 'Выключатель апстримов', icon: 'engines',
    note: 'Упавший апстрим временно исключается; фоновая проверка возвращает его, когда он ожил.',
    fields: [
      { path: 'breaker.enabled', label: 'Включён', type: 'bool' },
      { path: 'breaker.failure_threshold', label: 'Неудач подряд', type: 'number', min: 1, max: 100 },
      { path: 'breaker.open_seconds', label: 'Исключать на, с', type: 'number', min: 1 },
      { path: 'breaker.health_interval', label: 'Проверка раз в, с', type: 'number', min: 0, hint: 'GET {base}/models. 0 — не проверять.' },
    ],
  },
  {
    id: 'protocol1', title: 'Протокол 1 (Telegram)', icon: 'update',
    note: 'Старый режим: сервер сам отправляет ответ в Telegram. Теряются форматирование, темы форума и память — включайте, только если без него не обойтись.',
    fields: [
      { path: 'protocol1.enabled', label: 'Разрешить', env: 'PROTOCOL1_ENABLED', type: 'bool' },
      { path: 'protocol1.telegram_bot_token', label: 'Токен бота', env: 'TELEGRAM_BOT_TOKEN', type: 'secret' },
      { path: 'protocol1.telegram_api', label: 'Адрес Bot API', placeholder: 'https://api.telegram.org' },
    ],
  },
];

/** Готовые значения числовых полей и их единица (s — секунды, h — часы). */
const PRESETS = {
  'server.max_body_mb': { presets: [4, 8, 16, 32, 64] },
  'server.max_messages': { presets: [100, 200, 500, 1000] },
  'server.max_string_kb': { presets: [128, 256, 512, 1024, 2048] },
  'server.body_read_timeout': { presets: [10, 30, 60, 120], unit: 's' },
  'queue.max_queue': { presets: [50, 100, 200, 500, 1000] },
  'queue.max_queue_per_chat': { presets: [1, 2, 3, 5, 10] },
  'queue.max_wait': { presets: [60, 120, 300, 600, 1800], unit: 's' },
  'queue.upstream_timeout': { presets: [30, 60, 120, 300, 600], unit: 's' },
  'queue.sync_timeout': { presets: [15, 30, 60, 120], unit: 's' },
  'queue.upstream_retries': { presets: [0, 1, 2, 3, 5] },
  'callback.retries': { presets: [0, 3, 6, 10, 20] },
  'callback.timeout': { presets: [5, 10, 15, 30, 60], unit: 's' },
  'storage.retention_hours': { presets: [6, 24, 72, 168, 720], unit: 'h' },
  'storage.stats_retention_days': { presets: [7, 14, 30, 90, 365] },
  'logging.keep_days': { presets: [3, 7, 14, 30, 90] },
  'breaker.failure_threshold': { presets: [1, 2, 3, 5, 10] },
  'breaker.open_seconds': { presets: [10, 30, 60, 120, 300], unit: 's' },
  'breaker.health_interval': { presets: [0, 5, 15, 30, 60], unit: 's', zero: 'не проверять' },
};

/** Интерфейсы системы и найденные локальные прокси — один запрос на экран. */
let netCache = null;
export function netOptions() {
  if (!netCache) netCache = api('/admin/api/network').catch(() => ({ interfaces: [], proxies: [] }));
  return netCache;
}

export function ifaceOptions(n, inherit) {
  return [
    inherit ? { value: '', label: t('как в «Сеть и маршруты»') } : { value: '', label: t('Системный маршрут') },
    { value: 'system', label: 'system', hint: t('Системный маршрут') },
    ...(n.interfaces || []).map((i) => ({ value: i.name, hint: [...i.ipv4, ...i.ipv6].join(', ') })),
  ];
}

export function proxyOptions(n, inherit) {
  return [
    inherit ? { value: '', label: t('как в «Сеть и маршруты»') } : { value: '', label: t('как в системе'), hint: 'HTTP(S)_PROXY' },
    { value: 'none', label: 'none', hint: t('напрямую') },
    ...(n.proxies || []).map((p) => ({ value: p.url, hint: t('найден: {what}', { what: p.label }) })),
  ];
}

const PAGES = {
  schedule: () => import('./schedule.js'),
  profiles: () => import('./profiles.js'),
  appearance: () => import('./appearance.js'),
  application: () => import('./application.js'),
  updates: () => import('./updates.js'),
  about: () => import('./about.js'),
};

export async function render(ctx) {
  const sub = ctx.parts[1];
  if (sub && PAGES[sub]) {
    ctx.back('#/settings', t('Настройки'));
    const mod = await PAGES[sub]();
    return mod.render(ctx);
  }
  const section = SECTIONS.find((s) => s.id === sub);
  if (section) return renderSection(ctx, section);
  return renderIndex(ctx);
}

function renderIndex(ctx) {
  ctx.title(t('Настройки'));
  const root = ctx.root;
  root.classList.add('app__screen--grid');
  const s = store.settings;
  const link = (id, title, ic, value) => cell({
    label: h('span', { style: { display: 'inline-flex', gap: '12px', alignItems: 'center' } }, icon(ic, 'nav-bar__icon'), t(title)),
    value,
    onClick: () => ctx.go('#/settings/' + id),
  });
  const ui = s.file.ui;
  root.append(
    hero(t('Настройки'), t('Профиль {p} · {f}', { p: s.profile, f: s.paths.settings_file })),
    group(t('Сервер очереди'), list(...SECTIONS.map((x) => link(x.id, x.title, x.icon)))),
    group(t('Работа'), list(
      link('schedule', 'Расписание', 'schedule', s.file.schedule.length ? String(s.file.schedule.length) : t('нет правил')),
      link('profiles', 'Профили, экспорт и импорт', 'profiles', s.profile),
    )),
    group(t('Интерфейс'), list(
      link('appearance', 'Оформление', 'palette', ui.skin),
      desktop ? link('application', 'Приложение', 'app', ui.mode === 'native' ? t('нативное окно') : t('веб-окно')) : null,
      link('updates', 'Обновления', 'update'),
      link('about', 'О программе', 'info'),
    )),
    s.locked.length ? group(t('Задано в окружении'), list(cell({ label: s.locked.join(', '), wrap: true })), t('Эти значения берутся из переменных окружения и перекрывают файл.')) : null,
  );
}

function renderSection(ctx, sec) {
  ctx.back('#/settings', t('Настройки'));
  ctx.title(t(sec.title));
  const root = ctx.root;
  const draft = clone(store.settings.file);
  const effective = store.settings.effective;
  const mask = store.settings.mask;
  let dirty = false;
  const saveBtn = navBtn(t('Сохранить'), save);
  saveBtn.disabled = true;
  ctx.actions(saveBtn);
  const err = h('div');
  const touch = () => {
    dirty = true;
    saveBtn.disabled = false;
  };
  async function save() {
    clear(err);
    try {
      await saveSettings(draft);
      dirty = false;
      saveBtn.disabled = true;
      toast(t('Сохранено'), 'ok');
    } catch (e) {
      err.append(callout(e.message, 'error'));
      errorToast(e);
    }
  }
  const rows = sec.fields.map((f) => {
    const locked = isLocked(f.path);
    const value = locked ? getPath(effective, f.path) : getPath(draft, f.path);
    const lock = locked ? t('Задано в окружении ({e}) — значение из файла не действует', { e: f.env || f.path }) : null;
    const hint = [f.hint ? t(f.hint) : null, f.env && !locked ? f.env : null].filter(Boolean).join(' · ');
    if (f.type === 'bool') {
      const c = h('div.list__cell',
        h('span.list__label', t(f.label), hint || lock ? h('span.list__value.list__value--muted', { style: { display: 'block', textAlign: 'left', maxWidth: 'none', whiteSpace: 'normal' } }, lock || hint) : null),
        switchEl(!!value, (v) => { setPath(draft, f.path, v); touch(); }, t(f.label), locked));
      return c;
    }
    let el;
    if (f.type === 'select') {
      el = select(f.options, value, { disabled: locked });
      el.addEventListener('change', () => { setPath(draft, f.path, el.value); touch(); });
    } else if (f.type === 'list') {
      el = input((value || []).join(', '), { placeholder: f.placeholder || '', disabled: locked });
      el.addEventListener('input', () => { setPath(draft, f.path, el.value.split(/[,;\s]+/).map((x) => x.trim()).filter(Boolean)); touch(); });
    } else if (f.type === 'secret') {
      el = input(value, { type: 'password', placeholder: t('не задан'), autocomplete: 'new-password', disabled: locked });
      el.addEventListener('input', () => { setPath(draft, f.path, el.value); touch(); });
    } else if (f.type === 'number') {
      const p = PRESETS[f.path] || {};
      el = numberInput(value, { min: f.min, max: f.max, presets: p.presets, unit: p.unit, zero: p.zero ? t(p.zero) : null, disabled: locked, label: t(f.label),
        onChange: (v) => { setPath(draft, f.path, v ?? 0); touch(); } });
    } else if (f.type === 'iface' || f.type === 'proxy') {
      el = combo(value, [], { placeholder: f.placeholder ? t(f.placeholder) : '', disabled: locked, label: t(f.label), onChange: (v) => { setPath(draft, f.path, v.trim()); touch(); } });
      netOptions().then((n) => el.setOptions(f.type === 'iface' ? ifaceOptions(n, false) : proxyOptions(n, false)));
    } else {
      el = input(value, { placeholder: f.placeholder || '', disabled: locked });
      el.addEventListener('input', () => { setPath(draft, f.path, el.value); touch(); });
    }
    const secretHint = f.type === 'secret' && value === mask ? t('Сохранён. Введите новый, чтобы заменить, или сотрите, чтобы удалить') : null;
    return field(t(f.label), el, { hint: [secretHint, hint].filter(Boolean).join(' · ') || null, lock });
  });
  root.append(hero(t(sec.title)), err, group(null, h('div.list', ...rows), sec.note ? t(sec.note) : null));
  if (sec.id === 'network') root.append(networkTools(save, () => dirty));
  ctx.onDestroy(() => {
    if (dirty) toast(t('Изменения не сохранены'), 'warn');
  });
}

/** Список интерфейсов для полей и проверка маршрутов: бот, каждый апстрим, Telegram. */
function networkTools(save, isDirty) {
  const ifaces = h('div');
  const out = h('div');
  netOptions().then((n) => {
    ifaces.append(group(t('Интерфейсы'), list(...n.interfaces.map((i) => cell({ label: i.name, value: [...i.ipv4, ...i.ipv6].join(', '), wrap: true })))));
  }).catch(errorToast);
  async function run() {
    if (isDirty()) await save();
    clear(out).append(h('div.summary__wait', h('span.ring'), t('Проверяю маршруты…')));
    try {
      const r = await api('/admin/api/network/check', { method: 'POST', body: {} });
      const rows = r.items.map((it) => {
        const p = it.probe;
        const ok = p.http_status != null;
        return cell({
          label: h('span.jobline',
            h('span.jobline__top', h('span.jobline__id', it.name), cstate(ok ? 'HTTP ' + p.http_status : t('недоступен'), ok ? 'run' : 'bad')),
            h('span.jobline__meta', h('span', p.url), h('span', p.route), p.exit_ip ? h('span', t('выход {ip}', { ip: p.exit_ip })) : null),
            h('span.jobline__meta', h('span', p.verdict))),
          wrap: true,
        });
      });
      clear(out).append(group(t('Результат проверки'), list(...rows), t('Запросы — GET без данных: ни модели, ни боту ничего не отправляется.')));
    } catch (e) {
      clear(out).append(callout(e.message, 'error'));
    }
  }
  return h('div',
    group(t('Проверка'), list(cell({ label: t('Проверить маршруты'), kind: 'tint', onClick: run })), t('DNS, соединение, HTTP и внешний IP каждого направления.')),
    out, ifaces);
}

export { SECTIONS, state };
