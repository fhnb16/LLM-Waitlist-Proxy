// Графики на SVG и токенах темы. Одна ось на график; линии 2px; у столбиков
// скругление 4px только на свободном конце и зазор 2px между сегментами;
// перекрестие и подсказка при наведении; для 2+ серий — легенда, а серии
// различаются и начертанием (пунктир), не только цветом; таблица — по кнопке.

import { h, svg, clear } from './ui.js';
import { t } from './i18n.js';

function niceMax(v) {
  if (!(v > 0)) return 1;
  const p = Math.pow(10, Math.floor(Math.log10(v)));
  const n = v / p;
  const step = n <= 1 ? 1 : n <= 2 ? 2 : n <= 2.5 ? 2.5 : n <= 5 ? 5 : 10;
  return step * p;
}

function legend(series, kind) {
  if (series.length < 2) return null;
  return h('div.plot__legend', series.map((s) =>
    h('span.plot__key', { style: { color: s.color } },
      h('span.plot__swatch', { class: kind === 'bar' ? 'plot__swatch--bar' : s.dash ? 'plot__swatch--dash' : null }),
      h('span', { style: { color: 'var(--text-2)' } }, s.label)),
  ));
}

function dataTable(points, series, tFmt, yFmt) {
  return h('div.dtable__wrap', h('table.dtable',
    h('thead', h('tr', h('th', t('Время')), series.map((s) => h('th', s.label)))),
    h('tbody', points.slice().reverse().map((p) => h('tr', h('td', tFmt(p.t)), series.map((s) => h('td', p[s.key] == null ? '—' : yFmt(p[s.key])))))),
  ));
}

/** Каркас графика: заголовок, легенда, место под SVG, переключатель «Таблица». */
function frame(el, opts, kind, draw) {
  clear(el);
  el.classList.add('plot');
  const body = h('div');
  let showTable = false;
  const toggle = h('button.plot__toggle', { type: 'button' }, t('Таблица'));
  const tip = h('div.plot__tip', { hidden: true });
  el.append(h('div.plot__head', h('h3.plot__title', opts.title), legend(opts.series, kind)), body, tip, h('div.plot__foot', h('span', opts.note || ''), toggle));
  const render = () => {
    clear(body);
    tip.hidden = true;
    if (showTable) {
      body.append(dataTable(opts.points, opts.series, opts.tFormat || defaultT, opts.yFormat || String));
      toggle.textContent = t('График');
      return;
    }
    toggle.textContent = t('Таблица');
    const hasData = opts.points.some((p) => opts.series.some((s) => p[s.key] != null && p[s.key] !== 0));
    if (!hasData) {
      body.append(h('div.plot__empty', opts.empty || t('Пока нет данных за этот период')));
      return;
    }
    draw(body, tip, el);
  };
  toggle.addEventListener('click', () => {
    showTable = !showTable;
    render();
  });
  let lastW = 0;
  const ro = new ResizeObserver(() => {
    const w = el.clientWidth;
    if (Math.abs(w - lastW) > 4) {
      lastW = w;
      render();
    }
  });
  ro.observe(el);
  render();
  return { update(o) { Object.assign(opts, o); render(); }, destroy() { ro.disconnect(); } };
}

function defaultT(ts) {
  return new Date(ts).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

function geometry(el, height, n) {
  const W = Math.max(240, el.clientWidth - 28);
  const H = height;
  const m = { l: 44, r: 8, t: 8, b: 20 };
  const iw = W - m.l - m.r;
  const ih = H - m.t - m.b;
  const step = n > 1 ? iw / (n - 1) : iw;
  return { W, H, m, iw, ih, step };
}

/** Верх шкалы и шаг делений: шаг «красивый» (1, 2, 2,5, 5 × 10ⁿ), для счётчиков — целый. */
function scale(rawMax, integer) {
  let step = niceMax((rawMax || 1) / 4);
  if (integer) step = Math.max(1, Math.ceil(step));
  const top = Math.max(step, Math.ceil((rawMax || 1) / step - 1e-9) * step);
  return { top, step };
}

function yAxis(g, sc, yFmt) {
  const nodes = [];
  const n = Math.round(sc.top / sc.step);
  for (let i = 0; i <= n; i++) {
    const v = sc.step * i;
    const y = g.m.t + g.ih - (g.ih * v) / sc.top;
    nodes.push(svg('line', { class: 'plot__grid', x1: g.m.l, x2: g.W - g.m.r, y1: y, y2: y }));
    nodes.push(svg('text', { class: 'plot__axis', x: g.m.l - 6, y: y + 3, 'text-anchor': 'end' }, yFmt(v)));
  }
  return nodes;
}

function xLabels(g, points, tFmt, xAt) {
  const nodes = [];
  const n = points.length;
  const every = Math.max(1, Math.ceil(n / Math.max(2, Math.floor(g.iw / 70))));
  for (let i = 0; i < n; i += every) {
    nodes.push(svg('text', { class: 'plot__axis', x: xAt(i), y: g.H - 4, 'text-anchor': 'middle' }, tFmt(points[i].t, i)));
  }
  return nodes;
}

function placeTip(tip, el, x, rows, title) {
  clear(tip);
  tip.append(h('div.plot__tip-time', title));
  for (const r of rows) {
    tip.append(h('div.plot__tip-row', h('span.plot__swatch', { class: r.dash ? 'plot__swatch--dash' : r.bar ? 'plot__swatch--bar' : null, style: { color: r.color } }), h('span', r.label), h('span.plot__tip-val', r.value)));
  }
  tip.hidden = false;
  const w = tip.offsetWidth;
  const left = Math.min(Math.max(8, x + 14), el.clientWidth - w - 8);
  tip.style.left = (x + 14 + w > el.clientWidth ? Math.max(8, x - w - 14) : left) + 'px';
  tip.style.top = '36px';
}

/**
 * Линейный график. opts: title, series [{key,label,color,dash,area}], points
 * [{t, key: value}], yFormat, tFormat, height, yMax.
 */
export function lineChart(el, opts) {
  return frame(el, opts, 'line', (body, tip) => {
    const pts = opts.points;
    const g = geometry(el, opts.height || 160, pts.length);
    const yFmt = opts.yFormat || ((v) => String(Math.round(v)));
    const tFmt = opts.tFormat || defaultT;
    let raw = opts.yMax || 0;
    for (const p of pts) for (const s of opts.series) if (p[s.key] != null) raw = Math.max(raw, p[s.key]);
    const sc = scale(raw, opts.integer);
    const max = sc.top;
    const xAt = (i) => g.m.l + (pts.length > 1 ? i * g.step : g.iw / 2);
    const yAt = (v) => g.m.t + g.ih - (v / max) * g.ih;
    const root = svg('svg', { class: 'plot__svg', viewBox: `0 0 ${g.W} ${g.H}`, height: g.H, role: 'img', 'aria-label': opts.title });
    root.append(...yAxis(g, sc, yFmt), ...xLabels(g, pts, tFmt, xAt));
    for (const s of opts.series) {
      let d = '';
      let area = '';
      let open = false;
      let segStart = 0;
      pts.forEach((p, i) => {
        const v = p[s.key];
        if (v == null) {
          if (open && s.area) area += `L${xAt(i - 1)},${yAt(0)}L${xAt(segStart)},${yAt(0)}Z`;
          open = false;
          return;
        }
        const cmd = open ? 'L' : 'M';
        if (!open) segStart = i;
        d += `${cmd}${xAt(i).toFixed(1)},${yAt(v).toFixed(1)}`;
        if (s.area) area += `${cmd}${xAt(i).toFixed(1)},${yAt(v).toFixed(1)}`;
        open = true;
      });
      if (open && s.area) area += `L${xAt(pts.length - 1)},${yAt(0)}L${xAt(segStart)},${yAt(0)}Z`;
      if (s.area && area) root.append(svg('path', { class: 'plot__area', d: area, style: `fill:${s.color}` }));
      // Одиночная точка без соседей — кружок, иначе её не видно.
      const single = pts.filter((p) => p[s.key] != null).length === 1;
      if (single) {
        const i = pts.findIndex((p) => p[s.key] != null);
        root.append(svg('circle', { cx: xAt(i), cy: yAt(pts[i][s.key]), r: 4, style: `fill:${s.color}` }));
      }
      root.append(svg('path', { class: 'plot__line' + (s.dash ? ' plot__line--dash' : ''), d, style: `stroke:${s.color}` }));
    }
    const cross = svg('line', { class: 'plot__cross', y1: g.m.t, y2: g.m.t + g.ih, visibility: 'hidden' });
    const dots = opts.series.map((s) => svg('circle', { class: 'plot__dot', r: 4, visibility: 'hidden', style: `fill:${s.color}` }));
    root.append(cross, ...dots);
    const hit = svg('rect', { class: 'plot__hit', x: g.m.l, y: g.m.t, width: g.iw, height: g.ih });
    const onMove = (ev) => {
      const box = root.getBoundingClientRect();
      const x = ((ev.clientX - box.left) / box.width) * g.W;
      const i = Math.max(0, Math.min(pts.length - 1, Math.round((x - g.m.l) / (g.step || 1))));
      const p = pts[i];
      cross.setAttribute('x1', xAt(i));
      cross.setAttribute('x2', xAt(i));
      cross.setAttribute('visibility', 'visible');
      opts.series.forEach((s, k) => {
        const v = p[s.key];
        if (v == null) dots[k].setAttribute('visibility', 'hidden');
        else {
          dots[k].setAttribute('cx', xAt(i));
          dots[k].setAttribute('cy', yAt(v));
          dots[k].setAttribute('visibility', 'visible');
        }
      });
      placeTip(tip, el, (xAt(i) / g.W) * box.width, opts.series.map((s) => ({ label: s.label, color: s.color, dash: s.dash, value: p[s.key] == null ? '—' : yFmt(p[s.key]) })), (opts.tipTitle || tFmt)(p.t, i));
    };
    const onLeave = () => {
      cross.setAttribute('visibility', 'hidden');
      dots.forEach((d) => d.setAttribute('visibility', 'hidden'));
      tip.hidden = true;
    };
    hit.addEventListener('pointermove', onMove);
    hit.addEventListener('pointerdown', onMove);
    hit.addEventListener('pointerleave', onLeave);
    root.append(hit);
    body.append(root);
  });
}

function barPath(x, y, w, hgt, rTop) {
  const r = Math.min(rTop, w / 2, hgt);
  if (r <= 0) return `M${x},${y}h${w}v${hgt}h${-w}Z`;
  return `M${x},${y + hgt}V${y + r}Q${x},${y} ${x + r},${y}H${x + w - r}Q${x + w},${y} ${x + w},${y + r}V${y + hgt}Z`;
}

/**
 * Столбики (со стопкой серий). opts: title, series [{key,label,color}], points,
 * labels (подписи по оси X вместо времени), yFormat, tFormat, height.
 */
export function barChart(el, opts) {
  return frame(el, opts, 'bar', (body, tip) => {
    const pts = opts.points;
    const n = pts.length;
    const g = geometry(el, opts.height || 150, n);
    const yFmt = opts.yFormat || ((v) => String(Math.round(v)));
    const tFmt = opts.labels ? (_t, i) => opts.labels[i] : opts.tFormat || defaultT;
    let raw = 0;
    for (const p of pts) raw = Math.max(raw, opts.series.reduce((a, s) => a + (p[s.key] || 0), 0));
    const sc = scale(raw, opts.integer !== false);
    const max = sc.top;
    const slot = g.iw / n;
    const bw = Math.max(2, Math.min(28, slot - 2));
    const xAt = (i) => g.m.l + slot * i + slot / 2;
    const root = svg('svg', { class: 'plot__svg', viewBox: `0 0 ${g.W} ${g.H}`, height: g.H, role: 'img', 'aria-label': opts.title });
    root.append(...yAxis(g, sc, yFmt), ...xLabels(g, pts, tFmt, xAt));
    pts.forEach((p, i) => {
      let acc = 0;
      const nonzero = opts.series.filter((s) => (p[s.key] || 0) > 0);
      nonzero.forEach((s, k) => {
        const v = p[s.key];
        const y0 = g.m.t + g.ih - (acc / max) * g.ih;
        const hh = (v / max) * g.ih;
        const top = k === nonzero.length - 1;
        const gap = k > 0 ? 2 : 0;
        root.append(svg('path', { class: 'plot__bar', d: barPath(xAt(i) - bw / 2, y0 - hh, bw, Math.max(1, hh - gap), top ? 4 : 0), style: `fill:${s.color}` }));
        acc += v;
      });
    });
    const hit = svg('rect', { class: 'plot__hit', x: g.m.l, y: g.m.t, width: g.iw, height: g.ih });
    const hl = svg('rect', { y: g.m.t, height: g.ih, width: slot, style: 'fill:var(--text);opacity:0.05', visibility: 'hidden' });
    root.insertBefore(hl, root.firstChild);
    const onMove = (ev) => {
      const box = root.getBoundingClientRect();
      const x = ((ev.clientX - box.left) / box.width) * g.W;
      const i = Math.max(0, Math.min(n - 1, Math.floor((x - g.m.l) / slot)));
      hl.setAttribute('x', g.m.l + slot * i);
      hl.setAttribute('visibility', 'visible');
      const p = pts[i];
      const rows = opts.series.map((s) => ({ label: s.label, color: s.color, bar: true, value: yFmt(p[s.key] || 0) }));
      placeTip(tip, el, (xAt(i) / g.W) * box.width, rows, opts.labels ? opts.labels[i] : (opts.tipTitle || tFmt)(p.t, i));
    };
    hit.addEventListener('pointermove', onMove);
    hit.addEventListener('pointerdown', onMove);
    hit.addEventListener('pointerleave', () => {
      hl.setAttribute('visibility', 'hidden');
      tip.hidden = true;
    });
    root.append(hit);
    body.append(root);
  });
}

/** Спарклайн для карточек и KPI. */
export function sparkline(values, cls = 'spark') {
  const W = 100;
  const H = 24;
  const vals = values.map((v) => (v == null ? null : v));
  const max = Math.max(1e-9, ...vals.filter((v) => v != null));
  const n = vals.length;
  const x = (i) => (n > 1 ? (i / (n - 1)) * W : W / 2);
  const y = (v) => H - 2 - (v / max) * (H - 4);
  let d = '';
  let open = false;
  vals.forEach((v, i) => {
    if (v == null) {
      open = false;
      return;
    }
    d += `${open ? 'L' : 'M'}${x(i).toFixed(1)},${y(v).toFixed(1)}`;
    open = true;
  });
  const area = d ? `${d}L${x(n - 1)},${H}L${x(vals.findIndex((v) => v != null))},${H}Z` : '';
  return svg('svg', { class: cls, viewBox: `0 0 ${W} ${H}`, preserveAspectRatio: 'none', 'aria-hidden': 'true' },
    area ? svg('path', { class: 'spark__area', d: area }) : null,
    d ? svg('path', { class: 'spark__line', d, 'vector-effect': 'non-scaling-stroke' }) : null,
  );
}

/** Подписи корзин гистограммы ожидания/ответа. */
export function bucketLabels(bounds) {
  const f = (ms) => (ms < 1000 ? ms + ' ' + t('мс') : ms < 60000 ? ms / 1000 + ' ' + t('с') : ms / 60000 + ' ' + t('мин'));
  return [...bounds.map((b) => '≤' + f(b)), '>' + f(bounds[bounds.length - 1])];
}
