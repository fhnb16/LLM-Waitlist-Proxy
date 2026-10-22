import { api } from '../api.js';
import { t } from '../i18n.js';
import { lineChart, barChart, sparkline } from '../charts.js';
import { h, clear, hero, group, list, cell, field, input, select, textarea, cellSwitch, pick, btn, cstate, fmt, modal, toast, errorToast, confirmDialog, state, callout, clone, navBtn } from '../ui.js';
import { store, on, saveSettings, isLocked } from '../store.js';

const BREAKER = { closed: ['в работе', 'run'], open: ['исключён', 'bad'], half_open: ['проверка', 'busy'] };

export async function render(ctx) {
  if (ctx.parts[1]) return renderModel(ctx, ctx.parts[1]);
  ctx.title(t('Модели'));
  const root = ctx.root;
  const locked = isLocked('models');
  ctx.actions(locked ? null : navBtn(t('Добавить'), () => addModel(ctx), 'plus'));
  const box = h('div');
  root.append(hero(t('Модели'), t('Таблица маршрутизации: модель → апстримы со слотами.')), box);
  if (locked) root.prepend(h('div.banner.banner--warn', h('span.banner__title', t('Модели заданы в MODELS_CONFIG')), t('Таблица читается из внешнего файла; правки в панели не действуют.')));
  let sparks = {};
  try {
    const s = await api('/admin/api/stats?range=1h&group=model');
    for (const g of s.groups) sparks[g.key] = g.points.map((p) => p.jobs);
  } catch (_) { /* без спарклайнов */ }

  const paint = () => {
    const file = store.settings.file;
    const ov = store.overview;
    const names = Object.keys(file.models);
    clear(box);
    if (!names.length) {
      box.append(state(t('Моделей нет'), t('Добавьте модель: имя, которое бот укажет в поле model, и адрес OpenAI-совместимого сервера.'), btn(t('Добавить модель'), { kind: 'filled', inline: true, onClick: () => addModel(ctx) })));
      return;
    }
    box.append(list(...names.map((name) => {
      const m = file.models[name];
      const rt = ov && ov.models.find((x) => x.name === name);
      const status = !m.enabled ? cstate(t('выключена'), 'off') : rt && !rt.available ? cstate(t('недоступна'), 'bad') : rt && rt.running ? cstate(t('работает'), 'run') : cstate(t('ждёт'), '');
      const aliases = Object.entries(file.aliases || {}).filter(([, v]) => v === name).map(([k]) => k);
      return cell({
        label: h('span.jobline',
          h('span.jobline__top', h('strong', name), status),
          h('span.jobline__meta',
            h('span', t('апстримов: {n}', { n: m.upstreams.length })),
            rt ? h('span', t('слотов: {n}', { n: rt.slots })) : null,
            rt ? h('span', t('очередь: {n}', { n: rt.queued })) : null,
            aliases.length ? h('span', t('алиасы: {a}', { a: aliases.join(', ') })) : null,
            m.fallback.length ? h('span', t('резерв: {a}', { a: m.fallback.join(' → ') })) : null),
          m.description ? h('span.jobline__meta', m.description) : null),
        extra: sparks[name] ? sparkline(sparks[name]) : null,
        onClick: () => ctx.go('#/models/' + encodeURIComponent(name)),
      });
    })));
    box.append(h('p.footnote', t('Бот получает этот список через GET /v1/models. Алиасы видны боту как отдельные модели.')));
  };
  paint();
  ctx.onDestroy(on('tick', paint));
  ctx.onDestroy(on('settings', paint));
  if (ctx.params.get('new')) addModel(ctx);
}

function addModel(ctx) {
  const name = input('', { placeholder: 'qwen3-27b' });
  const url = input('http://127.0.0.1:8080/v1/chat/completions', { placeholder: 'http://127.0.0.1:8080/v1/chat/completions', inputmode: 'url' });
  const slots = input('1', { type: 'number', min: '1', max: '64' });
  const engineOpts = [{ value: '', label: t('нет — адрес вручную') }, ...(store.settings.file.engines || []).map((e) => ({ value: e.id, label: e.name || e.id }))];
  const engine = select(engineOpts, '');
  modal({
    title: t('Новая модель'),
    sub: t('Имя — то, что бот укажет в поле model.'),
    body: h('div.list',
      field(t('Имя'), name),
      field(t('Движок'), engine, { hint: t('Локальный движок подставит адрес и слоты сам.') }),
      field(t('Адрес'), url, { hint: t('…/v1/chat/completions OpenAI-совместимого сервера') }),
      field(t('Слоты'), slots, { hint: t('Одновременных запросов (--parallel у llama.cpp)') })),
    actions: [
      { label: t('Отмена') },
      {
        label: t('Создать'),
        kind: 'filled',
        onClick: async () => {
          const n = name.value.trim();
          if (!n) throw new Error(t('Введите имя модели'));
          const file = clone(store.settings.file);
          if (file.models[n]) throw new Error(t('Модель {n} уже есть', { n }));
          file.models[n] = {
            upstreams: [{ name: '', url: engine.value ? '' : url.value.trim(), model: '', api_key: '', slots: Number(slots.value) || 1, engine: engine.value, stream: false, timeout: 0, interface: '', proxy: '', drop_params: [] }],
            defaults: {}, force: {}, fallback: [], max_queue: null, enabled: true, description: '',
          };
          await saveSettings(file);
          toast(t('Модель {n} создана', { n }), 'ok');
          ctx.go('#/models/' + encodeURIComponent(n));
        },
      },
    ],
  });
}

function jsonArea(value, onValid) {
  const ta = textarea(JSON.stringify(value || {}, null, 2), { rows: 4 });
  const err = h('span.field__hint', { style: { color: 'var(--danger)', gridColumn: '1' } });
  ta.addEventListener('input', () => {
    try {
      const v = ta.value.trim() ? JSON.parse(ta.value) : {};
      if (typeof v !== 'object' || Array.isArray(v) || v === null) throw new Error(t('нужен объект { … }'));
      ta.classList.remove('code--invalid');
      err.textContent = '';
      onValid(v);
    } catch (e) {
      ta.classList.add('code--invalid');
      err.textContent = e.message;
      onValid(undefined);
    }
  });
  return [ta, err];
}

async function renderModel(ctx, name) {
  ctx.back('#/models', t('Модели'));
  ctx.title(name);
  const root = ctx.root;
  if (!store.settings.file.models[name]) {
    root.append(state(t('Модель не найдена'), name));
    return;
  }
  const draft = clone(store.settings.file);
  const m = draft.models[name];
  const mask = store.settings.mask;
  const locked = isLocked('models');
  let invalid = new Set();
  let dirty = false;
  const saveBtn = navBtn(t('Сохранить'), save);
  const touch = () => {
    dirty = true;
    saveBtn.disabled = invalid.size > 0;
  };
  ctx.actions(locked ? null : saveBtn);

  async function save() {
    if (invalid.size) {
      toast(t('Исправьте JSON в параметрах'), 'error');
      return;
    }
    try {
      await saveSettings(draft);
      dirty = false;
      toast(t('Сохранено'), 'ok');
      paintRuntime();
    } catch (e) {
      errorToast(e);
    }
  }

  const others = Object.keys(draft.models).filter((x) => x !== name);
  const aliasInput = input(Object.entries(draft.aliases || {}).filter(([, v]) => v === name).map(([k]) => k).join(', '), { placeholder: 'fast, small' });
  aliasInput.addEventListener('input', () => {
    for (const [k, v] of Object.entries(draft.aliases || {})) if (v === name) delete draft.aliases[k];
    for (const a of aliasInput.value.split(',').map((x) => x.trim()).filter(Boolean)) draft.aliases[a] = name;
    touch();
  });
  const desc = input(m.description, { placeholder: t('например: основная, 27B Q4') });
  desc.addEventListener('input', () => { m.description = desc.value; touch(); });
  const maxQ = input(m.max_queue ?? '', { type: 'number', min: '1', placeholder: t('без предела') });
  maxQ.addEventListener('input', () => { m.max_queue = maxQ.value === '' ? null : Number(maxQ.value); touch(); });

  const runtime = h('div');
  const charts = h('div.plots');
  const ups = h('div');
  const [defArea, defErr] = jsonArea(m.defaults, (v) => { if (v) { m.defaults = v; invalid.delete('d'); } else invalid.add('d'); touch(); });
  const [forceArea, forceErr] = jsonArea(m.force, (v) => { if (v) { m.force = v; invalid.delete('f'); } else invalid.add('f'); touch(); });
  const testOut = h('div');
  const ifaceList = h('datalist', { id: 'lmq-ifaces' }, h('option', { value: 'system' }));
  api('/admin/api/network').then((n) => ifaceList.append(...n.interfaces.map((i) => h('option', { value: i.name }, [...i.ipv4, ...i.ipv6].join(', '))))).catch(() => {});

  root.append(
    ifaceList,
    hero(name, m.description || t('Модель')),
    runtime,
    group(t('Основное'), list(
      field(t('Описание'), desc),
      cellSwitch(t('Включена'), m.enabled, (v) => { m.enabled = v; touch(); }, { sub: t('Выключенная модель пропадает из /v1/models') }),
      cellSwitch(t('Понимает картинки'), !!m.vision, (v) => { m.vision = v; touch(); }, { sub: t('В /v1/models появится признак vision — бот будет отправлять фото') }),
      field(t('Предел очереди'), maxQ, { hint: t('Сверх — резервная модель или 429') }),
      field(t('Алиасы'), aliasInput, { hint: t('Через запятую. Бот может указать любой из них') }),
    )),
    group(t('Резервные модели'), others.length ? pick(others, m.fallback, (v) => { m.fallback = v; touch(); }) : h('p.group__note', t('Других моделей нет.')),
      t('При ошибке, переполнении или исключённых апстримах задание переходит на следующую модель по порядку выбора. В колбэке приходит фактическая модель.')),
    ups,
    group(t('Параметры по умолчанию'), h('div.list', h('div.field.field--stack', h('span.field__label', 'defaults'), defArea, defErr)), t('Подставляются, если бот их не прислал. Например: {"max_tokens": 400}')),
    group(t('Принудительные параметры'), h('div.list', h('div.field.field--stack', h('span.field__label', 'force'), forceArea, forceErr)), t('Перекрывают присланное. Например: {"chat_template_kwargs": {"enable_thinking": false}}')),
    group(t('Проверка'), list(cell({ label: t('Отправить тестовый запрос'), kind: 'tint', onClick: test })), t('Короткий синхронный запрос «Reply with: OK» с высшим приоритетом.')),
    testOut,
    h('h2.group__title', t('Статистика за час')),
    charts,
    locked ? null : group(null, list(cell({ label: t('Удалить модель'), kind: 'danger', onClick: remove }))),
  );

  /** Параметры, которые апстрим не принимает: через запятую. */
  function dropField(u) {
    const el = input((u.drop_params || []).join(', '), { placeholder: 'mirostat, chat_template_kwargs' });
    el.addEventListener('input', () => { u.drop_params = el.value.split(/[,;\s]+/).map((x) => x.trim()).filter(Boolean); touch(); });
    return field(t('Не отправлять параметры'), el, { hint: t('Через запятую. Отклонённые апстримом параметры LMQ убирает и сам') });
  }

  function paintUpstreams() {
    clear(ups);
    const engines = [{ value: '', label: t('нет') }, ...(draft.engines || []).map((e) => ({ value: e.id, label: e.name || e.id }))];
    m.upstreams.forEach((u, i) => {
      const f = (label, key, attrs = {}, hint) => {
        const el = input(u[key], attrs);
        el.addEventListener('input', () => { u[key] = attrs.type === 'number' ? Number(el.value) || 0 : el.value; touch(); });
        return field(label, el, { hint });
      };
      const eng = select(engines, u.engine);
      eng.addEventListener('change', () => { u.engine = eng.value; touch(); paintUpstreams(); });
      ups.append(group(t('Апстрим {n}', { n: i + 1 }) + (u.name ? ' · ' + u.name : ''), list(
        f(t('Подпись'), 'name', { placeholder: `${name}#${i}` }),
        field(t('Движок'), eng, { hint: u.engine ? t('Адрес и слоты берутся из движка, поля ниже можно оставить пустыми') : null }),
        f(t('Адрес'), 'url', { placeholder: u.engine ? t('из движка') : 'http://127.0.0.1:8080/v1/chat/completions', inputmode: 'url' }),
        f(t('Имя у апстрима'), 'model', { placeholder: name }, t('Что ждёт сервер модели. Пусто — имя модели')),
        f(t('Ключ'), 'api_key', { type: 'password', placeholder: t('не нужен'), autocomplete: 'new-password' }, u.api_key === mask ? t('Сохранён. Введите новый, чтобы заменить') : null),
        f(t('Слоты'), 'slots', { type: 'number', min: '1', max: '256' }, u.engine ? t('1 — взять --parallel из движка') : t('Одновременных запросов')),
        f(t('Таймаут, с'), 'timeout', { type: 'number', min: '0' }, t('0 — UPSTREAM_TIMEOUT')),
        cellSwitch(t('Стрим к апстриму'), u.stream, (v) => { u.stream = v; touch(); }, { sub: t('Ответ собирается внутри; боту всё равно уходит целиком') }),
        f(t('Интерфейс'), 'interface', { placeholder: t('как в «Сеть и маршруты»'), list: 'lmq-ifaces' }, t('Ethernet, happ-tun, IP или system')),
        f(t('Прокси'), 'proxy', { placeholder: t('как в «Сеть и маршруты»') }, t('socks5://127.0.0.1:10808, http://… или none')),
        dropField(u),
        m.upstreams.length > 1 ? cell({ label: t('Удалить апстрим'), kind: 'danger', onClick: () => { m.upstreams.splice(i, 1); touch(); paintUpstreams(); } }) : null,
      )));
    });
    ups.append(group(null, list(cell({ label: t('Добавить апстрим'), kind: 'tint', onClick: () => { m.upstreams.push({ name: '', url: '', model: '', api_key: '', slots: 1, engine: '', stream: false, timeout: 0, interface: '', proxy: '', drop_params: [] }); touch(); paintUpstreams(); } })),
      t('При ошибке соединения или 5xx задание переходит к следующему апстриму; 4xx — ошибка задания.')));
  }

  function paintRuntime() {
    const rt = store.overview && store.overview.models.find((x) => x.name === name);
    clear(runtime);
    if (!rt) return;
    runtime.append(group(t('Сейчас'), list(
      cell({ label: t('Очередь'), value: String(rt.queued) }),
      cell({ label: t('Выполняется'), value: `${rt.running} / ${rt.slots}` }),
      ...rt.upstreams.map((u) => {
        const [label, kind] = BREAKER[u.health.breaker] || [u.health.breaker, ''];
        const extra = h('span', { style: { display: 'inline-flex', gap: '8px', alignItems: 'center' } },
          cstate(t(label), u.usable ? kind : 'bad'),
          u.health.breaker !== 'closed' ? btn(t('Сбросить'), { inline: true, onClick: async () => {
            try {
              await api(`/admin/api/upstreams/${encodeURIComponent(u.id)}/reset`, { method: 'POST' });
              toast(t('Выключатель сброшен'), 'ok');
            } catch (e) { errorToast(e); }
          } }) : null);
        const sub = [
          `${u.busy}/${u.slots} ${t('слотов')}`,
          u.health.check_latency_ms != null ? t('проверка {v}', { v: fmt.ms(u.health.check_latency_ms) }) : null,
          u.engine ? t('движок {e}: {s}', { e: u.engine, s: u.engine_status || '—' }) : null,
          u.health.last_error ? u.health.last_error : null,
        ].filter(Boolean).join(' · ');
        return cell({ label: h('span.jobline', h('span.jobline__top', h('span.jobline__id', u.id)), h('span.jobline__meta', u.url), h('span.jobline__meta', sub)), extra });
      }),
    )));
  }

  async function paintCharts() {
    try {
      const s = await api('/admin/api/stats?range=1h&model=' + encodeURIComponent(name));
      if (!ctx.alive()) return;
      clear(charts);
      const a = h('div');
      const b = h('div');
      charts.append(a, b);
      lineChart(a, { title: t('Время ответа'), points: s.points, series: [{ key: 'lat_p50', label: 'p50', color: 'var(--tint)' }, { key: 'lat_p95', label: 'p95', color: 'var(--warn)', dash: true }], yFormat: fmt.ms });
      barChart(b, { title: t('Задания'), points: s.points, series: [{ key: 'ok', label: t('успешно'), color: 'var(--ok)' }, { key: 'errors', label: t('ошибки'), color: 'var(--danger)' }] });
    } catch (_) { /* без графиков */ }
  }

  async function test() {
    clear(testOut).append(h('div.summary__wait', h('span.ring'), t('Ждём ответ…')));
    if (dirty) await save();
    try {
      const r = await api(`/admin/api/models/${encodeURIComponent(name)}/test`, { method: 'POST', body: {} });
      clear(testOut).append(r.ok
        ? callout(t('Ответ за {v}: «{text}»', { v: fmt.ms(r.latency_ms), text: (r.text || '').slice(0, 200) }), 'ok')
        : callout(t('Ошибка ({s}): {e}', { s: r.status || '—', e: r.error }), 'error'));
    } catch (e) {
      clear(testOut).append(callout(e.message, 'error'));
    }
  }

  async function remove() {
    if (!(await confirmDialog(t('Удалить модель {n}?', { n: name }), t('Задания этой модели в очереди завершатся ошибкой. Ссылки на неё из резервов и алиасов будут убраны.'), { ok: t('Удалить'), danger: true }))) return;
    const file = clone(store.settings.file);
    delete file.models[name];
    for (const [k, v] of Object.entries(file.aliases || {})) if (v === name) delete file.aliases[k];
    for (const mm of Object.values(file.models)) mm.fallback = mm.fallback.filter((f) => f !== name);
    try {
      await saveSettings(file);
      toast(t('Модель удалена'), 'ok');
      ctx.go('#/models');
    } catch (e) {
      errorToast(e);
    }
  }

  paintUpstreams();
  paintRuntime();
  paintCharts();
  ctx.onDestroy(on('tick', paintRuntime));
  if (locked) {
    for (const el of root.querySelectorAll('input, select, textarea, .switch__input')) el.disabled = true;
    root.prepend(callout(t('Таблица моделей задана в MODELS_CONFIG — здесь только просмотр.'), 'warn'));
  }
}
