import { api } from '../api.js';
import { t } from '../i18n.js';
import { barChart, sparkline } from '../charts.js';
import { h, clear, hero, group, list, cell, field, input, switchEl, btn, fmt, modal, toast, errorToast, confirmDialog, state, showSecret, navBtn, callout } from '../ui.js';
import { reloadSettings } from '../store.js';

export async function render(ctx) {
  ctx.title(t('Клиенты'));
  ctx.actions(navBtn(t('Добавить'), () => create(), 'plus'));
  const root = ctx.root;
  const box = h('div');
  const chart = h('div');
  root.append(hero(t('Клиенты API'), t('Ключи для бота: заголовок Authorization: Bearer <ключ>.')), box, chart);

  async function load() {
    let items;
    let stats = null;
    try {
      items = (await api('/admin/api/clients')).items;
      stats = await api('/admin/api/stats?range=24h&group=client');
    } catch (e) {
      clear(box).append(callout(e.message, 'error'));
      return;
    }
    if (!ctx.alive()) return;
    const byClient = {};
    for (const g of (stats && stats.groups) || []) byClient[g.key] = g;
    clear(box);
    if (!items.length) {
      box.append(state(t('Клиентов нет'), t('Создайте ключ и вставьте его в боте: Нейросеть → провайдер «Сервер очереди» → API-ключ.'), btn(t('Создать ключ'), { kind: 'filled', inline: true, onClick: create })));
    } else {
      box.append(list(...items.map((c) => {
        const g = byClient[c.name];
        const sw = c.from_env ? null : switchEl(c.enabled, async (v) => {
          try {
            await api('/admin/api/clients/' + encodeURIComponent(c.name), { method: 'PATCH', body: { enabled: v } });
            toast(v ? t('Клиент включён') : t('Клиент выключен'), 'ok');
          } catch (e) {
            errorToast(e);
          }
        }, t('Включён'));
        return cell({
          label: h('span.jobline',
            h('span.jobline__top', h('strong', c.name), c.from_env ? h('span.tag.tag--muted', 'API_KEYS') : null),
            h('span.jobline__meta',
              c.created_at ? h('span', t('создан {d}', { d: fmt.date(c.created_at) })) : null,
              g ? h('span', t('за сутки: {n} заданий, ошибок {p}', { n: g.totals.jobs, p: fmt.pct(g.totals.error_rate || 0) })) : h('span', t('за сутки заданий нет'))),
            c.note ? h('span.jobline__meta', c.note) : null),
          extra: h('span', { style: { display: 'inline-flex', gap: '10px', alignItems: 'center' } },
            g ? sparkline(g.points.map((p) => p.jobs)) : null,
            c.from_env ? null : btn(t('Ещё'), { inline: true, kind: 'ghost', onClick: () => menu(c) }),
            sw),
        });
      })));
      box.append(h('p.footnote', t('Ключ хранится только в виде хэша и показывается один раз. Клиенты из API_KEYS меняются в окружении.')));
    }
    clear(chart);
    if (stats && stats.groups.length) {
      const pts = stats.points.map((p, i) => {
        const o = { t: p.t };
        for (const g of stats.groups) o[g.key] = g.points[i].jobs;
        return o;
      });
      const colors = ['var(--tint)', 'var(--warn)', 'var(--text-2)', 'var(--ok)'];
      const series = stats.groups.slice(0, 4).map((g, i) => ({ key: g.key, label: g.key, color: colors[i] }));
      barChart(chart, { title: t('Задания по клиентам за сутки'), points: pts, series, note: stats.groups.length > 4 ? t('показаны первые четыре клиента') : '' });
    }
  }

  function create() {
    const name = input('', { placeholder: 'bot' });
    const note = input('', { placeholder: t('например: UniversalBot, прод') });
    modal({
      title: t('Новый клиент'),
      body: h('div.list', field(t('Имя'), name, { hint: t('Пишется в журнал и статистику') }), field(t('Заметка'), note)),
      actions: [
        { label: t('Отмена') },
        {
          label: t('Создать'),
          kind: 'filled',
          onClick: async () => {
            const r = await api('/admin/api/clients', { method: 'POST', body: { name: name.value.trim(), note: note.value.trim() } });
            await reloadSettings();
            load();
            setTimeout(() => showSecret(t('Ключ клиента {n}', { n: r.name }), t('Скопируйте ключ сейчас — больше он показан не будет.'), r.key), 50);
          },
        },
      ],
    });
  }

  function menu(c) {
    const m = modal({
      title: c.name,
      body: h('div.sheet',
        h('button.sheet__item', { type: 'button', on: { click: async () => {
          m.close();
          if (!(await confirmDialog(t('Выдать новый ключ?'), t('Старый ключ клиента {n} сразу перестанет работать.', { n: c.name }), { ok: t('Выдать') }))) return;
          try {
            const r = await api(`/admin/api/clients/${encodeURIComponent(c.name)}/rotate`, { method: 'POST' });
            showSecret(t('Новый ключ {n}', { n: c.name }), t('Скопируйте ключ сейчас — больше он показан не будет.'), r.key);
          } catch (e) { errorToast(e); }
        } } }, t('Выдать новый ключ')),
        h('button.sheet__item', { type: 'button', on: { click: () => {
          m.close();
          const note = input(c.note);
          modal({ title: t('Заметка'), body: h('div.list', field(t('Заметка'), note)), actions: [{ label: t('Отмена') }, { label: t('Сохранить'), kind: 'filled', onClick: async () => { await api('/admin/api/clients/' + encodeURIComponent(c.name), { method: 'PATCH', body: { note: note.value } }); load(); } }] });
        } } }, t('Изменить заметку')),
        h('button.sheet__item.sheet__item--danger', { type: 'button', on: { click: async () => {
          m.close();
          if (!(await confirmDialog(t('Удалить клиента {n}?', { n: c.name }), t('Бот с этим ключом получит 401.'), { ok: t('Удалить'), danger: true }))) return;
          try {
            await api('/admin/api/clients/' + encodeURIComponent(c.name), { method: 'DELETE' });
            toast(t('Клиент удалён'), 'ok');
            load();
          } catch (e) { errorToast(e); }
        } } }, t('Удалить')),
      ),
    });
  }

  await load();
}
