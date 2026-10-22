import { api } from '../api.js';
import { t } from '../i18n.js';
import { h, clear, hero, group, list, cell, segmented, cstate, fmt, btn, toast, errorToast, confirmDialog, jobState, state, loader, debounce, callout } from '../ui.js';
import { on } from '../store.js';

const FILTERS = [
  { value: 'active', label: 'Активные' },
  { value: 'done', label: 'Готово' },
  { value: 'failed', label: 'Ошибки' },
  { value: 'dead', label: 'Не доставлено' },
  { value: '', label: 'Все' },
];

export async function render(ctx) {
  if (ctx.parts[1]) return renderJob(ctx, ctx.parts[1]);
  ctx.title(t('Очередь'));
  const root = ctx.root;
  let filter = ctx.params.get('state') ?? 'active';
  let query = '';
  let offset = 0;
  const PAGE = 50;

  const counts = h('p.hero__sub');
  const seg = segmented(FILTERS.map((f) => ({ value: f.value, label: t(f.label) })), filter, (v) => {
    filter = v;
    offset = 0;
    load();
  });
  const search = h('input.search__input', { type: 'search', placeholder: t('job_id, chat_id или клиент'), 'aria-label': t('Поиск') });
  search.addEventListener('input', debounce(() => {
    query = search.value;
    offset = 0;
    load();
  }, 250));
  const box = h('div');
  const bulk = h('div.actions');
  root.append(
    h('header.hero', h('h1.hero__title', t('Очередь')), counts),
    h('div.search', h('div.search__box', h('span', { 'aria-hidden': 'true' }, '⌕'), search)),
    h('div.toolbar.toolbar--tight', seg),
    box,
    bulk,
  );

  async function load() {
    if (!box.firstChild) box.append(loader());
    let data;
    try {
      const qs = new URLSearchParams({ limit: String(PAGE), offset: String(offset) });
      if (filter) qs.set('state', filter);
      if (query.trim()) qs.set('q', query.trim());
      data = await api('/admin/api/jobs?' + qs);
    } catch (e) {
      clear(box).append(callout(e.message, 'error'));
      return;
    }
    if (!ctx.alive()) return;
    const c = data.counts || {};
    counts.textContent = [
      t('в очереди: {n}', { n: c.queued || 0 }),
      t('выполняется: {n}', { n: c.running || 0 }),
      t('доставляется: {n}', { n: c.pending_delivery || 0 }),
      t('не доставлено: {n}', { n: c.dead || 0 }),
    ].join(' · ');
    clear(box);
    if (!data.items.length) {
      box.append(state(t('Пусто'), filter === 'active' ? t('Сейчас нет заданий в очереди.') : t('Нет заданий с таким фильтром.')));
    } else {
      box.append(list(...data.items.map((j) => jobCell(ctx, j))));
      const pager = h('div.btn-row', { style: { marginTop: '12px' } });
      if (offset > 0) pager.append(btn(t('Назад'), { inline: true, onClick: () => { offset = Math.max(0, offset - PAGE); load(); } }));
      if (offset + PAGE < data.total) pager.append(btn(t('Дальше'), { inline: true, onClick: () => { offset += PAGE; load(); } }));
      if (pager.childNodes.length) box.append(pager);
      box.append(h('p.footnote', t('Показано {a}–{b} из {n}', { a: offset + 1, b: offset + data.items.length, n: data.total })));
    }
    clear(bulk);
    if ((c.dead || 0) > 0) {
      bulk.append(btn(t('Повторить доставку всех недоставленных ({n})', { n: c.dead }), {
        onClick: async () => {
          try {
            const r = await api('/admin/api/jobs/redeliver-dead', { method: 'POST' });
            toast(t('Повторная доставка: {n}', { n: r.count }), 'ok');
            load();
          } catch (e) {
            errorToast(e);
          }
        },
      }));
    }
    bulk.append(btn(t('Очистить завершённые'), {
      kind: 'ghost',
      onClick: async () => {
        if (!(await confirmDialog(t('Очистить завершённые?'), t('Удалятся доставленные и недоставленные задания вместе с их историей. Статистика останется.'), { ok: t('Очистить'), danger: true }))) return;
        try {
          const r = await api('/admin/api/jobs/purge', { method: 'POST', body: { states: ['delivered', 'dead'] } });
          toast(t('Удалено: {n}', { n: r.removed }), 'ok');
          load();
        } catch (e) {
          errorToast(e);
        }
      },
    }));
  }

  await load();
  const refresh = debounce(load, 800);
  ctx.onDestroy(on('job', refresh));
}

function jobCell(ctx, j) {
  const st = jobState(j.public_state);
  const delivery = j.delivery === 'pending' && j.state !== 'queued' && j.state !== 'running' ? jobState('pending_delivery') : null;
  return cell({
    label: h(
      'span.jobline',
      h('span.jobline__top', h('span.jobline__id', j.job_id), cstate(st.label, st.kind), delivery ? cstate(delivery.label, delivery.kind) : null),
      h(
        'span.jobline__meta',
        h('span', j.model_used !== j.model ? `${j.model} → ${j.model_used}` : j.model),
        h('span', j.chat_id ? 'chat ' + j.chat_id : t('синхронный запрос')),
        h('span', j.client),
        j.position ? h('span', t('позиция {n}', { n: j.position })) : null,
        j.queued_ms != null ? h('span', t('ожидание {v}', { v: fmt.ms(j.queued_ms) })) : null,
        j.latency_ms != null ? h('span', t('ответ {v}', { v: fmt.ms(j.latency_ms) })) : null,
        h('span', fmt.ago(j.created_at)),
      ),
      j.error_code || j.delivery_error ? h('span.jobline__err', [j.error_code, j.delivery_error].filter(Boolean).join(' · ')) : null,
    ),
    onClick: () => ctx.go('#/queue/' + encodeURIComponent(j.job_id)),
  });
}

async function renderJob(ctx, id) {
  ctx.title(t('Задание'));
  ctx.back('#/queue', t('Очередь'));
  const root = ctx.root;
  const box = h('div', loader());
  root.append(box);
  async function load() {
    let j;
    try {
      j = await api('/admin/api/jobs/' + encodeURIComponent(id));
    } catch (e) {
      clear(box).append(state(t('Задание не найдено'), e.message));
      return;
    }
    if (!ctx.alive()) return;
    const st = jobState(j.public_state);
    const rows = [
      cell({ label: t('Состояние'), extra: cstate(st.label, st.kind) }),
      cell({ label: t('Модель'), value: j.model_used !== j.model ? `${j.model} → ${j.model_used}` : j.model }),
      cell({ label: t('Апстрим'), value: j.upstream_used || '—' }),
      cell({ label: t('Клиент'), value: j.client }),
      cell({ label: 'chat_id', value: String(j.chat_id) }),
      cell({ label: 'reply_to', value: j.reply_to == null ? '—' : String(j.reply_to) }),
      cell({ label: 'thread_id', value: j.thread_id == null ? '—' : String(j.thread_id) }),
      cell({ label: t('Протокол'), value: String(j.protocol) }),
      j.position ? cell({ label: t('Позиция в очереди'), value: String(j.position) }) : null,
      cell({ label: t('Попыток'), value: String(j.attempts) + (j.hops ? ' · ' + t('резерв: {n}', { n: j.hops }) : '') }),
    ].filter(Boolean);
    const times = [
      cell({ label: t('Поставлено'), value: fmt.date(j.created_at) }),
      cell({ label: t('Начато'), value: fmt.date(j.started_at) }),
      cell({ label: t('Завершено'), value: fmt.date(j.finished_at) }),
      cell({ label: t('Ожидание'), value: fmt.ms(j.queued_ms) }),
      cell({ label: t('Ответ модели'), value: fmt.ms(j.latency_ms) }),
    ];
    const usage = j.usage ? (() => { try { return JSON.parse(j.usage); } catch (_) { return null; } })() : null;
    if (usage) times.push(cell({ label: t('Токены'), value: `${usage.prompt_tokens ?? '?'} + ${usage.completion_tokens ?? '?'}` }));
    const delivery = [
      cell({ label: t('Доставка'), value: { none: t('ещё не нужна'), pending: t('ожидает'), delivered: t('доставлено'), dead: t('не доставлено') }[j.delivery] || j.delivery }),
      cell({ label: t('Попыток доставки'), value: String(j.delivery_attempts) }),
      j.next_delivery_at && j.delivery === 'pending' ? cell({ label: t('Следующая попытка'), value: fmt.time(j.next_delivery_at) }) : null,
      cell({ label: t('Колбэк'), value: j.callback_url || '—', wrap: true, stack: true }),
      j.delivered_at ? cell({ label: t('Доставлено'), value: fmt.date(j.delivered_at) }) : null,
    ].filter(Boolean);
    const errors = [];
    if (j.error) errors.push(callout(`${j.error_code || ''} — ${j.error}`, 'error'));
    if (j.delivery_error) errors.push(callout(t('Доставка: {e}', { e: j.delivery_error }), 'warn'));
    const msgs = (j.messages || []).map((m) => cell({ label: m.role, value: m.kind === 'text' ? t('{n} символов', { n: m.size }) : m.kind === 'parts' ? t('частей: {n}', { n: m.size }) : '—' }));
    const actions = h('div.actions');
    if (j.public_state === 'queued' || j.public_state === 'running') {
      actions.append(btn(t('Отменить задание'), {
        kind: 'danger',
        onClick: async () => {
          if (!(await confirmDialog(t('Отменить задание?'), t('Бот получит error_code: cancelled и промолчит.'), { ok: t('Отменить'), danger: true }))) return;
          try {
            await api(`/admin/api/jobs/${encodeURIComponent(id)}/cancel`, { method: 'POST' });
            toast(t('Задание отменено'), 'ok');
            load();
          } catch (e) {
            errorToast(e);
          }
        },
      }));
    }
    if (j.delivery === 'dead') {
      actions.append(btn(t('Повторить доставку'), {
        kind: 'filled',
        onClick: async () => {
          try {
            await api(`/admin/api/jobs/${encodeURIComponent(id)}/redeliver`, { method: 'POST' });
            toast(t('Доставка поставлена в очередь'), 'ok');
            load();
          } catch (e) {
            errorToast(e);
          }
        },
      }));
    }
    clear(box).append(
      hero(j.job_id, fmt.ago(j.created_at)),
      errors.length ? h('div', { style: { margin: '0 16px 16px', display: 'grid', gap: '8px' } }, errors) : null,
      group(t('Задание'), list(...rows)),
      group(t('Время'), list(...times)),
      group(t('Доставка боту'), list(...delivery)),
      msgs.length ? group(t('Сообщения'), list(...msgs), t('Тексты сообщений и ответа в панели не показываются.')) : null,
      j.params && Object.keys(j.params).length ? group(t('Параметры запроса'), h('pre.json', { style: { margin: '0 16px' } }, JSON.stringify(j.params, null, 2))) : null,
      actions,
    );
  }
  await load();
  const refresh = debounce(load, 500);
  ctx.onDestroy(on('job', (e) => { if (e.job_id === id) refresh(); }));
}
