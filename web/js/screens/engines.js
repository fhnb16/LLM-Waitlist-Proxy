import { api } from '../api.js';
import { t } from '../i18n.js';
import { h, clear, hero, group, list, cell, field, input, select, textarea, cellSwitch, btn, cstate, fmt, modal, toast, errorToast, confirmDialog, state, callout, clone, navBtn, meter } from '../ui.js';
import { store, on, saveSettings } from '../store.js';
import { logView } from './logview.js';

const KINDS = [
  { value: 'llama_cpp', label: 'llama.cpp (llama-server)' },
  { value: 'vllm', label: 'vLLM' },
  { value: 'ollama', label: 'Ollama' },
  { value: 'koboldcpp', label: 'KoboldCpp' },
  { value: 'custom', label: t('Свой бинарник') },
];
const DEFAULT_EXE = { llama_cpp: 'llama-server', vllm: 'vllm', ollama: 'ollama', koboldcpp: 'koboldcpp', custom: '' };
const DEFAULT_PORT = { llama_cpp: 8080, vllm: 8000, ollama: 11434, koboldcpp: 5001, custom: 8080 };
const STATUS = {
  ready: ['готов', 'run'],
  starting: ['запускается', 'busy'],
  stopping: ['останавливается', 'busy'],
  crashed: ['упал, перезапуск', 'bad'],
  failed: ['остановлен после сбоев', 'bad'],
  stopped: ['остановлен', 'off'],
};

function statusChip(s) {
  const [l, k] = STATUS[s] || [s, ''];
  return cstate(t(l), k);
}

async function control(id, action) {
  try {
    await api(`/admin/api/engines/${encodeURIComponent(id)}/${action}`, { method: 'POST' });
    toast({ start: t('Запуск…'), stop: t('Остановка…'), restart: t('Перезапуск…') }[action], 'ok');
  } catch (e) {
    errorToast(e);
  }
}

function controls(info) {
  const running = info && !['stopped', 'failed'].includes(info.status);
  return h('div.btn-row', { style: { padding: '0' } },
    running
      ? btn(t('Остановить'), { kind: 'danger', inline: true, onClick: () => control(info.id, 'stop') })
      : btn(t('Запустить'), { kind: 'filled', inline: true, onClick: () => control(info.id, 'start') }),
    running ? btn(t('Перезапустить'), { inline: true, onClick: () => control(info.id, 'restart') }) : null,
  );
}

export async function render(ctx) {
  if (ctx.parts[1]) return renderEngine(ctx, ctx.parts[1]);
  ctx.title(t('Движки'));
  ctx.actions(navBtn(t('Добавить'), () => addEngine(ctx), 'plus'));
  const root = ctx.root;
  const box = h('div');
  root.append(hero(t('Движки'), t('Локальные серверы моделей: LMQ запускает их, следит и перезапускает при падении.')), box);
  let infos = [];
  const load = async () => {
    try {
      infos = (await api('/admin/api/engines')).items;
    } catch (e) {
      clear(box).append(callout(e.message, 'error'));
      return;
    }
    paint();
  };
  const paint = () => {
    clear(box);
    if (!infos.length) {
      box.append(state(t('Движков нет'), t('Добавьте llama-server, vLLM, Ollama или любой свой бинарник — LMQ будет запускать его вместе с собой.'), btn(t('Добавить движок'), { kind: 'filled', inline: true, onClick: () => addEngine(ctx) })));
      return;
    }
    box.append(list(...infos.map((e) => cell({
      label: h('span.jobline',
        h('span.jobline__top', h('strong', e.name), statusChip(e.status)),
        h('span.jobline__meta',
          h('span', (KINDS.find((k) => k.value === e.kind) || {}).label || e.kind),
          h('span', e.endpoint.replace('/v1/chat/completions', '')),
          e.pid ? h('span', 'PID ' + e.pid) : null,
          e.started_at && e.status !== 'stopped' ? h('span', t('работает {t}', { t: fmt.uptime((Date.now() - e.started_at) / 1000) })) : null,
          e.restarts ? h('span', t('перезапусков: {n}', { n: e.restarts })) : null),
        !e.exe_resolved ? h('span.jobline__err', t('исполняемый файл не найден')) : null),
      onClick: () => ctx.go('#/engines/' + encodeURIComponent(e.id)),
    }))));
    box.append(h('p.footnote', t('Чтобы модель отправляла задания движку, выберите его в апстриме модели: адрес и слоты подставятся сами.')));
  };
  await load();
  ctx.onDestroy(on('engine', load));
  ctx.onDestroy(on('settings', load));
}

function addEngine(ctx) {
  const kind = select(KINDS, 'llama_cpp');
  const id = input('', { placeholder: 'llama-main' });
  const exe = input(DEFAULT_EXE.llama_cpp, { placeholder: 'C:\\llama\\llama-server.exe' });
  kind.addEventListener('change', () => { exe.value = DEFAULT_EXE[kind.value]; });
  modal({
    title: t('Новый движок'),
    body: h('div.list',
      field(t('Тип'), kind),
      field(t('Идентификатор'), id, { hint: t('Латиница, цифры, дефис. На него ссылаются апстримы моделей') }),
      field(t('Исполняемый файл'), exe, { hint: t('Полный путь или имя из PATH') })),
    actions: [
      { label: t('Отмена') },
      {
        label: t('Создать'),
        kind: 'filled',
        onClick: async () => {
          const v = id.value.trim();
          if (!/^[A-Za-z0-9_-]{1,40}$/.test(v)) throw new Error(t('Идентификатор: латиница, цифры, _ и -'));
          const file = clone(store.settings.file);
          if (file.engines.some((e) => e.id === v)) throw new Error(t('Движок {n} уже есть', { n: v }));
          const port = DEFAULT_PORT[kind.value];
          file.engines.push({
            id: v, name: v, kind: kind.value, exe: exe.value.trim(),
            llama: { model: '', host: '127.0.0.1', port, ctx_size: 8192, parallel: 1, n_gpu_layers: 99, threads: 0, chat_template: '', flash_attn: false, jinja: true, alias: '' },
            args: [], env: {}, cwd: '', autostart: false, restart_on_crash: true, max_restarts_per_hour: 5, health_url: '', ready_timeout: 600,
          });
          await saveSettings(file);
          ctx.go('#/engines/' + encodeURIComponent(v));
        },
      },
    ],
  });
}

async function renderEngine(ctx, id) {
  ctx.back('#/engines', t('Движки'));
  const root = ctx.root;
  const idx = store.settings.file.engines.findIndex((e) => e.id === id);
  if (idx < 0) {
    root.append(state(t('Движок не найден'), id));
    return;
  }
  const draft = clone(store.settings.file);
  const e = draft.engines[idx];
  ctx.title(e.name || id);
  let dirty = false;
  const saveBtn = navBtn(t('Сохранить'), save);
  ctx.actions(saveBtn);
  const touch = () => { dirty = true; };
  async function save() {
    try {
      await saveSettings(draft);
      dirty = false;
      toast(t('Сохранено. Изменения вступят в силу после перезапуска движка'), 'ok');
      loadInfo();
    } catch (er) {
      errorToast(er);
    }
  }

  const status = h('div');
  const gpuBox = h('div');
  const form = h('div');
  const usedBy = Object.entries(draft.models).filter(([, m]) => m.upstreams.some((u) => u.engine === id)).map(([n]) => n);

  const txt = (obj, key, attrs = {}) => {
    const el = input(obj[key], attrs);
    el.addEventListener('input', () => { obj[key] = attrs.type === 'number' ? (el.value === '' ? 0 : Number(el.value)) : el.value; touch(); });
    return el;
  };

  function paintForm() {
    clear(form);
    const kindSel = select(KINDS, e.kind);
    kindSel.addEventListener('change', () => { e.kind = kindSel.value; if (!e.exe) e.exe = DEFAULT_EXE[e.kind]; e.llama.port = DEFAULT_PORT[e.kind]; touch(); paintForm(); });
    const exeIn = txt(e, 'exe', { placeholder: 'C:\\llama\\llama-server.exe' });
    const detectOut = h('span.field__hint');
    const detect = btn(t('Проверить'), {
      inline: true,
      onClick: async () => {
        detectOut.textContent = t('Проверяем…');
        try {
          const r = await api('/admin/api/engines/detect', { method: 'POST', body: { exe: e.exe } });
          detectOut.textContent = r.ok ? `${r.path}\n${r.version}` : r.error;
          detectOut.style.whiteSpace = 'pre-wrap';
          detectOut.style.color = r.ok ? 'var(--text-2)' : 'var(--danger)';
        } catch (er) {
          detectOut.textContent = er.message;
        }
      },
    });
    detect.classList.add('field__btn');
    const l = e.llama;
    const llamaRows = [];
    const k = e.kind;
    if (k !== 'custom' && k !== 'ollama') llamaRows.push(field(t('Модель'), txt(l, 'model', { placeholder: k === 'vllm' ? 'Qwen/Qwen3-8B' : 'C:\\models\\qwen3-27b-q4_k_m.gguf' }), { hint: k === 'llama_cpp' ? '-m' : k === 'vllm' ? 'vllm serve <model>' : '--model' }));
    llamaRows.push(field(t('Адрес'), txt(l, 'host', { placeholder: '127.0.0.1' }), { hint: '--host' }));
    llamaRows.push(field(t('Порт'), txt(l, 'port', { type: 'number', min: '1', max: '65535' }), { hint: '--port' }));
    if (k === 'llama_cpp' || k === 'vllm' || k === 'koboldcpp') llamaRows.push(field(t('Контекст'), txt(l, 'ctx_size', { type: 'number', min: '0' }), { hint: k === 'vllm' ? '--max-model-len' : k === 'koboldcpp' ? '--contextsize' : '--ctx-size' }));
    if (k === 'llama_cpp' || k === 'ollama') llamaRows.push(field(t('Параллельно'), txt(l, 'parallel', { type: 'number', min: '1', max: '64' }), { hint: k === 'ollama' ? 'OLLAMA_NUM_PARALLEL' : t('--parallel, оно же число слотов апстрима') }));
    if (k === 'llama_cpp' || k === 'koboldcpp') llamaRows.push(field(t('Слоёв на GPU'), txt(l, 'n_gpu_layers', { type: 'number', min: '-1' }), { hint: t('-ngl; -1 — не передавать') }));
    if (k === 'llama_cpp') {
      llamaRows.push(field(t('Потоков'), txt(l, 'threads', { type: 'number', min: '0' }), { hint: t('--threads; 0 — по умолчанию') }));
      llamaRows.push(field(t('Шаблон чата'), txt(l, 'chat_template', { placeholder: 'chatml' }), { hint: '--chat-template' }));
      llamaRows.push(cellSwitch('--jinja', l.jinja, (v) => { l.jinja = v; touch(); }));
      llamaRows.push(cellSwitch('--flash-attn on', l.flash_attn, (v) => { l.flash_attn = v; touch(); }));
    }
    if (k === 'llama_cpp' || k === 'vllm') llamaRows.push(field(t('Имя модели'), txt(l, 'alias', { placeholder: t('необязательно') }), { hint: k === 'vllm' ? '--served-model-name' : '--alias' }));
    const args = textarea(e.args.join('\n'), { rows: 4, placeholder: k === 'custom' ? '--model\nC:\\m.gguf\n--port\n8080' : '--cache-type-k\nq8_0' });
    args.addEventListener('input', () => { e.args = args.value.split('\n').map((x) => x.trim()).filter(Boolean); touch(); });
    const env = textarea(Object.entries(e.env).map(([a, b]) => `${a}=${b}`).join('\n'), { rows: 3, placeholder: 'CUDA_VISIBLE_DEVICES=0' });
    env.addEventListener('input', () => {
      e.env = {};
      for (const line of env.value.split('\n')) {
        const i = line.indexOf('=');
        if (i > 0) e.env[line.slice(0, i).trim()] = line.slice(i + 1).trim();
      }
      touch();
    });
    form.append(
      group(t('Движок'), list(
        field(t('Название'), txt(e, 'name')),
        field(t('Тип'), kindSel),
        h('label.field', h('span.field__label', t('Исполняемый файл')), h('span.field__row', exeIn, detect), detectOut),
        field(t('Рабочая папка'), txt(e, 'cwd', { placeholder: t('папка исполняемого файла') })),
      )),
      llamaRows.length ? group(k === 'llama_cpp' ? t('Параметры llama.cpp') : t('Параметры'), list(...llamaRows)) : null,
      group(t('Дополнительные аргументы'), h('div.list', h('div.field.field--stack', h('span.field__label', t('по одному на строку')), args)),
        k === 'custom' ? t('Для своего бинарника — все аргументы. Укажите адрес проверки готовности ниже.') : t('Добавляются после аргументов формы.')),
      group(t('Окружение'), h('div.list', h('div.field.field--stack', h('span.field__label', t('КЛЮЧ=значение, по одному на строку')), env))),
      group(t('Запуск и перезапуск'), list(
        cellSwitch(t('Запускать вместе с LMQ'), e.autostart, (v) => { e.autostart = v; touch(); }),
        cellSwitch(t('Перезапускать при падении'), e.restart_on_crash, (v) => { e.restart_on_crash = v; touch(); }),
        field(t('Перезапусков в час'), txt(e, 'max_restarts_per_hour', { type: 'number', min: '0', max: '100' }), { hint: t('Больше — движок остаётся выключенным') }),
        field(t('Проверка готовности'), txt(e, 'health_url', { placeholder: t('по типу движка') }), { hint: t('Пока не ответит 200, задания ему не выдаются') }),
        field(t('Ждать готовности, с'), txt(e, 'ready_timeout', { type: 'number', min: '5' })),
      )),
      group(null, list(cell({ label: t('Удалить движок'), kind: 'danger', onClick: remove })), usedBy.length ? t('Используется моделями: {m}', { m: usedBy.join(', ') }) : null),
    );
  }

  let info = null;
  async function loadInfo() {
    try {
      info = (await api('/admin/api/engines')).items.find((x) => x.id === id);
    } catch (_) {
      info = null;
    }
    if (!ctx.alive()) return;
    clear(status);
    if (!info) {
      status.append(callout(t('Сохраните настройки, чтобы управлять движком.'), 'warn'));
      return;
    }
    status.append(
      hero(info.name, (KINDS.find((x) => x.value === info.kind) || {}).label || info.kind),
      group(t('Состояние'), list(
        cell({ label: t('Состояние'), extra: statusChip(info.status) }),
        cell({ label: 'PID', value: info.pid ? String(info.pid) : '—' }),
        cell({ label: t('Запущен'), value: info.started_at ? fmt.date(info.started_at) : '—' }),
        cell({ label: t('Готов'), value: info.ready_at ? fmt.date(info.ready_at) : '—' }),
        cell({ label: t('Перезапусков'), value: String(info.restarts) }),
        cell({ label: t('Последнее завершение'), value: info.last_exit || '—' }),
        cell({ label: t('Адрес для моделей'), value: info.endpoint, wrap: true }),
        cell({ label: t('Команда'), stack: true, value: h('code.job__cmd', info.command.join(' ')) }),
        !info.exe_resolved ? cell({ label: h('span', { style: { color: 'var(--danger)' } }, t('Исполняемый файл не найден: {f}', { f: info.command[0] || '—' })) }) : null,
      ), usedBy.length ? t('Модели: {m}', { m: usedBy.join(', ') }) : t('Ни одна модель пока не использует этот движок.')),
      h('div.actions', controls(info)),
    );
  }

  async function remove() {
    if (!(await confirmDialog(t('Удалить движок {n}?', { n: id }), usedBy.length ? t('Его используют модели: {m}. Их апстримы перестанут работать.', { m: usedBy.join(', ') }) : t('Процесс будет остановлен.'), { ok: t('Удалить'), danger: true }))) return;
    const file = clone(store.settings.file);
    file.engines = file.engines.filter((x) => x.id !== id);
    for (const m of Object.values(file.models)) for (const u of m.upstreams) if (u.engine === id) u.engine = '';
    try {
      await saveSettings(file);
      ctx.go('#/engines');
    } catch (er) {
      errorToast(er);
    }
  }

  function paintGpu() {
    clear(gpuBox);
    const g = store.overview && store.overview.gpu;
    if (!g || !g.available || !g.devices.length) return;
    gpuBox.append(group(t('Видеокарты'), h('div.meters', g.devices.flatMap((d) => [
      meter(`${d.name} · ${t('загрузка')}`, d.util + '%', d.util / 100),
      meter(t('память'), `${fmt.mb(d.mem_used_mb)} / ${fmt.mb(d.mem_total_mb)}`, d.mem_total_mb ? d.mem_used_mb / d.mem_total_mb : 0),
    ]))));
  }

  const logBox = h('div');
  root.append(status, gpuBox, h('h2.group__title', t('Вывод движка')), logBox, form);
  paintForm();
  await loadInfo();
  paintGpu();
  const lv = logView(logBox, {
    load: async () => (await api(`/admin/api/engines/${encodeURIComponent(id)}/logs?tail=2000`)).items.map(toLine),
    filename: `engine-${id}.log`,
    onClear: () => api(`/admin/api/engines/${encodeURIComponent(id)}/logs`, { method: 'DELETE' }),
  });
  ctx.onDestroy(on('engine_log', (ev) => { if (ev.id === id) lv.push(toLine(ev.line)); }));
  ctx.onDestroy(on('engine', (ev) => { if (ev.id === id) loadInfo(); }));
  ctx.onDestroy(on('tick', paintGpu));
  ctx.onDestroy(() => lv.destroy());
  ctx.onDestroy(() => {
    if (dirty) toast(t('Изменения движка не сохранены'), 'warn');
  });
}

function toLine(l) {
  return { ts: l.ts, level: l.stream === 'sys' ? 'sys' : l.level, src: l.stream, text: l.text };
}
