// Журнал в реальном времени: автопрокрутка, пауза, поиск с подсветкой, фильтр
// уровня, выгрузка в файл. Строка: { ts, level, src, text, fields? }.

import { t } from '../i18n.js';
import { h, clear, segmented, btn, download, debounce } from '../ui.js';

const CAP = 5000;
const RANK = { trace: 0, debug: 1, info: 2, sys: 2, warn: 3, error: 4 };

export function logView(container, { load, filename = 'log.txt', onClear, short = false } = {}) {
  let lines = [];
  let paused = false;
  let pending = [];
  let minLevel = 0;
  let query = '';
  const body = h('div.logview__body', { class: short ? 'logview__body--short' : null, role: 'log', 'aria-live': 'off', tabindex: '0' });
  const search = h('input.logview__search', { type: 'search', placeholder: t('Поиск по журналу'), 'aria-label': t('Поиск по журналу') });
  const pauseBtn = btn(t('Пауза'), { inline: true, onClick: () => setPaused(!paused) });
  const levels = segmented(
    [{ value: 0, label: t('Все') }, { value: 2, label: 'info+' }, { value: 3, label: 'warn+' }, { value: 4, label: 'error' }],
    0,
    (v) => {
      minLevel = v;
      redraw();
    },
  );
  levels.style.margin = '0';
  const counter = h('span', { style: { fontSize: '12px', color: 'var(--text-3)' } });
  container.append(
    h('div.logview',
      h('div.logview__bar', search, levels, pauseBtn,
        btn(t('Скачать'), { inline: true, kind: 'ghost', onClick: () => download(filename, lines.map(plain).join('\n')) }),
        onClear ? btn(t('Очистить'), { inline: true, kind: 'ghost', onClick: async () => { await onClear(); lines = []; redraw(); } }) : null,
        counter),
      body),
  );
  search.addEventListener('input', debounce(() => {
    query = search.value.trim().toLowerCase();
    redraw();
  }, 150));

  function plain(l) {
    const d = new Date(l.ts);
    return `${d.toLocaleString()} [${l.src || l.level}] ${l.text}${l.fields ? ' ' + l.fields : ''}`;
  }

  function matches(l) {
    if ((RANK[l.level] ?? 2) < minLevel) return false;
    if (!query) return true;
    return (l.text + ' ' + (l.fields || '')).toLowerCase().includes(query);
  }

  function highlight(text) {
    if (!query) return [text];
    const out = [];
    const low = text.toLowerCase();
    let i = 0;
    for (;;) {
      const j = low.indexOf(query, i);
      if (j < 0) {
        out.push(text.slice(i));
        break;
      }
      out.push(text.slice(i, j), h('mark.logview__mark', text.slice(j, j + query.length)));
      i = j + query.length;
    }
    return out;
  }

  function row(l) {
    const d = new Date(l.ts);
    const ts = d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
    return h('div.logview__line', { class: 'logview__line--' + (l.level || 'info') },
      h('span.logview__ts', ts),
      h('span.logview__lvl', l.src && l.src !== 'sys' ? l.src : l.level),
      h('span.logview__msg', ...highlight(l.text), l.fields ? h('span.logview__fields', ' ' + l.fields) : null));
  }

  function nearBottom() {
    return body.scrollHeight - body.scrollTop - body.clientHeight < 40;
  }

  function redraw() {
    clear(body);
    const shown = lines.filter(matches);
    if (!shown.length) body.append(h('div.logview__empty', lines.length ? t('Ничего не найдено') : t('Пока пусто')));
    const frag = document.createDocumentFragment();
    for (const l of shown.slice(-2000)) frag.append(row(l));
    body.append(frag);
    body.scrollTop = body.scrollHeight;
    counter.textContent = t('строк: {n}', { n: lines.length });
  }

  function setPaused(p) {
    paused = p;
    pauseBtn.textContent = p ? t('Продолжить') + (pending.length ? ` (${pending.length})` : '') : t('Пауза');
    if (!p && pending.length) {
      lines.push(...pending);
      pending = [];
      if (lines.length > CAP) lines = lines.slice(-CAP);
      redraw();
    }
  }

  function push(l) {
    if (paused) {
      pending.push(l);
      if (pending.length > CAP) pending.shift();
      pauseBtn.textContent = t('Продолжить') + ` (${pending.length})`;
      return;
    }
    lines.push(l);
    if (lines.length > CAP) lines.shift();
    if (!matches(l)) return;
    const stick = nearBottom();
    const empty = body.querySelector('.logview__empty');
    if (empty) empty.remove();
    body.append(row(l));
    while (body.childNodes.length > 2000) body.removeChild(body.firstChild);
    if (stick) body.scrollTop = body.scrollHeight;
    counter.textContent = t('строк: {n}', { n: lines.length });
  }

  if (load) {
    load().then((ls) => {
      lines = ls.slice(-CAP);
      redraw();
    }).catch(() => redraw());
  } else redraw();

  return { push, destroy() {} };
}
