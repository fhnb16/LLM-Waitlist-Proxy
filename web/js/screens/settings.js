import { desktop } from '../api.js';
import { t } from '../i18n.js';
import { h, clear, hero, group, list, cell, field, input, select, switchEl, icon, toast, errorToast, callout, clone, navBtn, getPath, setPath, state } from '../ui.js';
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
      { path: 'metrics.enabled', label: 'Метрики Prometheus', type: 'bool', hint: 'GET /metrics на адресе API.' },
      { path: 'metrics.token', label: 'Токен метрик', env: 'METRICS_TOKEN', type: 'secret', hint: 'Пусто — /metrics только с localhost.' },
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
      el = input(value, { type: 'number', min: f.min, max: f.max, inputmode: 'numeric', disabled: locked });
      el.addEventListener('input', () => { setPath(draft, f.path, el.value === '' ? 0 : Number(el.value)); touch(); });
    } else {
      el = input(value, { placeholder: f.placeholder || '', disabled: locked });
      el.addEventListener('input', () => { setPath(draft, f.path, el.value); touch(); });
    }
    const secretHint = f.type === 'secret' && value === mask ? t('Сохранён. Введите новый, чтобы заменить, или сотрите, чтобы удалить') : null;
    return field(t(f.label), el, { hint: [secretHint, hint].filter(Boolean).join(' · ') || null, lock });
  });
  root.append(hero(t(sec.title)), err, group(null, h('div.list', ...rows), sec.note ? t(sec.note) : null));
  ctx.onDestroy(() => {
    if (dirty) toast(t('Изменения не сохранены'), 'warn');
  });
}

export { SECTIONS, state };
