import { api, desktop, desktopCall } from '../api.js';
import { t } from '../i18n.js';
import { h, clear, hero, group, list, cell, field, input, cellSwitch, segmented, btn, fmt, toast, errorToast, clone, callout, navBtn } from '../ui.js';
import { store, saveSettings } from '../store.js';

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
  const repo = input(u.repo, { placeholder: 'owner/LLM-Waitlist-Proxy' });
  repo.addEventListener('input', () => { u.repo = repo.value.trim(); });
  const pubkey = input(u.pubkey, { placeholder: 'RWQ…' });
  pubkey.addEventListener('input', () => { u.pubkey = pubkey.value.trim(); });
  const out = h('div');

  async function check() {
    clear(out).append(h('div.summary__wait', h('span.ring'), t('Проверяем…')));
    try {
      const i = await api('/admin/api/updates');
      clear(out);
      if (i.error) {
        out.append(callout(i.error, 'warn'));
        return;
      }
      const rows = [
        cell({ label: t('Установлена'), value: i.current }),
        cell({ label: t('Последняя'), value: (i.latest || '—') + (i.prerelease ? ' (beta)' : '') }),
        i.published_at ? cell({ label: t('Выпущена'), value: fmt.date(Date.parse(i.published_at)) }) : null,
      ];
      out.append(group(i.newer ? t('Доступна новая версия') : t('Установлена последняя версия'), list(...rows)));
      if (i.notes) out.append(group(t('Что нового'), h('div.list', h('div.list__cell.list__cell--stack', h('div.rich', { style: { whiteSpace: 'pre-wrap', fontSize: '14px' } }, i.notes)))));
      const acts = h('div.actions');
      if (i.url) acts.append(btn(t('Открыть страницу релиза'), { onClick: () => (desktop ? desktopCall('open_url', { url: i.url }) : window.open(i.url, '_blank', 'noopener')) }));
      if (i.newer && i.can_install && desktop) {
        acts.append(btn(t('Скачать и установить'), {
          kind: 'filled',
          onClick: async () => {
            try {
              toast(t('Загрузка и проверка подписи…'), 'ok');
              const r = await api('/admin/api/updates/download', { method: 'POST' });
              await desktopCall('run_installer', { path: r.path });
            } catch (e) {
              errorToast(e);
            }
          },
        }));
      } else if (i.newer && !u.pubkey) {
        acts.append(callout(t('Для установки из панели задайте публичный ключ minisign: без проверки подписи файл не устанавливается.'), 'warn'));
      }
      out.append(acts);
    } catch (e) {
      clear(out).append(callout(e.message, 'error'));
    }
  }

  root.append(
    hero(t('Обновления'), t('Через GitHub Releases. Установщик проверяется подписью minisign.')),
    group(t('Источник'), h('div.list',
      field(t('Репозиторий'), repo, { hint: t('owner/name на GitHub') }),
      field(t('Ключ подписи'), pubkey, { hint: t('Публичный ключ minisign (строка RW…)') }),
      cellSwitch(t('Проверять при запуске'), u.auto_check, (v) => { u.auto_check = v; }),
    )),
    group(t('Канал'), segmented([{ value: 'stable', label: t('Стабильный') }, { value: 'beta', label: 'Beta' }], u.channel, (v) => { u.channel = v; })),
    h('div.actions', btn(t('Проверить сейчас'), { kind: 'filled', onClick: check })),
    out,
  );
}
