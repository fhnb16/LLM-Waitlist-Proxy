// Построение интерфейса из BEM-блоков каркаса. Строки вставляются только как текст.

import { t, locale } from './i18n.js';

// Необязательные блоки экранов передаются как null/false (`cond ? node : null`).
// Штатный append превратил бы их в текст «null», поэтому они пропускаются.
for (const proto of [Element.prototype, DocumentFragment.prototype]) {
  const native = proto.append;
  proto.append = function (...nodes) {
    return native.apply(this, nodes.filter((n) => n !== null && n !== undefined && n !== false));
  };
}

/**
 * h('div.list', {attrs}, ...children). Атрибуты: class, on: {event: fn}, style,
 * dataset, остальные — setAttribute; свойства value/checked/disabled — напрямую.
 */
export function h(tag, attrs, ...children) {
  const [name, ...classes] = tag.split('.');
  const el = document.createElement(name || 'div');
  if (classes.length) el.className = classes.join(' ');
  if (attrs && (typeof attrs !== 'object' || attrs instanceof Node || Array.isArray(attrs))) {
    children.unshift(attrs);
    attrs = null;
  }
  if (attrs) {
    for (const [k, v] of Object.entries(attrs)) {
      if (v === undefined || v === null || v === false) continue;
      if (k === 'on') for (const [ev, fn] of Object.entries(v)) el.addEventListener(ev, fn);
      else if (k === 'class') el.className = [el.className, v].filter(Boolean).join(' ');
      else if (k === 'style' && typeof v === 'object') Object.assign(el.style, v);
      else if (k === 'dataset') Object.assign(el.dataset, v);
      else if (k === 'value' || k === 'checked' || k === 'disabled' || k === 'selected' || k === 'indeterminate') el[k] = v;
      else if (k === 'text') el.textContent = v;
      else el.setAttribute(k, v === true ? '' : v);
    }
  }
  append(el, children);
  return el;
}

export function append(el, children) {
  for (const c of children.flat(Infinity)) {
    if (c === null || c === undefined || c === false) continue;
    el.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
  return el;
}

export function clear(el) {
  while (el.firstChild) el.removeChild(el.firstChild);
  return el;
}

const SVGNS = 'http://www.w3.org/2000/svg';
export function svg(tag, attrs, ...children) {
  const el = document.createElementNS(SVGNS, tag);
  for (const [k, v] of Object.entries(attrs || {})) if (v !== undefined && v !== null) el.setAttribute(k, v);
  for (const c of children.flat()) if (c) el.append(c instanceof Node ? c : document.createTextNode(String(c)));
  return el;
}

/** Значок из набора (контурные пути 24×24). */
export function icon(name, cls = 'tab-bar__icon') {
  const paths = ICONS[name] || ICONS.dot;
  return svg('svg', { class: cls, viewBox: '0 0 24 24', 'aria-hidden': 'true' }, ...paths.map((d) => svg('path', { d })));
}

export const ICONS = {
  chevron: ['M6 9l6 6 6-6'],
  minus: ['M5 12h14'],
  overview: ['M4 13h4v7H4z', 'M10 9h4v11h-4z', 'M16 4h4v16h-4z'],
  queue: ['M4 6h16', 'M4 12h16', 'M4 18h10', 'M18 16l2 2-2 2'],
  models: ['M12 3l8 4.5v9L12 21l-8-4.5v-9z', 'M12 12l8-4.5', 'M12 12v9', 'M12 12L4 7.5'],
  engines: ['M4 7h16v10H4z', 'M8 11h1', 'M12 11h4', 'M7 17v3', 'M17 17v3', 'M7 4v3', 'M17 4v3'],
  clients: ['M15 7a4 4 0 1 1-8 0 4 4 0 0 1 8 0z', 'M3 21c0-4 4-6 8-6s8 2 8 6', 'M19 8v6', 'M16 11h6'],
  logs: ['M6 3h9l4 4v14H6z', 'M14 3v5h5', 'M9 12h7', 'M9 16h7'],
  sandbox: ['M9 3h6', 'M10 3v6L5 19a1.5 1.5 0 0 0 1.3 2h11.4a1.5 1.5 0 0 0 1.3-2L14 9V3', 'M7.5 15h9'],
  debug: ['M8 8a4 4 0 0 1 8 0v6a4 4 0 0 1-8 0z', 'M4 11h4', 'M16 11h4', 'M5 18l3-2', 'M19 18l-3-2', 'M9 4l-2-2', 'M15 4l2-2'],
  settings: ['M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z', 'M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z'],
  more: ['M5 12h.01', 'M12 12h.01', 'M19 12h.01'],
  schedule: ['M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18z', 'M12 7v5l3 2'],
  palette: ['M12 3a9 9 0 1 0 0 18c1 0 1.5-.8 1.5-1.5 0-.4-.2-.8-.4-1.1-.3-.3-.4-.7-.4-1.1 0-.8.7-1.5 1.5-1.5H16a5 5 0 0 0 5-5c0-4.4-4-7.8-9-7.8z', 'M7.5 11.5h.01', 'M10 7.5h.01', 'M14.5 7.5h.01'],
  profiles: ['M4 5h16v14H4z', 'M4 9h16', 'M8 13h5'],
  app: ['M3 5h18v12H3z', 'M8 21h8', 'M12 17v4'],
  update: ['M21 12a9 9 0 1 1-3-6.7', 'M21 4v5h-5'],
  info: ['M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18z', 'M12 11v5', 'M12 8h.01'],
  pause: ['M8 5v14', 'M16 5v14'],
  play: ['M7 4l13 8-13 8z'],
  refresh: ['M20 11a8 8 0 0 0-14.9-3', 'M4 4v4h4', 'M4 13a8 8 0 0 0 14.9 3', 'M20 20v-4h-4'],
  plus: ['M12 5v14', 'M5 12h14'],
  trash: ['M4 7h16', 'M9 7V4h6v3', 'M6 7l1 13h10l1-13'],
  copy: ['M8 8h12v12H8z', 'M4 16V4h12'],
  download: ['M12 3v12', 'M7 10l5 5 5-5', 'M5 21h14'],
  stop: ['M6 6h12v12H6z'],
  back: ['M15 5l-7 7 7 7'],
  dot: ['M12 12h.01'],
};

export function chevron() {
  return svg('svg', { class: 'list__chevron', viewBox: '0 0 8 14', 'aria-hidden': 'true' }, svg('path', { d: 'M1 1l6 6-6 6' }));
}

// ------------------------------------------------------------- блоки каркаса

export function hero(title, sub) {
  return h('header.hero', h('h1.hero__title', title), sub ? h('p.hero__sub', sub) : null);
}

export function group(title, children, note, opts = {}) {
  return h(
    'section.group',
    { class: opts.spaced ? 'group__title--spaced' : null },
    title ? h('h2.group__title', title) : null,
    children,
    note ? h('p.group__note', { class: opts.noteWarn ? 'group__note--warn' : null }, note) : null,
  );
}

export function list(...children) {
  return h('div.list', ...children);
}

/** Строка списка. onClick делает её кнопкой со стрелкой. */
export function cell({ label, value, onClick, kind, stack, wrap, muted, extra, title } = {}) {
  const isBtn = !!onClick;
  const el = h(isBtn ? 'button.list__cell' : 'div.list__cell', {
    type: isBtn ? 'button' : null,
    title,
    class: [
      isBtn ? 'list__cell--link' : '',
      kind === 'tint' ? 'list__cell--tint' : '',
      kind === 'danger' ? 'list__cell--danger' : '',
      stack ? 'list__cell--stack' : '',
    ].join(' '),
    on: isBtn ? { click: onClick } : null,
  });
  if (kind === 'tint' || kind === 'danger') {
    append(el, [label]);
    return el;
  }
  append(el, [
    h('span.list__label', label),
    value !== undefined && value !== null
      ? h('span.list__value', { class: [wrap ? 'list__value--wrap' : '', muted ? 'list__value--muted' : ''].join(' ') }, value)
      : null,
    extra || null,
    isBtn ? chevron() : null,
  ]);
  return el;
}

export function switchEl(checked, onChange, ariaLabel, disabled) {
  const input = h('input.switch__input', { type: 'checkbox', checked, disabled, 'aria-label': ariaLabel || '' });
  input.addEventListener('change', () => onChange && onChange(input.checked));
  return h('label.switch', input, h('span.switch__track'), h('span.switch__thumb'));
}

export function cellSwitch(label, checked, onChange, opts = {}) {
  const c = h('div.list__cell', h('span.list__label', label, opts.sub ? h('span.list__value.list__value--muted', { style: { display: 'block', textAlign: 'left', maxWidth: 'none' } }, opts.sub) : null), switchEl(checked, onChange, label, opts.disabled));
  return c;
}

/** Строка формы «подпись | поле». */
export function field(label, control, { hint, stack, lock } = {}) {
  // У составных полей (combo, stepper) подпись ведёт на их текстовое поле.
  const target = control.matches && control.matches('.combo, .stepper') ? control.querySelector('input') : control;
  const id = target.id || 'f' + Math.random().toString(36).slice(2, 9);
  if (target.tagName === 'INPUT' || target.tagName === 'SELECT' || target.tagName === 'TEXTAREA') target.id = id;
  return h(
    'label.field',
    { class: stack ? 'field--stack' : null, for: id },
    h('span.field__label', label),
    control,
    lock ? h('span.field__lock', lock) : null,
    hint ? h('span.field__hint', hint) : null,
  );
}

export function input(value, attrs = {}) {
  return h('input.field__input', { type: 'text', value: value ?? '', autocomplete: 'off', spellcheck: 'false', ...attrs });
}

/**
 * Поле «ввод + выпадающий список» (WAI-ARIA combobox): выбрать вариант или вписать своё.
 * `options` — [{ value, label?, hint? }]; `onChange(text)` — на каждый ввод и выбор.
 * Список открывается кнопкой, стрелкой вниз или вводом (тогда он фильтруется по тексту).
 */
export function combo(value, options = [], { placeholder, onChange, disabled, inputmode, label } = {}) {
  const id = 'c' + Math.random().toString(36).slice(2, 9);
  const inp = h('input.combo__input', {
    type: 'text', value: value ?? '', placeholder, autocomplete: 'off', spellcheck: 'false', disabled, inputmode,
    role: 'combobox', 'aria-expanded': 'false', 'aria-controls': id + '-list', 'aria-autocomplete': 'list', 'aria-label': label,
  });
  const btn = h('button.combo__toggle', { type: 'button', tabindex: '-1', disabled, 'aria-label': t('Показать варианты') }, icon('chevron', 'combo__icon'));
  const list = h('ul.combo__list', { id: id + '-list', role: 'listbox' });
  const root = h('div.combo', inp, btn);
  let opts = options;
  let shown = [];
  let active = -1;
  let isOpen = false;

  const norm = (o) => (typeof o === 'object' ? o : { value: String(o) });
  function paint(filter) {
    const q = (filter || '').trim().toLowerCase();
    shown = opts.map(norm).filter((o) => !q || String(o.value).toLowerCase().includes(q) || (o.label || '').toLowerCase().includes(q) || (o.hint || '').toLowerCase().includes(q));
    clear(list);
    if (!shown.length) list.append(h('li.combo__empty', t('Нет вариантов — можно вписать своё')));
    shown.forEach((o, i) => {
      const li = h('li.combo__option', { role: 'option', id: `${id}-o${i}`, 'aria-selected': String(String(o.value) === inp.value), class: i === active ? 'combo__option--active' : null },
        h('span.combo__value', o.label || String(o.value) || t('(пусто)')),
        o.hint ? h('span.combo__hint', o.hint) : null);
      // pointerdown, а не click: иначе поле теряет фокус и список закрывается раньше выбора.
      li.addEventListener('pointerdown', (e) => { e.preventDefault(); pick(o); });
      list.append(li);
    });
  }
  function place() {
    const r = root.getBoundingClientRect();
    const below = window.innerHeight - r.bottom;
    list.style.left = r.left + 'px';
    list.style.width = r.width + 'px';
    if (below < 220 && r.top > below) {
      list.style.top = '';
      list.style.bottom = window.innerHeight - r.top + 4 + 'px';
    } else {
      list.style.bottom = '';
      list.style.top = r.bottom + 4 + 'px';
    }
  }
  function open(filter) {
    if (disabled || isOpen) return paint(filter);
    isOpen = true;
    active = -1;
    paint(filter);
    document.body.append(list);
    place();
    inp.setAttribute('aria-expanded', 'true');
    root.classList.add('combo--open');
    window.addEventListener('scroll', close, true);
    window.addEventListener('resize', close);
  }
  function close() {
    if (!isOpen) return;
    isOpen = false;
    list.remove();
    inp.setAttribute('aria-expanded', 'false');
    inp.removeAttribute('aria-activedescendant');
    root.classList.remove('combo--open');
    window.removeEventListener('scroll', close, true);
    window.removeEventListener('resize', close);
  }
  function pick(o) {
    inp.value = String(o.value);
    close();
    if (onChange) onChange(inp.value);
    inp.dispatchEvent(new Event('change', { bubbles: true }));
  }
  function move(d) {
    if (!isOpen) open();
    if (!shown.length) return;
    active = (active + d + shown.length) % shown.length;
    [...list.children].forEach((li, i) => li.classList.toggle('combo__option--active', i === active));
    inp.setAttribute('aria-activedescendant', `${id}-o${active}`);
    list.children[active]?.scrollIntoView({ block: 'nearest' });
  }
  inp.addEventListener('input', () => { if (onChange) onChange(inp.value); open(inp.value); });
  inp.addEventListener('keydown', (e) => {
    if (e.key === 'ArrowDown') { e.preventDefault(); move(1); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); move(-1); }
    else if (e.key === 'Enter' && isOpen && active >= 0) { e.preventDefault(); pick(shown[active]); }
    else if (e.key === 'Escape' && isOpen) { e.preventDefault(); close(); }
  });
  inp.addEventListener('blur', () => setTimeout(close, 120));
  btn.addEventListener('pointerdown', (e) => { e.preventDefault(); if (isOpen) close(); else { inp.focus(); open(); } });

  root.input = inp;
  root.setOptions = (o) => { opts = o; if (isOpen) paint(); };
  root.setValue = (v) => { inp.value = v ?? ''; };
  root.close = close;
  return root;
}

/** Подпись значения с единицей: секунды → «5 мин», мс → «1,5 с», объёмы → «8 МБ». */
function unitLabel(v, unit) {
  if (v === '' || v === null || v === undefined) return '';
  switch (unit) {
    case 's': return Number(v) === 0 ? null : fmt.secs(Number(v));
    case 'ms': return Number(v) === 0 ? null : fmt.ms(Number(v));
    case 'h': return Number(v) >= 48 && Number(v) % 24 === 0 ? `${Number(v) / 24} ${t('дн')}` : null;
    default: return null;
  }
}

/**
 * Число: поле с кнопками −/+ и списком готовых значений; ручной ввод проверяется
 * (целые или с `decimals` знаками, `min`/`max`). Пусто — `null`, если `nullable`.
 * Вместо `input type=number`: тот теряет ввод при запятой и меняется колесом мыши.
 */
export function numberInput(value, { min, max, step = 1, decimals = 0, presets = [], unit, nullable, placeholder, disabled, onChange, label, zero } = {}) {
  const opts = presets.map((p) => {
    const sub = unitLabel(p, unit);
    const lbl = p === 0 && zero ? `0 — ${zero}` : sub ? `${p} — ${sub}` : String(p);
    return { value: String(p), label: lbl };
  });
  if (nullable) opts.unshift({ value: '', label: placeholder || t('не задано') });
  let current = value === null || value === undefined ? null : Number(value);
  const c = combo(value ?? '', opts, { placeholder, disabled, inputmode: decimals ? 'decimal' : 'numeric', label, onChange: (s) => apply(s) });
  const inp = c.input;
  const minus = h('button.stepper__btn', { type: 'button', disabled, 'aria-label': t('Меньше') }, icon('minus', 'stepper__icon'));
  const plus = h('button.stepper__btn', { type: 'button', disabled, 'aria-label': t('Больше') }, icon('plus', 'stepper__icon'));
  const root = h('div.stepper', minus, c, plus);
  const round = (n) => Number(n.toFixed(decimals));

  function parse(s) {
    const txt = String(s).trim().replace(',', '.');
    if (txt === '') return nullable ? { ok: true, v: null } : { ok: false, msg: t('Нужно число') };
    if (!/^-?\d+(\.\d+)?$/.test(txt)) return { ok: false, msg: t('Нужно число') };
    const n = Number(txt);
    if (!decimals && !Number.isInteger(n)) return { ok: false, msg: t('Нужно целое число') };
    if (min !== undefined && n < min) return { ok: false, msg: t('Не меньше {n}', { n: min }) };
    if (max !== undefined && n > max) return { ok: false, msg: t('Не больше {n}', { n: max }) };
    return { ok: true, v: n };
  }
  function apply(s) {
    const r = parse(s);
    root.classList.toggle('stepper--invalid', !r.ok);
    inp.setAttribute('aria-invalid', String(!r.ok));
    inp.title = r.ok ? '' : r.msg;
    if (r.ok) {
      current = r.v;
      if (onChange) onChange(r.v);
    }
  }
  function bump(d) {
    let n = current ?? (min !== undefined ? min : 0);
    n = round(n + d * step);
    if (min !== undefined) n = Math.max(min, n);
    if (max !== undefined) n = Math.min(max, n);
    inp.value = String(n);
    apply(inp.value);
  }
  minus.addEventListener('click', () => bump(-1));
  plus.addEventListener('click', () => bump(1));
  inp.addEventListener('keydown', (e) => {
    // PageUp/PageDown — шаг; стрелки заняты списком.
    if (e.key === 'PageUp') { e.preventDefault(); bump(1); }
    if (e.key === 'PageDown') { e.preventDefault(); bump(-1); }
  });
  root.input = inp;
  return root;
}

/** Время `HH:MM`: список с шагом 30 минут и ручной ввод. */
export function timeInput(value, { onChange, disabled, label } = {}) {
  const opts = [];
  for (let m = 0; m < 24 * 60; m += 30) opts.push(String(Math.floor(m / 60)).padStart(2, '0') + ':' + String(m % 60).padStart(2, '0'));
  const c = combo(value, opts, { disabled, label, inputmode: 'numeric', onChange: (s) => {
    const ok = /^([01]?\d|2[0-3]):[0-5]\d$/.test(s.trim());
    c.classList.toggle('combo--invalid', !ok);
    c.input.setAttribute('aria-invalid', String(!ok));
    if (ok && onChange) onChange(s.trim().padStart(5, '0'));
  } });
  return c;
}

export function select(options, value, attrs = {}) {
  const s = h('select.field__select', attrs);
  for (const o of options) {
    const opt = typeof o === 'object' ? o : { value: o, label: o };
    s.append(h('option', { value: opt.value, selected: String(opt.value) === String(value) }, opt.label));
  }
  return s;
}

export function textarea(value, attrs = {}) {
  return h('textarea.code', { spellcheck: 'false', ...attrs, value: value ?? '' });
}

export function segmented(options, value, onChange) {
  const wrap = h('div.segmented', { role: 'tablist' });
  const render = (v) => {
    clear(wrap);
    for (const o of options) {
      const b = h('button.segmented__item', {
        type: 'button',
        role: 'tab',
        'aria-selected': String(o.value === v),
        class: o.value === v ? 'segmented__item--active' : null,
        on: {
          click: () => {
            render(o.value);
            onChange(o.value);
          },
        },
      }, o.label);
      wrap.append(b);
    }
  };
  render(value);
  wrap.setValue = render;
  return wrap;
}

export function btn(label, opts = {}) {
  const cls = ['btn', opts.kind ? 'btn--' + opts.kind : '', opts.inline ? 'btn--inline' : ''].filter(Boolean).join(' ');
  const b = h('button', { type: 'button', class: cls, disabled: opts.disabled, title: opts.title, on: opts.onClick ? { click: opts.onClick } : null });
  if (opts.icon) b.append(icon(opts.icon, 'nav-bar__icon'));
  b.append(label);
  return b;
}

export function navBtn(label, onClick, iconName) {
  const b = h('button.nav-bar__btn', { type: 'button', title: label, 'aria-label': label, on: { click: onClick } });
  if (iconName) {
    b.classList.add('nav-bar__btn--icon');
    b.append(icon(iconName, 'nav-bar__icon'));
  } else b.append(h('span.nav-bar__btn-label', label));
  return b;
}

export function cstate(text, kind) {
  return h('span.cstate', { class: kind ? 'cstate--' + kind : null }, text);
}

export function badge(text, kind) {
  return h('span.badge', { class: kind ? 'badge--' + kind : null }, text);
}

export function callout(text, kind) {
  const icons = { ok: '✓', warn: '!', error: '×' };
  return h('div.callout', { class: kind ? 'callout--' + kind : null }, h('span.callout__icon', icons[kind] || 'i'), h('div.callout__text', text));
}

export function state(title, text, action) {
  return h('div.state', h('p.state__title', title), text ? h('p.state__text', text) : null, action || null);
}

export function ring(lg) {
  return h('span.ring', { class: lg ? 'ring--lg' : null, role: 'status', 'aria-label': t('Загрузка') });
}

export function loader() {
  return h('div.loader', ring(true));
}

export function meter(label, value, fraction, warn) {
  const pct = Math.max(0, Math.min(1, fraction || 0)) * 100;
  return h(
    'div.meter',
    h('div.meter__head', h('span.meter__label', label), h('span.meter__value', value)),
    h('div.meter__track', h('div.meter__fill', { class: warn ? 'meter__fill--warn' : null, style: { width: pct.toFixed(1) + '%' } })),
  );
}

export function pick(options, selected, onChange, { single = false } = {}) {
  const wrap = h('div.pick');
  let sel = new Set(selected);
  const render = () => {
    clear(wrap);
    for (const o of options) {
      const opt = typeof o === 'object' ? o : { value: o, label: o };
      const on = sel.has(opt.value);
      wrap.append(
        h('button.pick__item', {
          type: 'button',
          'aria-pressed': String(on),
          class: on ? 'pick__item--on' : null,
          on: {
            click: () => {
              if (single) sel = new Set([opt.value]);
              else if (on) sel.delete(opt.value);
              else sel.add(opt.value);
              render();
              onChange([...sel]);
            },
          },
        }, opt.label),
      );
    }
  };
  render();
  return wrap;
}

// -------------------------------------------------------------- уведомления

let toastTimer = null;
export function toast(text, kind, action) {
  const el = document.getElementById('toast');
  if (!el) return;
  clear(el);
  el.className = 'toast toast--shown' + (kind ? ' toast--' + kind : '');
  el.append(h('span.toast__text', text));
  if (action) {
    el.style.pointerEvents = 'auto';
    el.append(h('button.toast__action', { type: 'button', on: { click: () => { action.onClick(); el.classList.remove('toast--shown'); } } }, action.label));
  } else el.style.pointerEvents = 'none';
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => el.classList.remove('toast--shown'), action ? 6000 : 3200);
}

export function errorToast(e) {
  toast(t(e && e.message ? e.message : String(e)), 'error');
}

// ------------------------------------------------------------- модальное окно

let scrollY = 0;
/**
 * Модальное окно (лист снизу на телефоне, по центру шире 768px): фокус внутри,
 * Esc и нажатие на затемнение закрывают, фокус возвращается на кнопку-источник.
 */
export function modal({ title, sub, body, actions = [], onClose, wide }) {
  const opener = document.activeElement;
  const titleId = 'm' + Math.random().toString(36).slice(2, 8);
  const close = () => {
    document.removeEventListener('keydown', onKey, true);
    wrap.remove();
    if (!document.querySelector('.modal')) {
      document.documentElement.classList.remove('modal-open');
      window.scrollTo(0, scrollY);
    }
    if (opener && opener.focus) opener.focus();
    onClose && onClose();
  };
  const foot = actions.length
    ? h('div.modal__foot', actions.map((a) =>
        btn(a.label, {
          kind: a.kind,
          disabled: a.disabled,
          onClick: async (ev) => {
            const b = ev.currentTarget;
            if (a.onClick) {
              b.disabled = true;
              try {
                const keep = await a.onClick(ev);
                if (keep !== true) close();
              } catch (e) {
                errorToast(e);
              } finally {
                b.disabled = false;
              }
            } else close();
          },
        }),
      ))
    : null;
  const panel = h(
    'div.modal__panel',
    { role: 'dialog', 'aria-modal': 'true', 'aria-labelledby': titleId, style: wide ? { maxWidth: '900px' } : null },
    h(
      'div.modal__head',
      h('div.modal__titles', h('h2.modal__title', { id: titleId }, title), sub ? h('p.modal__sub', sub) : null),
      h('button.modal__close', { type: 'button', 'aria-label': t('Закрыть'), on: { click: close } }, '✕'),
    ),
    h('div.modal__body', body),
    foot,
  );
  const wrap = h('div.modal', { on: { mousedown: (e) => { if (e.target === wrap) close(); } } }, panel);
  const onKey = (e) => {
    if (e.key === 'Escape') {
      e.stopPropagation();
      close();
    } else if (e.key === 'Tab') {
      const f = [...panel.querySelectorAll('button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])')].filter((x) => !x.disabled && x.offsetParent !== null);
      if (!f.length) return;
      const first = f[0];
      const last = f[f.length - 1];
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    }
  };
  if (!document.querySelector('.modal')) {
    scrollY = window.scrollY;
    document.documentElement.classList.add('modal-open');
  }
  document.addEventListener('keydown', onKey, true);
  document.body.append(wrap);
  const focusable = panel.querySelector('.modal__body input, .modal__body select, .modal__body textarea') || panel.querySelector('.modal__foot .btn--filled') || panel.querySelector('.modal__close');
  setTimeout(() => focusable && focusable.focus(), 30);
  return { close, panel };
}

export function confirmDialog(title, text, { ok = t('Подтвердить'), danger = false } = {}) {
  return new Promise((resolve) => {
    let done = false;
    modal({
      title,
      body: h('p', { style: { margin: '0', color: 'var(--text-2)' } }, text),
      onClose: () => { if (!done) resolve(false); },
      actions: [
        { label: t('Отмена') },
        { label: ok, kind: danger ? 'danger' : 'filled', onClick: () => { done = true; resolve(true); } },
      ],
    });
  });
}

/** Ключ или токен, показанный один раз, с кнопкой «Копировать». */
export function showSecret(title, text, secret) {
  const value = h('div.secret__value', secret);
  modal({
    title,
    body: [
      h('p', { style: { margin: '0 0 8px', color: 'var(--text-2)', fontSize: '14px' } }, text),
      h('div.secret', value, btn(t('Копировать'), { inline: true, onClick: () => copy(secret) })),
    ],
    actions: [{ label: t('Готово'), kind: 'filled' }],
  });
}

// --------------------------------------------------------------- утилиты

export async function copy(text) {
  try {
    await navigator.clipboard.writeText(text);
    toast(t('Скопировано'), 'ok');
  } catch (_) {
    const ta = h('textarea', { value: text, style: { position: 'fixed', opacity: '0' } });
    document.body.append(ta);
    ta.select();
    document.execCommand('copy');
    ta.remove();
    toast(t('Скопировано'), 'ok');
  }
}

export function download(name, text, type = 'text/plain;charset=utf-8') {
  const url = URL.createObjectURL(new Blob([text], { type }));
  const a = h('a', { href: url, download: name });
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 2000);
}

export function debounce(fn, ms) {
  let tm = null;
  return (...a) => {
    clearTimeout(tm);
    tm = setTimeout(() => fn(...a), ms);
  };
}

export const fmt = {
  num(v, digits = 0) {
    if (v === null || v === undefined || Number.isNaN(v)) return '—';
    return Number(v).toLocaleString(locale(), { maximumFractionDigits: digits, minimumFractionDigits: 0 });
  },
  ms(v) {
    if (v === null || v === undefined || Number.isNaN(v)) return '—';
    if (v > 0 && v < 10) return v.toLocaleString(locale(), { maximumFractionDigits: 1 }) + ' ' + t('мс');
    if (v < 1000) return Math.round(v) + ' ' + t('мс');
    if (v < 60000) return (v / 1000).toLocaleString(locale(), { maximumFractionDigits: v < 10000 ? 1 : 0 }) + ' ' + t('с');
    const m = Math.floor(v / 60000);
    const s = Math.round((v % 60000) / 1000);
    return m + ' ' + t('мин') + (s ? ' ' + s + ' ' + t('с') : '');
  },
  secs(v) {
    return fmt.ms((v || 0) * 1000);
  },
  pct(v) {
    if (v === null || v === undefined) return '—';
    return (v * 100).toLocaleString(locale(), { maximumFractionDigits: v < 0.1 ? 1 : 0 }) + '%';
  },
  time(ts) {
    if (!ts) return '—';
    return new Date(ts).toLocaleTimeString(locale(), { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  },
  hm(ts) {
    return new Date(ts).toLocaleTimeString(locale(), { hour: '2-digit', minute: '2-digit' });
  },
  date(ts) {
    if (!ts) return '—';
    return new Date(ts).toLocaleString(locale(), { day: '2-digit', month: '2-digit', hour: '2-digit', minute: '2-digit' });
  },
  ago(ts) {
    if (!ts) return '—';
    const d = Date.now() - ts;
    if (d < 5000) return t('только что');
    if (d < 60000) return t('{n} с назад', { n: Math.round(d / 1000) });
    if (d < 3600000) return t('{n} мин назад', { n: Math.round(d / 60000) });
    if (d < 86400000) return t('{n} ч назад', { n: Math.round(d / 3600000) });
    return fmt.date(ts);
  },
  uptime(s) {
    if (s === null || s === undefined) return '—';
    const d = Math.floor(s / 86400);
    const hh = Math.floor((s % 86400) / 3600);
    const mm = Math.floor((s % 3600) / 60);
    if (d) return `${d} ${t('д')} ${hh} ${t('ч')}`;
    if (hh) return `${hh} ${t('ч')} ${mm} ${t('мин')}`;
    return `${mm} ${t('мин')}`;
  },
  mb(v) {
    if (v === null || v === undefined) return '—';
    return v >= 1024 ? (v / 1024).toLocaleString(locale(), { maximumFractionDigits: 1 }) + ' ' + t('ГБ') : v + ' ' + t('МБ');
  },
};

/** Подпись и вид состояния задания. */
export function jobState(s) {
  const map = {
    queued: [t('в очереди'), 'busy'],
    running: [t('выполняется'), 'run'],
    done: [t('готово'), 'run'],
    failed: [t('ошибка'), 'bad'],
    expired: [t('истекло'), 'off'],
    cancelled: [t('отменено'), 'off'],
    superseded: [t('вытеснено'), 'off'],
    dead: [t('не доставлено'), 'bad'],
    pending_delivery: [t('доставляется'), 'busy'],
  };
  const [label, kind] = map[s] || [s, ''];
  return { label, kind };
}

/** Глубокое чтение и запись по пути «a.b.c». */
export function getPath(obj, path) {
  return path.split('.').reduce((o, k) => (o == null ? undefined : o[k]), obj);
}
export function setPath(obj, path, value) {
  const keys = path.split('.');
  let o = obj;
  for (let i = 0; i < keys.length - 1; i++) {
    if (o[keys[i]] == null || typeof o[keys[i]] !== 'object') o[keys[i]] = {};
    o = o[keys[i]];
  }
  o[keys[keys.length - 1]] = value;
}

export function clone(v) {
  return v === undefined ? undefined : JSON.parse(JSON.stringify(v));
}
