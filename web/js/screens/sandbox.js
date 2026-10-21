import { streamPost } from '../api.js';
import { t } from '../i18n.js';
import { h, clear, hero, group, list, field, input, textarea, pick, btn, fmt, toast, errorToast, state, callout } from '../ui.js';
import { store } from '../store.js';

export async function render(ctx) {
  ctx.title(t('Песочница'));
  const root = ctx.root;
  root.classList.add('app__screen--wide');
  const models = [...Object.keys(store.settings.file.models), ...Object.keys(store.settings.file.aliases || {})];
  root.append(hero(t('Песочница'), t('Один запрос нескольким моделям сразу: ответы потоком, время и скорость рядом.')));
  if (!models.length) {
    root.append(state(t('Моделей нет'), t('Сначала добавьте модель.')));
    return;
  }
  let saved = {};
  try {
    saved = JSON.parse(localStorage.getItem('lmq.sandbox') || '{}');
  } catch (_) { /* пусто */ }
  let chosen = (saved.models || [models[0]]).filter((m) => models.includes(m)).slice(0, 4);
  if (!chosen.length) chosen = [models[0]];
  const system = textarea(saved.system || '', { rows: 2, placeholder: t('Системный промпт (необязательно)') });
  const user = textarea(saved.user || t('Объясни в двух предложениях, что такое очередь заданий.'), { rows: 4 });
  const temp = input(saved.temperature ?? '0.7', { type: 'number', step: '0.1', min: '0', max: '2' });
  const maxTok = input(saved.max_tokens ?? '400', { type: 'number', min: '1', max: '32768' });
  const extra = textarea(saved.extra || '', { rows: 2, placeholder: '{"top_k": 20}' });
  const out = h('div');
  const picker = pick(models, chosen, (v) => {
    if (v.length > 4) {
      toast(t('Не больше четырёх моделей'), 'warn');
      v = v.slice(0, 4);
    }
    chosen = v;
  });
  let abort = null;
  const sendBtn = btn(t('Отправить'), { kind: 'filled', onClick: send });
  const stopBtn = btn(t('Остановить'), { kind: 'ghost', disabled: true, onClick: () => abort && abort.abort() });

  root.append(
    group(t('Модели'), picker, t('До четырёх. Запросы идут в синхронном режиме с высшим приоритетом и занимают слоты.')),
    group(t('Запрос'), h('div.list',
      h('div.field.field--stack', h('span.field__label', t('Системный')), system),
      h('div.field.field--stack', h('span.field__label', t('Пользователь')), user),
      field('temperature', temp),
      field('max_tokens', maxTok),
      h('div.field.field--stack', h('span.field__label', t('Другие параметры (JSON)')), extra))),
    h('div.btn-row', sendBtn, stopBtn),
    h('div', { style: { height: '18px' } }),
    out,
  );

  async function send() {
    let params = {};
    if (extra.value.trim()) {
      try {
        params = JSON.parse(extra.value);
      } catch (e) {
        toast(t('Параметры: неверный JSON'), 'error');
        return;
      }
    }
    if (temp.value !== '') params.temperature = Number(temp.value);
    if (maxTok.value !== '') params.max_tokens = Number(maxTok.value);
    const messages = [];
    if (system.value.trim()) messages.push({ role: 'system', content: system.value });
    messages.push({ role: 'user', content: user.value });
    try {
      localStorage.setItem('lmq.sandbox', JSON.stringify({ models: chosen, system: system.value, user: user.value, temperature: temp.value, max_tokens: maxTok.value, extra: extra.value }));
    } catch (_) { /* без запоминания */ }
    const cols = {};
    clear(out);
    const grid = h('div.compare', { class: 'compare--' + chosen.length });
    for (const m of chosen) {
      const text = h('div.compare__out');
      const stats = h('div.compare__stats', h('span.ring'), t('ждём слот…'));
      cols[m] = { text, stats, started: performance.now(), first: null, chars: 0 };
      grid.append(h('div.compare__col', h('div.compare__head', m), text, stats));
    }
    out.append(grid);
    const summary = h('div');
    out.append(summary);
    abort = new AbortController();
    sendBtn.disabled = true;
    stopBtn.disabled = false;
    const results = [];
    try {
      await streamPost('/admin/api/sandbox', { models: chosen, messages, params }, (ev, data) => {
        if (ev === 'delta') {
          const c = cols[data.model];
          if (!c) return;
          if (c.first === null) {
            c.first = performance.now() - c.started;
            clear(c.stats).append(h('span.ring'), t('генерация…'));
          }
          c.text.append(data.text);
          c.text.scrollTop = c.text.scrollHeight;
        } else if (ev === 'done') {
          const c = cols[data.model];
          if (!c) return;
          results.push(data);
          clear(c.stats);
          if (data.ok) {
            if (!c.text.textContent) c.text.textContent = data.text || '';
            c.stats.append(
              h('span', t('ответ {v}', { v: fmt.ms(data.latency_ms) })),
              data.first_token_ms != null ? h('span', t('первый токен {v}', { v: fmt.ms(data.first_token_ms) })) : null,
              h('span', t('токенов {a}+{b}', { a: data.prompt_tokens, b: data.completion_tokens })),
              data.tps ? h('span', t('{v} ток/с', { v: fmt.num(data.tps, 1) })) : null,
            );
          } else c.stats.append(h('span.compare__err', `${data.status || ''} ${data.error}`));
        }
      }, abort.signal);
    } catch (e) {
      if (e.name !== 'AbortError') errorToast(e);
    } finally {
      sendBtn.disabled = false;
      stopBtn.disabled = true;
      abort = null;
    }
    const ok = results.filter((r) => r.ok);
    if (ok.length > 1) {
      const fastest = ok.reduce((a, b) => (a.latency_ms < b.latency_ms ? a : b));
      const quickest = ok.filter((r) => r.tps).reduce((a, b) => (a && a.tps > b.tps ? a : b), null);
      summary.append(callout(
        t('Быстрее всех ответила {m} ({v}).', { m: fastest.model, v: fmt.ms(fastest.latency_ms) }) +
        (quickest ? ' ' + t('Выше всего скорость у {m}: {v} ток/с.', { m: quickest.model, v: fmt.num(quickest.tps, 1) }) : ''),
        'ok',
      ));
    }
  }
}
