import { api, desktop, desktopCall } from '../api.js';
import { t } from '../i18n.js';
import { h, clear, hero, group, list, cell, field, input, cellSwitch, segmented, btn, fmt, toast, errorToast, clone, callout, navBtn, confirmDialog, ring } from '../ui.js';
import { store, saveSettings } from '../store.js';

/** Строка состояния: точка (ok — готово к установке, error — ошибка) или кольцо, пока идёт работа. */
function statusLine(el, s) {
  clear(el);
  el.className = 'probe';
  const latest = s.info && s.info.latest;
  if (s.checking || s.downloading) {
    el.append(ring(), h('span.probe__text', s.checking ? t('Проверяем…') : t('Скачиваем версию {v} и проверяем подпись', { v: latest || '' })));
    return;
  }
  let main = null;
  let sub = null;
  if (s.ready) {
    el.classList.add('probe--ok');
    main = t('Версия {v} скачана и проверена', { v: s.ready.version });
    sub = s.ready.authenticode_text;
  } else if (s.error) {
    el.classList.add('probe--error');
    main = s.error;
  } else if (s.info && s.info.newer) {
    main = s.info.can_install ? t('Доступна версия {v}', { v: latest }) : t('Доступна версия {v}, установщика с подписью нет', { v: latest });
  } else if (s.info) {
    el.classList.add('probe--ok');
    main = t('Установлена последняя версия');
  }
  if (!main) return;
  const checked = s.checked_at ? t('Проверено {d}', { d: fmt.date(s.checked_at) }) : null;
  el.append(h('span.probe__dot', { 'aria-hidden': 'true' }), h('span.probe__text', h('span.probe__main', main), sub || checked ? h('span.probe__url', [sub, checked].filter(Boolean).join(' · ')) : null));
}

/** Запуск скачанного установщика: без действительной подписи Authenticode — только с согласия. */
async function install(r) {
  let confirmed = false;
  if (r.verdict === 'confirm') {
    const sig = r.authenticode || {};
    const why = sig.status === 'untrusted' ? t('Windows не доверяет сертификату «{p}»', { p: sig.publisher || '—' }) : t('Подписи Authenticode нет');
    confirmed = await confirmDialog(
      t('Установщик без подписи Windows'),
      t('{why}. Подпись minisign сошлась — файл из релиза LMQ, но Defender и SmartScreen могут предупредить. Установить?', { why }),
      { ok: t('Установить') });
    if (!confirmed) return;
  }
  toast(t('LMQ закроется, установит версию {v} и запустится снова', { v: r.version }), 'ok');
  await desktopCall('run_installer', { path: r.path, confirmed });
}

export async function render(ctx) {
  ctx.title(t('Обновления'));
  const root = ctx.root;
  const u = clone(store.settings.file.updates);
  const saveBtn = navBtn(t('Сохранить'), async () => {
    try {
      const file = clone(store.settings.file);
      file.updates = u;
      await saveSettings(file);
      toast(t('Сохранено'), 'ok');
    } catch (e) {
      errorToast(e);
    }
  });
  ctx.actions(saveBtn);
  const repo = input(u.repo, { placeholder: 'fhnb16/LLM-Waitlist-Proxy' });
  repo.addEventListener('input', () => { u.repo = repo.value.trim(); });
  const pubkey = input(u.pubkey, { placeholder: 'RWQ…' });
  pubkey.addEventListener('input', () => { u.pubkey = pubkey.value.trim(); });
  const status = h('div.probe');
  const out = h('div');
  let timer = null;

  function show(s) {
    statusLine(status, s);
    clear(out);
    const i = s.info;
    if (i && !i.error) {
      const rows = [
        cell({ label: t('Установлена'), value: i.current }),
        cell({ label: t('Последняя'), value: (i.latest || '—') + (i.prerelease ? ' (beta)' : '') }),
        i.published_at ? cell({ label: t('Выпущена'), value: fmt.date(Date.parse(i.published_at)) }) : null,
      ];
      out.append(group(i.newer ? t('Доступна новая версия') : t('Установлена последняя версия'), list(...rows)));
      if (i.newer && i.notes) out.append(group(t('Что нового'), h('div.list', h('div.list__cell.list__cell--stack', h('div.rich', { style: { whiteSpace: 'pre-wrap', fontSize: '14px' } }, i.notes)))));
    }
    const acts = h('div.actions');
    if (s.ready && desktop) {
      acts.append(btn(t('Установить и перезапустить'), { kind: 'filled', onClick: () => install(s.ready).catch(errorToast) }));
    } else if (i && i.newer && i.can_install && !s.downloading) {
      acts.append(btn(t('Скачать'), { kind: 'filled', onClick: download }));
    }
    acts.append(btn(t('Проверить сейчас'), { kind: s.ready || (i && i.newer) ? null : 'filled', onClick: check, disabled: s.checking || s.downloading }));
    if (i && i.url && i.newer) acts.append(btn(t('Страница релиза'), { onClick: () => (desktop ? desktopCall('open_url', { url: i.url }) : window.open(i.url, '_blank', 'noopener')) }));
    out.append(acts);
    if (s.ready && !desktop) out.append(callout(t('Установщик скачан на машину с LMQ. Поставить его можно из окна LMQ'), 'ok'));
    if (i && i.newer && !u.pubkey) out.append(callout(t('Для установки задайте публичный ключ minisign: без проверки подписи файл не устанавливается'), 'warn'));
    // Пока идёт проверка или загрузка — следим за состоянием.
    clearTimeout(timer);
    if ((s.checking || s.downloading) && root.isConnected) timer = setTimeout(load, 1500);
  }

  async function load() {
    try {
      show(await api('/admin/api/updates/state'));
    } catch (e) {
      clear(out).append(callout(e.message, 'error'));
    }
  }

  async function download() {
    const poll = setTimeout(load, 300);
    try {
      await api('/admin/api/updates/download', { method: 'POST' });
    } catch (e) {
      errorToast(e);
    }
    clearTimeout(poll);
    await load();
  }

  async function check() {
    statusLine(status, { checking: true });
    try {
      const i = await api('/admin/api/updates');
      const s = await api('/admin/api/updates/state');
      if (i.newer && i.can_install && !s.ready) return download();
      show(s);
    } catch (e) {
      errorToast(e);
      await load();
    }
  }

  root.append(
    hero(t('Обновления'), t('Через GitHub Releases. Установщик проверяется подписью minisign.')),
    status,
    out,
    group(t('Автоматически'), h('div.list',
      cellSwitch(t('Проверять обновления'), u.auto_check, (v) => { u.auto_check = v; }, { sub: t('При запуске и раз в 6 часов') }),
      cellSwitch(t('Скачивать новую версию'), u.auto_download, (v) => { u.auto_download = v; }, { sub: t('В фоне, с проверкой подписи') }),
      cellSwitch(t('Ставить обновление само'), u.auto_install, (v) => { u.auto_install = v; }, { sub: t('Когда очередь пуста, и только с подписью Windows. LMQ перезапустится в трее') }),
    )),
    group(t('Канал'), segmented([{ value: 'stable', label: t('Стабильный') }, { value: 'beta', label: 'Beta' }], u.channel, (v) => { u.channel = v; })),
    group(t('Источник'), h('div.list',
      field(t('Репозиторий'), repo, { hint: t('owner/name на GitHub') }),
      field(t('Ключ подписи'), pubkey, { hint: t('Публичный ключ minisign (строка RW…)') }),
    )),
  );
  await load();
}
