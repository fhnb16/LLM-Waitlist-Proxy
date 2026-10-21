import { desktop, desktopCall } from '../api.js';
import { t } from '../i18n.js';
import { h, hero, group, list, cell, cellSwitch, segmented, toast, errorToast, clone, callout, confirmDialog } from '../ui.js';
import { store, saveSettings } from '../store.js';

export async function render(ctx) {
  ctx.title(t('Приложение'));
  const root = ctx.root;
  if (!desktop) {
    root.append(hero(t('Приложение')), callout(t('Эти настройки доступны в окне настольного приложения LMQ.'), 'warn'));
    return;
  }
  const app = clone(store.settings.file.app);
  const save = async (after) => {
    try {
      const file = clone(store.settings.file);
      file.app = app;
      await saveSettings(file);
      await desktopCall('apply_app_settings');
      toast(t('Сохранено'), 'ok');
      if (after) after();
    } catch (e) {
      errorToast(e);
    }
  };
  const mode = store.settings.file.ui.mode;
  root.append(
    hero(t('Приложение'), t('Настольное приложение для Windows')),
    group(t('Окно'), h('div', segmented([
      { value: 'native', label: t('Нативное (Fluent)') },
      { value: 'web', label: t('Веб (26 тем)') },
    ], mode, async (v) => {
      if (v === mode) return;
      if (!(await confirmDialog(t('Сменить вид окна?'), t('Приложение перезапустится в новом виде. Сервер очереди продолжит работу без потери заданий.'), { ok: t('Перезапустить') }))) return;
      try {
        const file = clone(store.settings.file);
        file.ui.mode = v;
        await saveSettings(file);
        await desktopCall('restart_app');
      } catch (e) {
        errorToast(e);
      }
    })), t('Нативное окно — элементы Windows 11 с Mica; веб-окно — эта панель со всеми темами.')),
    group(t('Запуск'), list(
      cellSwitch(t('Запускать при входе в Windows'), app.autostart, (v) => { app.autostart = v; save(); }),
      cellSwitch(t('Запускать свёрнутым в трей'), app.start_minimized, (v) => { app.start_minimized = v; save(); }),
      cellSwitch(t('Сразу выдавать задания моделям'), app.server_autostart, (v) => { app.server_autostart = v; save(); }, { sub: t('Иначе приложение стартует с выдачей на паузе') }),
    )),
    group(t('Поведение'), list(
      cellSwitch(t('Закрытие окна сворачивает в трей'), app.close_to_tray, (v) => { app.close_to_tray = v; save(); }),
      cellSwitch(t('Уведомления Windows'), app.notifications, (v) => { app.notifications = v; save(); }, { sub: t('Апстрим упал или вернулся, задание не доставлено, движок остановился') }),
    )),
    group(t('Папки'), list(
      cell({ label: t('Открыть папку настроек'), onClick: () => desktopCall('open_path', { which: 'config' }).catch(errorToast), value: store.settings.paths.config_dir, muted: true }),
      cell({ label: t('Открыть папку данных'), onClick: () => desktopCall('open_path', { which: 'data' }).catch(errorToast), value: store.settings.paths.data_dir, muted: true }),
      cell({ label: t('Открыть папку журналов'), onClick: () => desktopCall('open_path', { which: 'logs' }).catch(errorToast), value: store.settings.paths.log_dir, muted: true }),
    ), store.settings.paths.portable ? t('Портативный режим: всё хранится рядом с программой.') : t('Настройки — в %APPDATA%\\LMQ (переносятся с профилем), данные — в %LOCALAPPDATA%\\LMQ.')),
    group(null, list(cell({ label: t('Выйти из LMQ'), kind: 'danger', onClick: async () => {
      if (await confirmDialog(t('Выйти?'), t('Сервер очереди остановится; задания в очереди сохранятся до следующего запуска.'), { ok: t('Выйти'), danger: true })) desktopCall('quit');
    } }))),
  );
}
