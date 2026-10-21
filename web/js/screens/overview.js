import { api } from '../api.js';
import { t, tn } from '../i18n.js';
import { lineChart, barChart, sparkline, bucketLabels } from '../charts.js';
import { h, clear, hero, group, list, cell, segmented, cstate, meter, fmt, navBtn, toast, errorToast, btn } from '../ui.js';
import { store, on } from '../store.js';

const RANGES = ['5m', '1h', '24h', '7d', '30d'];
let range = (() => {
  try {
    return localStorage.getItem('lmq.range') || '1h';
  } catch (_) {
    return '1h';
  }
})();

function tFormat(r) {
  return (ts) => {
    const d = new Date(ts);
    if (r === '7d' || r === '30d') return d.toLocaleDateString([], { day: '2-digit', month: '2-digit' }) + (r === '7d' ? ' ' + d.toLocaleTimeString([], { hour: '2-digit' }) : '');
    return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  };
}

function kpiTile(label, value, unit, sub, spark, bad) {
  return h('div.kpi__tile', { class: bad ? 'kpi__tile--bad' : null },
    h('span.kpi__label', label),
    h('span.kpi__value', value, unit ? h('span.kpi__unit', unit) : null),
    sub ? h('span.kpi__sub', sub) : null,
    spark ? sparkline(spark, 'kpi__spark') : null,
  );
}

export async function render(ctx) {
  ctx.title(t('Обзор'));
  const root = ctx.root;
  root.classList.add('app__screen--wide');
  const pauseBtn = navBtn(t('Пауза'), async () => {
    try {
      const paused = !(store.overview && store.overview.paused);
      await api('/admin/api/pause', { method: 'POST', body: { paused } });
      toast(paused ? t('Выдача заданий на паузе') : t('Выдача заданий возобновлена'), paused ? 'warn' : 'ok');
      store.overview.paused = paused;
      paint();
    } catch (e) {
      errorToast(e);
    }
  }, 'pause');
  ctx.actions(pauseBtn);

  const head = h('div');
  const banners = h('div');
  const kpi = h('div.kpi');
  const rangeSeg = segmented(RANGES.map((r) => ({ value: r, label: t(r) })), range, (v) => {
    range = v;
    try {
      localStorage.setItem('lmq.range', v);
    } catch (_) { /* без запоминания */ }
    loadStats();
  });
  const plotQueue = h('div');
  const plotJobs = h('div');
  const plotWait = h('div');
  const plotLat = h('div');
  const plotTps = h('div');
  const plotHist = h('div');
  const errorsBox = h('div');
  const models = h('div');
  const running = h('div');
  const engines = h('div');
  const gpu = h('div');
  root.append(
    head,
    banners,
    kpi,
    h('div.toolbar.toolbar--tight', h('span.toolbar__label', t('Период')), rangeSeg),
    h('div.plots', plotQueue, plotJobs, plotWait, plotLat, plotTps, plotHist),
    errorsBox,
    models,
    running,
    engines,
    gpu,
  );

  let series = null;
  const charts = [];
  ctx.onDestroy(() => charts.forEach((c) => c.destroy()));

  function paint() {
    const o = store.overview;
    if (!o) return;
    pauseBtn.title = o.paused ? t('Возобновить') : t('Пауза');
    pauseBtn.setAttribute('aria-label', pauseBtn.title);
    clear(pauseBtn).append(...(o.paused ? [h('span.nav-bar__btn-label', t('Возобновить'))] : [h('span.nav-bar__btn-label', t('Пауза'))]));
    clear(head).append(hero(t('Обзор'), [
      o.paused ? t('Выдача на паузе') : t('Работает'),
      fmt.uptime(o.uptime_s),
      t('профиль {p}', { p: o.profile }),
      'v' + o.version,
    ].join(' · ')));

    clear(banners);
    const b = o.bound || {};
    if (b.client_error) banners.append(h('div.banner.banner--danger', h('span.banner__title', t('API не слушает')), b.client_error));
    if (b.admin_error) banners.append(h('div.banner.banner--danger', h('span.banner__title', t('Панель не слушает')), b.admin_error));
    if (o.paused) banners.append(h('div.banner.banner--warn', h('span.banner__title', t('Выдача заданий на паузе')), t('Задания принимаются и ждут в очереди; MAX_WAIT продолжает действовать.')));
    if (!o.clients) banners.append(h('div.banner.banner--warn', h('span.banner__title', t('Нет клиентов API')), t('Бот получит 401, пока не создан ключ.'), h('div.banner__actions', btn(t('Создать ключ'), { inline: true, kind: 'filled', onClick: () => ctx.go('#/clients') }))));
    if (!o.models.length) banners.append(h('div.banner.banner--warn', h('span.banner__title', t('Нет моделей')), t('Добавьте модель и апстрим — например, llama.cpp на 127.0.0.1:8080.'), h('div.banner__actions', btn(t('Добавить модель'), { inline: true, kind: 'filled', onClick: () => ctx.go('#/models?new=1') }))));
    for (const r of (o.schedule && o.schedule.active) || []) {
      const what = { pause_intake: t('приём на паузе'), pause_dispatch: t('выдача на паузе'), limit_slots: t('не больше {n} одновременно', { n: r.slots }) }[r.action] || r.action;
      banners.append(h('div.banner', h('span.banner__title', t('Расписание: {name}', { name: r.name })), `${what}${r.models.length ? ' · ' + r.models.join(', ') : ''} · ${t('ещё {t}', { t: fmt.secs(r.ends_in) })}`));
    }
    const down = o.models.flatMap((m) => m.upstreams.filter((u) => u.health.breaker === 'open').map((u) => u.id));
    if (down.length) banners.append(h('div.banner.banner--danger', h('span.banner__title', t('Апстримы исключены выключателем')), down.join(', ')));

    // KPI
    const slots = o.models.reduce((a, m) => a + m.slots, 0);
    const h1 = o.last_1h || {};
    const pts = (series && series.points) || [];
    clear(kpi).append(
      kpiTile(t('В очереди'), fmt.num(o.queued), null, t('доставляется: {n}', { n: (o.counts && o.counts.pending_delivery) || 0 }), pts.map((p) => p.depth)),
      kpiTile(t('Выполняется'), fmt.num(o.running), slots ? '/ ' + slots : null, slots ? t('загрузка {p}', { p: fmt.pct(o.running / slots) }) : t('нет слотов'), pts.map((p) => p.running)),
      kpiTile(t('Заданий за час'), fmt.num(h1.jobs || 0), null, h1.jobs ? t('ошибок {p}', { p: fmt.pct(h1.error_rate || 0) }) : '—', pts.map((p) => p.jobs), (h1.error_rate || 0) > 0.2),
      kpiTile(t('Ожидание p95'), fmt.ms(h1.wait_p95), null, t('среднее {v}', { v: fmt.ms(h1.wait_avg) }), pts.map((p) => p.wait_p95)),
      kpiTile(t('Ответ p95'), fmt.ms(h1.lat_p95), null, t('среднее {v}', { v: fmt.ms(h1.lat_avg) }), pts.map((p) => p.lat_p95)),
      kpiTile(t('Токены/с'), fmt.num(h1.tps, 1), null, t('токенов за час: {n}', { n: fmt.num(h1.tokens || 0) }), pts.map((p) => p.tps)),
    );

    // Модели
    const modelRows = o.models.map((m) => {
      const broken = !m.available;
      const cells = [];
      for (let i = 0; i < m.slots; i++) cells.push(h('span.slots__cell', { class: i < m.busy ? 'slots__cell--busy' : null }));
      return cell({
        label: h('span', { style: { display: 'flex', alignItems: 'center', gap: '8px' } }, h('span.dot', { class: broken ? 'dot--bad' : m.running ? 'dot--run' : 'dot--ok' }), m.name),
        value: `${t('очередь')} ${m.queued} · ${m.running}/${m.slots}`,
        extra: h('span.slots', { 'aria-label': t('занято {b} из {s}', { b: m.busy, s: m.slots }) }, cells.slice(0, 16)),
        onClick: () => ctx.go('#/models/' + encodeURIComponent(m.name)),
      });
    });
    clear(models).append(group(t('Модели'), modelRows.length ? list(...modelRows) : list(cell({ label: t('Добавить модель'), kind: 'tint', onClick: () => ctx.go('#/models?new=1') }))));

    // Выполняются
    const rj = (o.running_jobs || []).slice().sort((a, b) => a.started_at - b.started_at);
    clear(running);
    if (rj.length) {
      running.append(group(tn(rj.length, 'задание выполняется', 'задания выполняются', 'заданий выполняются'), list(...rj.map((j) =>
        cell({
          label: h('span.jobline', h('span.jobline__top', h('span.jobline__id', j.sync ? t('синхронный запрос') : j.job_id)), h('span.jobline__meta', h('span', j.model), h('span', j.upstream), j.chat_id ? h('span', 'chat ' + j.chat_id) : null)),
          value: fmt.ms(Date.now() - j.started_at),
          onClick: j.sync ? null : () => ctx.go('#/queue/' + encodeURIComponent(j.job_id)),
        }),
      ))));
    }

    // Движки
    clear(engines);
    if ((o.engines || []).length) {
      const kind = { ready: 'run', starting: 'busy', stopping: 'busy', crashed: 'bad', failed: 'bad', stopped: 'off' };
      const label = { ready: t('готов'), starting: t('запускается'), stopping: t('останавливается'), crashed: t('упал, перезапуск'), failed: t('остановлен после сбоев'), stopped: t('остановлен') };
      engines.append(group(t('Движки'), list(...o.engines.map((e) => cell({ label: e.name, extra: cstate(label[e.status] || e.status, kind[e.status]), onClick: () => ctx.go('#/engines/' + encodeURIComponent(e.id)) })))));
    }

    // GPU
    clear(gpu);
    if (o.gpu && o.gpu.available && o.gpu.devices.length) {
      gpu.append(group(t('Видеокарты'), h('div.meters', o.gpu.devices.flatMap((d) => [
        meter(`${d.name} · ${t('загрузка')}`, d.util + '%', d.util / 100),
        meter(t('память'), `${fmt.mb(d.mem_used_mb)} / ${fmt.mb(d.mem_total_mb)}`, d.mem_total_mb ? d.mem_used_mb / d.mem_total_mb : 0, d.mem_total_mb && d.mem_used_mb / d.mem_total_mb > 0.92),
        d.temp_c != null ? meter(t('температура'), d.temp_c + ' °C' + (d.power_w != null ? ` · ${fmt.num(d.power_w)} ${t('Вт')}` : ''), d.temp_c / 100, d.temp_c >= 83) : null,
      ].filter(Boolean)))));
    }
  }

  async function loadStats() {
    try {
      series = await api('/admin/api/stats?range=' + range);
    } catch (e) {
      errorToast(e);
      return;
    }
    if (!ctx.alive()) return;
    const tf = tFormat(range);
    const pts = series.points;
    const common = { tFormat: tf, tipTitle: (ts) => new Date(ts).toLocaleString([], { day: '2-digit', month: '2-digit', hour: '2-digit', minute: '2-digit' }) };
    const defs = [
      [plotQueue, lineChart, { title: t('Очередь'), points: pts, series: [{ key: 'depth', label: t('в очереди'), color: 'var(--tint)', area: true }, { key: 'running', label: t('выполняется'), color: 'var(--warn)', dash: true }], yFormat: (v) => fmt.num(v), integer: true }],
      [plotJobs, barChart, { title: t('Задания'), points: pts, series: [{ key: 'ok', label: t('успешно'), color: 'var(--ok)' }, { key: 'errors', label: t('ошибки'), color: 'var(--danger)' }], yFormat: (v) => fmt.num(v) }],
      [plotWait, lineChart, { title: t('Время ожидания'), points: pts, series: [{ key: 'wait_p50', label: 'p50', color: 'var(--tint)' }, { key: 'wait_p95', label: 'p95', color: 'var(--warn)', dash: true }], yFormat: fmt.ms }],
      [plotLat, lineChart, { title: t('Время ответа модели'), points: pts, series: [{ key: 'lat_p50', label: 'p50', color: 'var(--tint)' }, { key: 'lat_p95', label: 'p95', color: 'var(--warn)', dash: true }], yFormat: fmt.ms }],
      [plotTps, lineChart, { title: t('Скорость генерации, токенов/с'), points: pts, series: [{ key: 'tps', label: t('токенов/с'), color: 'var(--tint)', area: true }], yFormat: (v) => fmt.num(v, 1) }],
    ];
    if (!charts.length) {
      for (const [el, fn, opts] of defs) charts.push(fn(el, { ...common, ...opts }));
      const labels = bucketLabels(series.buckets_ms);
      charts.push(barChart(plotHist, { title: t('Распределение ожидания'), labels, points: series.wait_hist.map((n, i) => ({ t: i, wait: n })), series: [{ key: 'wait', label: t('заданий'), color: 'var(--tint)' }], yFormat: (v) => fmt.num(v), note: t('за выбранный период') }));
    } else {
      defs.forEach(([, , opts], i) => charts[i].update({ ...common, ...opts }));
      charts[5].update({ points: series.wait_hist.map((n, i) => ({ t: i, wait: n })), labels: bucketLabels(series.buckets_ms) });
    }
    // Ошибки по кодам — горизонтальные полосы каркаса.
    clear(errorsBox);
    const errs = Object.entries(series.errors || {}).sort((a, b) => b[1] - a[1]);
    if (errs.length) {
      const max = errs[0][1];
      errorsBox.append(group(t('Ошибки по кодам'), h('div.chart', errs.map(([code, n]) =>
        h('div.chart__row', h('span.chart__label', code), h('span.chart__track', h('span.chart__bar', { style: { width: ((n / max) * 100).toFixed(1) + '%', background: 'var(--danger)' } })), h('span.chart__value', fmt.num(n)))))));
    }
    paint();
  }

  paint();
  await loadStats();
  const timer = setInterval(() => loadStats(), range === '5m' ? 10000 : 30000);
  ctx.onDestroy(() => clearInterval(timer));
  ctx.onDestroy(on('tick', paint));
}
