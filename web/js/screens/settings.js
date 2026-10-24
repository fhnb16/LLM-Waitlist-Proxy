import { api, desktop } from '../api.js';
import { t } from '../i18n.js';
import { h, clear, hero, group, list, cell, cstate, field, input, combo, numberInput, select, switchEl, icon, toast, errorToast, callout, clone, navBtn, getPath, setPath, state, btn, copy, fmt, segmented } from '../ui.js';
import { store, saveSettings, isLocked } from '../store.js';

const LEVELS = ['error', 'warn', 'info', 'debug', 'trace'].map((v) => ({ value: v, label: v }));

/** Разделы, собранные из полей: путь в settings.yaml, тип, подпись, подсказка, переменная окружения. */
const SECTIONS = [
  {
    id: 'server', tab: 'server', title: 'Сервер', icon: 'app',
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
    id: 'queue', tab: 'server', title: 'Очередь и лимиты', icon: 'queue',
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
    id: 'callback', tab: 'server', title: 'Доставка боту', icon: 'clients',
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
    id: 'storage', tab: 'log', title: 'Хранение и журнал', icon: 'logs',
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
    id: 'network', tab: 'net', title: 'Сеть и маршруты', icon: 'clients',
    note: 'Через какой интерфейс или прокси ходит каждое направление. Например: модели — через VPN (happ-tun или socks5-прокси VPN-клиента), бот — напрямую (Ethernet). Пусто — системный маршрут.',
    fields: [
      { path: 'network.upstream.interface', label: 'Модели: интерфейс', type: 'iface', placeholder: 'системный маршрут', hint: 'Имя (Ethernet, happ-tun, eth0) или IP. У апстрима можно задать свой.' },
      { path: 'network.upstream.proxy', label: 'Модели: прокси', type: 'proxy', placeholder: 'как в системе', hint: 'socks5://, socks5h://, http://. none — напрямую.' },
      { path: 'network.callback.interface', label: 'Бот: интерфейс', type: 'iface', placeholder: 'системный маршрут', hint: 'Колбэки на адрес бота.' },
      { path: 'network.callback.proxy', label: 'Бот: прокси', type: 'proxy', placeholder: 'как в системе' },
      { path: 'network.telegram.interface', label: 'Telegram: интерфейс', type: 'iface', placeholder: 'системный маршрут', hint: 'Протокол 1 и реле Bot API.' },
      { path: 'network.telegram.proxy', label: 'Telegram: прокси', type: 'proxy', placeholder: 'как в системе' },
    ],
  },
  {
    id: 'tg_relay', tab: 'telegram', title: 'Реле Telegram Bot API', icon: 'update',
    note: 'Замена воркера Cloudflare: бот ходит в Telegram через LMQ. В боте: «Сервер Bot API» — адрес ниже, «Адрес для скачивания файлов» — пусто. Токен бота в журнал не пишется.',
    fields: [
      { path: 'tg_relay.enabled', label: 'Включено', env: 'TG_RELAY_ENABLED', type: 'bool', hint: 'Адрес API принимает /bot<токен>/… и /file/bot<токен>/… и пересылает их в Telegram.' },
      { path: 'tg_relay.allowed_bots', label: 'Разрешённые боты', env: 'TG_RELAY_ALLOWED_BOTS', type: 'list', placeholder: '123456789', hint: 'id бота — число до двоеточия в токене. Пусто — реле обслужит любого бота, кто знает адрес.' },
      { path: 'tg_relay.interface', label: 'Интерфейс', type: 'iface', inherit: true, placeholder: 'как «Telegram» в «Сеть и маршруты»', hint: 'Через какую сеть идти к Telegram: VPN-интерфейс, Ethernet или IP.' },
      { path: 'tg_relay.proxy', label: 'Прокси', type: 'proxy', inherit: true, placeholder: 'как «Telegram» в «Сеть и маршруты»', hint: 'socks5://, socks5h://, http://. none — напрямую.' },
      { path: 'tg_relay.upstream', label: 'Адрес Bot API', placeholder: 'https://api.telegram.org', hint: 'Можно указать свой сервер Bot API или другое реле.' },
      { path: 'tg_relay.timeout', label: 'Ждать Telegram, с', type: 'number', min: 1, max: 600, hint: 'Зависший запрос лучше оборвать — бот повторит. getUpdates ждёт ещё и свой timeout.' },
      { path: 'tg_relay.max_body_mb', label: 'Предел загрузки, МБ', type: 'number', min: 1, max: 2000, hint: 'Файлы, которые бот отправляет в Telegram.' },
      { path: 'tg_relay.file_cache_mb', label: 'Кэш файлов, МБ', type: 'number', min: 0, max: 4096, hint: 'Скачанные ботом файлы хранятся в памяти час и повторно не идут через сеть.' },
    ],
  },
  {
    id: 'socks', tab: 'net', title: 'SOCKS5-прокси', icon: 'clients',
    note: 'LMQ сам становится прокси: программы, телефоны и Telegram подключаются к нему, а трафик уходит через выбранный интерфейс. Имена разрешает прокси (socks5h). Только TCP: UDP и звонки Telegram через него не идут.',
    fields: [
      { path: 'socks.enabled', label: 'Включён', env: 'SOCKS_ENABLED', type: 'bool', hint: 'Слушать порт и пропускать соединения через выбранный интерфейс.' },
      { path: 'socks.listen', label: 'Адрес и порт', env: 'SOCKS_LISTEN', placeholder: '127.0.0.1:1080', hint: '127.0.0.1 — только эта машина; 0.0.0.0 — локальная сеть и интернет (нужен проброс порта на роутере).' },
      { path: 'socks.interface', label: 'Выход через интерфейс', type: 'iface', placeholder: 'системный маршрут', hint: 'Куда отправлять трафик: VPN-интерфейс, Ethernet или IP.' },
      { path: 'socks.username', label: 'Логин', placeholder: 'без пароля', hint: 'Пусто — вход без пароля. Если прокси виден снаружи, задайте логин и пароль.' },
      { path: 'socks.password', label: 'Пароль', type: 'secret' },
      { path: 'socks.public_interface', label: 'Интерфейс без туннеля', type: 'iface', empty: 'авто: выход без туннеля', placeholder: 'авто: выход без туннеля', hint: 'Через него определяется внешний IP для ссылок — адрес, по которому прокси виден снаружи. Пусто — физический интерфейс с основным шлюзом, а не туннель VPN.' },
      { path: 'socks.public_host', label: 'Адрес для ссылок', placeholder: 'внешний IP определяется сам', hint: 'IP или домен вместо найденного внешнего IP (DDNS, другой адрес проброса).' },
    ],
  },
  {
    id: 'breaker', tab: 'server', title: 'Выключатель апстримов', icon: 'engines',
    note: 'Упавший апстрим временно исключается; фоновая проверка возвращает его, когда он ожил.',
    fields: [
      { path: 'breaker.enabled', label: 'Включён', type: 'bool' },
      { path: 'breaker.failure_threshold', label: 'Неудач подряд', type: 'number', min: 1, max: 100 },
      { path: 'breaker.open_seconds', label: 'Исключать на, с', type: 'number', min: 1 },
      { path: 'breaker.health_interval', label: 'Проверка раз в, с', type: 'number', min: 0, hint: 'GET {base}/models. 0 — не проверять.' },
    ],
  },
  {
    id: 'protocol1', tab: 'telegram', title: 'Протокол 1 (Telegram)', icon: 'update',
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
  'tg_relay.timeout': { presets: [10, 15, 30, 60, 120], unit: 's' },
  'tg_relay.max_body_mb': { presets: [10, 20, 50, 100] },
  'tg_relay.file_cache_mb': { presets: [0, 16, 32, 64, 128], zero: 'без кэша' },
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
    ...(n.interfaces || []).map((i) => ({ value: i.name, hint: [i.uplink ? t('выход без туннеля') : null, [...i.ipv4, ...i.ipv6].join(', ')].filter(Boolean).join(' · ') })),
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
    rememberTab(sub === 'schedule' || sub === 'profiles' ? 'server' : 'app');
    ctx.back('#/settings', t('Настройки'));
    const mod = await PAGES[sub]();
    return mod.render(ctx);
  }
  const section = SECTIONS.find((s) => s.id === sub);
  if (section) return renderSection(ctx, section);
  return renderIndex(ctx);
}

/** Вкладки индекса настроек — те же, что в нативном окне. */
const TABS = [
  { value: 'server', label: 'Сервер' },
  { value: 'net', label: 'Сеть' },
  { value: 'telegram', label: 'Telegram' },
  { value: 'log', label: 'Журнал' },
  { value: 'app', label: 'Приложение' },
];

function savedTab() {
  try {
    const v = localStorage.getItem('lmq.settingsTab');
    return TABS.some((x) => x.value === v) ? v : 'server';
  } catch (_) {
    return 'server';
  }
}

function rememberTab(v) {
  try {
    localStorage.setItem('lmq.settingsTab', v);
  } catch (_) { /* без запоминания */ }
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
  const body = h('div');
  const sections = (tab) => SECTIONS.filter((x) => x.tab === tab).map((x) => link(x.id, x.title, x.icon));
  const pages = {
    server: () => [
      group(t('Сервер очереди'), list(...sections('server'))),
      group(t('Работа'), list(
        link('schedule', 'Расписание', 'schedule', s.file.schedule.length ? String(s.file.schedule.length) : t('нет правил')),
        link('profiles', 'Профили, экспорт и импорт', 'profiles', s.profile),
      )),
    ],
    net: () => [group(t('Сеть'), list(...sections('net')))],
    telegram: () => [group('Telegram', list(...sections('telegram')))],
    log: () => [group(t('Журнал'), list(...sections('log')))],
    app: () => [group(t('Интерфейс'), list(
      link('appearance', 'Оформление', 'palette', ui.skin),
      desktop ? link('application', 'Приложение', 'app', ui.mode === 'native' ? t('нативное окно') : t('веб-окно')) : null,
      link('updates', 'Обновления', 'update'),
      link('about', 'О программе', 'info'),
    ))],
  };
  const show = (tab) => {
    rememberTab(tab);
    clear(body).append(...pages[tab]());
  };
  const tab = savedTab();
  root.append(
    hero(t('Настройки'), t('Профиль {p} · {f}', { p: s.profile, f: s.paths.settings_file })),
    h('div.toolbar.toolbar--tight', segmented(TABS.map((x) => ({ value: x.value, label: t(x.label) })), tab, show)),
    body,
    s.locked.length ? group(t('Задано в окружении'), list(cell({ label: s.locked.join(', '), wrap: true })), t('Эти значения берутся из переменных окружения и перекрывают файл.')) : null,
  );
  show(tab);
}

function renderSection(ctx, sec) {
  // «Назад» вернёт на вкладку этого раздела, даже если он открыт ссылкой с обзора.
  rememberTab(sec.tab);
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
      netOptions().then((n) => {
        const opts = f.type === 'iface' ? ifaceOptions(n, !!f.inherit) : proxyOptions(n, !!f.inherit);
        // Своя подпись пустого значения (например, «авто» вместо системного маршрута).
        if (f.empty) opts[0] = { value: '', label: t(f.empty) };
        el.setOptions(opts);
      });
    } else {
      el = input(value, { placeholder: f.placeholder || '', disabled: locked });
      el.addEventListener('input', () => { setPath(draft, f.path, el.value); touch(); });
    }
    const secretHint = f.type === 'secret' && value === mask ? t('Сохранён. Введите новый, чтобы заменить, или сотрите, чтобы удалить') : null;
    return field(t(f.label), el, { hint: [secretHint, hint].filter(Boolean).join(' · ') || null, lock });
  });
  root.append(hero(t(sec.title)), err, group(null, h('div.list', ...rows), sec.note ? t(sec.note) : null));
  if (sec.id === 'network') root.append(networkTools(save, () => dirty));
  if (sec.id === 'tg_relay') root.append(relayTools(ctx, save, () => dirty));
  if (sec.id === 'socks') root.append(socksTools(ctx, save, () => dirty));
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

/** Счётчики реле с запуска — строки списка (общие для раздела настроек и обзора). */
export function relayStatRows(s) {
  const failed = s.relay_errors + s.rejected;
  return [
    cell({ label: t('Запросов'), value: fmt.num(s.requests), extra: s.requests ? cstate(t('ошибок Telegram: {n}', { n: s.tg_errors }), s.tg_errors ? 'busy' : 'run') : null }),
    cell({ label: t('Сбоев реле и отказов'), value: `${fmt.num(s.relay_errors)} · ${fmt.num(s.rejected)}`, extra: failed ? cstate(t('есть'), 'bad') : null }),
    cell({ label: t('Средний ответ Telegram'), value: fmt.ms(s.avg_ms) }),
    cell({ label: t('Трафик'), value: `↑ ${fmt.bytes(s.bytes_up)} · ↓ ${fmt.bytes(s.bytes_down)}` }),
    cell({ label: t('Файлов'), value: t('{n}, из кэша {c}', { n: fmt.num(s.files), c: fmt.num(s.cache_hits) }) }),
    s.last_at ? cell({ label: t('Последний запрос'), value: fmt.ago(s.last_at) }) : null,
    Object.keys(s.methods || {}).length
      ? cell({ label: t('Методы'), value: Object.entries(s.methods).sort((a, b) => b[1] - a[1]).slice(0, 8).map(([m, n]) => `${m} ${n}`).join(' · '), wrap: true })
      : null,
    s.last_error ? cell({ label: t('Последняя ошибка'), value: `${s.last_error} · ${fmt.ago(s.last_error_at)}`, wrap: true }) : null,
  ].filter(Boolean);
}

/** Адрес для бота, счётчики и проверка всей цепочки: маршрут → Bot API → адрес API LMQ. */
function relayTools(ctx, save, isDirty) {
  const info = h('div');
  const out = h('div');
  async function load() {
    try {
      const r = await api('/admin/api/tg-relay');
      if (!ctx.alive()) return;
      const url = r.url || t('API не слушает');
      clear(info).append(
        group(t('Для бота'), list(
          cell({ label: t('Сервер Bot API'), value: url, extra: r.url ? btn(t('Копировать'), { inline: true, onClick: () => copy(r.url) }) : null, wrap: true }),
          cell({ label: t('Маршрут'), value: r.route, wrap: true }),
          cell({ label: t('Пересылает в'), value: r.upstream, wrap: true }),
        ), r.enabled ? t('Если бот на другой машине, укажите адрес, по которому она видит LMQ.') : t('Реле выключено: включите его и сохраните.'), { noteWarn: !r.enabled }),
        group(t('С запуска'), list(...relayStatRows(r.stats))),
      );
    } catch (e) {
      clear(info).append(callout(e.message, 'error'));
    }
  }
  const token = input('', { type: 'password', placeholder: t('необязательно'), autocomplete: 'off' });
  async function run() {
    if (isDirty()) await save();
    clear(out).append(h('div.summary__wait', h('span.ring'), t('Проверяю реле…')));
    try {
      const r = await api('/admin/api/tg-relay/check', { method: 'POST', body: { token: token.value.trim() || null } });
      const rows = r.steps.map((s) => cell({
        label: h('span.jobline',
          h('span.jobline__top', h('span.jobline__id', t(s.title)), cstate(s.ok ? t('работает') : t('не работает'), s.ok ? 'run' : 'bad')),
          h('span.jobline__meta', h('span', s.text))),
        wrap: true,
      }));
      const meta = [r.route, r.exit_ip ? t('выход {ip}', { ip: r.exit_ip }) : null, r.bot].filter(Boolean).join(' · ');
      clear(out).append(group(r.ok ? t('Реле работает') : t('Реле не работает'), list(...rows), meta));
      load();
    } catch (e) {
      clear(out).append(callout(e.message, 'error'));
    }
  }
  load();
  return h('div',
    info,
    group(t('Проверка'), h('div.list',
      field(t('Токен бота'), token, { hint: t('Для getMe через всю цепочку. Не сохраняется. Без него проверка идёт с пробным токеном.') }),
      cell({ label: t('Проверить реле'), kind: 'tint', onClick: run })),
    t('Маршрут до Telegram (DNS, соединение, внешний IP), ответ Bot API по нему и путь через адрес API LMQ.')),
    out);
}

/** Счётчики SOCKS5-прокси с запуска (общие для раздела настроек и обзора). */
export function socksStatRows(s) {
  return [
    s.listen_error ? cell({ label: t('Не слушает'), value: s.listen_error, wrap: true, extra: cstate(t('ошибка'), 'bad') }) : null,
    cell({ label: t('Соединений'), value: fmt.num(s.connections), extra: cstate(t('открыто: {n}', { n: s.active }), s.active ? 'run' : 'off') }),
    cell({ label: t('Не удалось подключиться'), value: fmt.num(s.failed) }),
    cell({ label: t('Отказано'), value: fmt.num(s.rejected), extra: s.rejected ? cstate(t('есть'), 'busy') : null }),
    cell({ label: t('Трафик'), value: `↑ ${fmt.bytes(s.bytes_up)} · ↓ ${fmt.bytes(s.bytes_down)}` }),
    s.last_error ? cell({ label: t('Последняя ошибка'), value: `${s.last_error} · ${fmt.ago(s.last_error_at)}`, wrap: true }) : null,
  ].filter(Boolean);
}

/** Готовые ссылки на прокси с копированием, счётчики и проверка выхода в интернет. */
function socksTools(ctx, save, isDirty) {
  const info = h('div');
  const out = h('div');
  async function load(refresh) {
    try {
      const r = await api('/admin/api/socks' + (refresh ? '?refresh=1' : ''));
      if (!ctx.alive()) return;
      const rows = r.links.map((l) => cell({
        label: h('span.jobline', h('span.jobline__top', h('span.jobline__id', t(l.title))), h('span.jobline__meta', h('span', `${l.host}:${l.port}`))),
        extra: h('span', { style: { display: 'inline-flex', gap: '6px', flexWrap: 'wrap', justifyContent: 'flex-end' } },
          btn('socks5h', { inline: true, onClick: () => copy(l.url) }),
          btn('Telegram', { inline: true, onClick: () => copy(l.telegram) })),
      }));
      rows.push(cell({ label: t('Обновить внешний IP'), kind: 'tint', onClick: () => load(true) }));
      const notes = [
        r.public_ip ? t('Внешний IP {ip} ({route}).', { ip: r.public_ip, route: r.public_route }) : null,
        r.public_error,
        r.local_only ? t('Прокси слушает только эту машину: для ссылок «Снаружи» и «В локальной сети» укажите адрес 0.0.0.0:порт.') : null,
        t('Ссылки содержат логин и пароль. Снаружи прокси доступен, если на роутере проброшен порт.'),
      ].filter(Boolean).join(' ');
      clear(info).append(
        group(t('Ссылки'), list(...rows), notes, { noteWarn: r.local_only || !!r.public_error }),
        group(t('С запуска'), list(cell({ label: t('Выход через'), value: r.route, wrap: true }), ...socksStatRows(r.stats))),
      );
    } catch (e) {
      clear(info).append(callout(e.message, 'error'));
    }
  }
  async function run() {
    if (isDirty()) await save();
    clear(out).append(h('div.summary__wait', h('span.ring'), t('Проверяю прокси…')));
    try {
      const r = await api('/admin/api/socks/check', { method: 'POST', body: {} });
      const rows = r.steps.map((s) => cell({
        label: h('span.jobline',
          h('span.jobline__top', h('span.jobline__id', t(s.title)), cstate(s.ok ? t('работает') : t('не работает'), s.ok ? 'run' : 'bad')),
          h('span.jobline__meta', h('span', s.text))),
        wrap: true,
      }));
      clear(out).append(group(r.ok ? t('Прокси работает') : t('Прокси не работает'), list(...rows), r.exit_ip ? t('выход {ip}', { ip: r.exit_ip }) : null));
      load(false);
    } catch (e) {
      clear(out).append(callout(e.message, 'error'));
    }
  }
  load(false);
  return h('div',
    info,
    group(t('Проверка'), list(cell({ label: t('Проверить прокси'), kind: 'tint', onClick: run })),
      t('Соединение через прокси и внешний IP — в сравнении с выбранным интерфейсом напрямую и с системным маршрутом.')),
    out);
}

export { SECTIONS, state };
