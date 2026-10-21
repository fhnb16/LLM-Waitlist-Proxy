import { api } from '../api.js';
import { t } from '../i18n.js';
import { h, clear, hero, group, list, cell, field, input, select, segmented, btn, fmt, toast, errorToast, state, clone, callout, cstate } from '../ui.js';
import { store, on, saveSettings } from '../store.js';

export async function render(ctx) {
  ctx.title(t('Отладка'));
  const root = ctx.root;
  const debugCfg = clone(store.settings.file.debug);
  const models = Object.keys(store.settings.file.models);
  const model = select(models.map((m) => ({ value: m, label: m })), models[0]);
  const text = input(t('Привет! Ответь одним словом.'));
  const chat = input('-1001234567890', { type: 'number' });
  const delay = input(debugCfg.delay_ms, { type: 'number', min: '0', max: '120000', step: '500' });
  const listBox = h('div');
  const result = h('div');

  const saveDebug = async () => {
    try {
      const file = clone(store.settings.file);
      file.debug = debugCfg;
      await saveSettings(file);
      toast(t('Приёмник: отвечает {c}', { c: { ok: '200', forbidden: '403', server_error: '500' }[debugCfg.reply] }), 'ok');
    } catch (e) {
      errorToast(e);
    }
  };
  delay.addEventListener('change', () => { debugCfg.delay_ms = Number(delay.value) || 0; saveDebug(); });

  root.append(
    hero(t('Отладка'), t('Встроенный эмулятор бота: принимает колбэки и показывает, что ушло бы на /webhook/llm.')),
    group(t('Как отвечает приёмник'), h('div', segmented([
      { value: 'ok', label: '200 OK' },
      { value: 'forbidden', label: '403' },
      { value: 'server_error', label: '500' },
    ], debugCfg.reply, (v) => { debugCfg.reply = v; saveDebug(); }), list(field(t('Задержка ответа, мс'), delay))),
    t('403 — задание сразу «не доставлено»; 500 — повторы через 2, 5, 15, 30, 60 с.')),
    models.length
      ? group(t('Тестовое задание'), list(
          field(t('Модель'), model),
          field(t('Сообщение'), text),
          field('chat_id', chat, { hint: t('Отрицательный — группа, положительный — личный чат') }),
          cell({ label: t('Поставить в очередь'), kind: 'tint', onClick: sendJob }),
        ), t('Задание идёт обычной очередью, а результат — в этот приёмник, минуя белый список колбэков.'))
      : callout(t('Добавьте модель, чтобы отправить тестовое задание.'), 'warn'),
    result,
    h('div.toolbar.toolbar--tight', { style: { display: 'flex', justifyContent: 'space-between', alignItems: 'center' } },
      h('span.toolbar__label', { style: { margin: '0' } }, t('Полученные колбэки')),
      btn(t('Очистить'), { inline: true, kind: 'ghost', onClick: async () => { await api('/admin/api/debug/callbacks', { method: 'DELETE' }); load(); } })),
    listBox,
    group(t('Адрес приёмника'), list(cell({ label: urlOf(), wrap: true })), t('Его можно указать как BOT_CALLBACK_URL (с CALLBACK_ALLOW_HTTP_PRIVATE и хостом в CALLBACK_ALLOWED_HOSTS), чтобы ловить настоящие задания.')),
  );

  function urlOf() {
    const b = store.overview && store.overview.bound && store.overview.bound.admin;
    return b ? `http://${b.replace('0.0.0.0', '127.0.0.1')}/admin/debug/callback` : '/admin/debug/callback';
  }

  async function sendJob() {
    clear(result);
    try {
      const r = await api('/admin/api/debug/job', { method: 'POST', body: { model: model.value, text: text.value, chat_id: Number(chat.value) || -1 } });
      result.append(callout(t('Задание {id} в очереди, позиция {p}. Колбэк появится ниже.', { id: r.job_id, p: r.position }), 'ok'));
    } catch (e) {
      result.append(callout(e.message, 'error'));
    }
  }

  function item(r) {
    const b = r.body || {};
    const ok = b.text != null || b.result;
    return h('section.group',
      h('h2.group__title', `#${r.id} · ${fmt.time(r.ts)} · ${t('ответ')} ${r.replied}${r.delay_ms ? ' · ' + t('задержка {v}', { v: fmt.ms(r.delay_ms) }) : ''}`),
      h('div.list',
        cell({ label: 'job_id', value: b.job_id || '—', extra: ok ? cstate(t('результат'), 'run') : cstate(b.error_code || t('ошибка'), 'bad') }),
        cell({ label: 'X-Callback-Token', value: r.token || t('нет') }),
        h('div.list__cell.list__cell--stack', h('pre.json', JSON.stringify(b, null, 2))),
      ),
    );
  }

  async function load() {
    try {
      const d = await api('/admin/api/debug/callbacks');
      if (!ctx.alive()) return;
      clear(listBox);
      if (!d.items.length) listBox.append(state(t('Пока ничего не пришло'), t('Отправьте тестовое задание — колбэк появится здесь.')));
      else listBox.append(...d.items.slice(0, 50).map(item));
    } catch (e) {
      clear(listBox).append(callout(e.message, 'error'));
    }
  }
  await load();
  ctx.onDestroy(on('debug_callback', load));
}
