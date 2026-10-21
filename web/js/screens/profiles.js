import { api } from '../api.js';
import { t } from '../i18n.js';
import { h, clear, hero, group, list, cell, field, input, toast, errorToast, confirmDialog, modal, callout, download, cstate } from '../ui.js';
import { reloadSettings } from '../store.js';

export async function render(ctx) {
  ctx.title(t('Профили'));
  const root = ctx.root;
  const box = h('div');
  root.append(hero(t('Профили, экспорт и импорт'), t('Профиль — отдельный набор настроек: например, «дом» и «сервер».')), box);

  async function load() {
    let d;
    try {
      d = await api('/admin/api/profiles');
    } catch (e) {
      clear(box).append(callout(e.message, 'error'));
      return;
    }
    if (!ctx.alive()) return;
    clear(box);
    box.append(
      group(t('Профили'), list(
        ...d.items.map((p) => cell({
          label: h('span.jobline', h('span.jobline__top', h('strong', p.name), p.active ? cstate(t('активный'), 'run') : null), h('span.jobline__meta', p.path)),
          onClick: p.active ? null : () => menu(p),
        })),
        cell({ label: t('Новый профиль из текущих настроек'), kind: 'tint', onClick: create }),
      ), t('Переключение применяется сразу, без перезапуска. Задания и статистика общие.')),
      group(t('Экспорт'), list(
        cell({ label: t('Скачать без секретов'), kind: 'tint', onClick: () => exportSettings(null) }),
        cell({ label: t('Скачать с секретами под паролем'), kind: 'tint', onClick: exportWithPassword }),
      ), t('Секреты (ключи апстримов, токены) шифруются паролем Argon2id + ChaCha20-Poly1305. Ключи клиентов хранятся хэшами и переносятся как есть.')),
      group(t('Импорт'), list(cell({ label: t('Загрузить файл настроек'), kind: 'tint', onClick: importSettings }))),
    );
  }

  function create() {
    const name = input('', { placeholder: 'server' });
    modal({
      title: t('Новый профиль'),
      body: h('div.list', field(t('Имя'), name)),
      actions: [{ label: t('Отмена') }, { label: t('Создать'), kind: 'filled', onClick: async () => { await api('/admin/api/profiles', { method: 'POST', body: { name: name.value.trim() } }); toast(t('Профиль создан'), 'ok'); load(); } }],
    });
  }

  function menu(p) {
    const m = modal({
      title: p.name,
      body: h('div.sheet',
        h('button.sheet__item', { type: 'button', on: { click: async () => {
          m.close();
          try {
            await api(`/admin/api/profiles/${encodeURIComponent(p.name)}/activate`, { method: 'POST' });
            await reloadSettings();
            toast(t('Активен профиль {p}', { p: p.name }), 'ok');
            load();
          } catch (e) { errorToast(e); }
        } } }, t('Сделать активным')),
        h('button.sheet__item.sheet__item--danger', { type: 'button', on: { click: async () => {
          m.close();
          if (!(await confirmDialog(t('Удалить профиль {p}?', { p: p.name }), t('Файл профиля будет удалён.'), { ok: t('Удалить'), danger: true }))) return;
          try {
            await api(`/admin/api/profiles/${encodeURIComponent(p.name)}`, { method: 'DELETE' });
            load();
          } catch (e) { errorToast(e); }
        } } }, t('Удалить')),
      ),
    });
  }

  async function exportSettings(password) {
    try {
      const r = await api('/admin/api/export', { method: 'POST', body: { password }, raw: true });
      const text = await r.text();
      const name = (r.headers.get('content-disposition') || '').match(/filename="([^"]+)"/);
      download(name ? name[1] : 'lmq-settings.yaml', text, 'application/x-yaml');
    } catch (e) {
      errorToast(e);
    }
  }

  function exportWithPassword() {
    const pw = input('', { type: 'password', autocomplete: 'new-password' });
    const pw2 = input('', { type: 'password', autocomplete: 'new-password' });
    modal({
      title: t('Экспорт с секретами'),
      body: h('div.list', field(t('Пароль'), pw), field(t('Ещё раз'), pw2)),
      actions: [{ label: t('Отмена') }, { label: t('Скачать'), kind: 'filled', onClick: async () => {
        if (pw.value.length < 6) throw new Error(t('Пароль — не короче 6 символов'));
        if (pw.value !== pw2.value) throw new Error(t('Пароли не совпадают'));
        await exportSettings(pw.value);
      } }],
    });
  }

  function importSettings() {
    const file = h('input', { type: 'file', accept: '.yaml,.yml,text/yaml' });
    const pw = input('', { type: 'password', placeholder: t('если файл защищён') });
    const target = input('', { placeholder: t('пусто — в текущий профиль') });
    modal({
      title: t('Импорт настроек'),
      body: [
        h('div.list', h('label.field', h('span.field__label', t('Файл')), file), field(t('Пароль'), pw), field(t('В профиль'), target, { hint: t('Новое имя — создать профиль, не трогая текущий') })),
        h('p.group__note', t('Пустые секреты в файле без секретов сохраняют текущие значения.')),
      ],
      actions: [{ label: t('Отмена') }, { label: t('Импортировать'), kind: 'filled', onClick: async () => {
        const f = file.files && file.files[0];
        if (!f) throw new Error(t('Выберите файл'));
        const text = await f.text();
        await api('/admin/api/import', { method: 'POST', body: { text, password: pw.value || null, profile: target.value.trim() || null } });
        await reloadSettings();
        toast(t('Настройки импортированы'), 'ok');
        load();
      } }],
    });
  }

  await load();
}
