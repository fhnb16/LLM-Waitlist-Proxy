import { api } from '../api.js';
import { t } from '../i18n.js';
import { h, hero, group, list, cell, fmt, showSecret, confirmDialog, errorToast, copy } from '../ui.js';

export async function render(ctx) {
  ctx.title(t('О программе'));
  const root = ctx.root;
  const a = await api('/admin/api/about');
  const b = a.bound || {};
  const api_ = b.client ? `http://${b.client}/v1/chat/completions` : '—';
  root.append(
    hero('LMQ ' + a.version, t('Сервер очереди между UniversalBot и локальными моделями')),
    group(t('Подключение бота'), list(
      cell({ label: t('Адрес провайдера'), value: api_, wrap: true, onClick: () => copy(api_) }),
      cell({ label: t('Панель'), value: b.admin ? `http://${b.admin}/ui/` : '—' }),
    ), t('В боте: Нейросеть → новый провайдер «Сервер очереди», адрес выше (через обратный прокси с HTTPS), ключ клиента, «Загрузить модели».')),
    group(t('Система'), list(
      cell({ label: t('Версия'), value: a.version }),
      cell({ label: t('ОС'), value: `${a.os} ${a.arch}` }),
      cell({ label: t('Запущен'), value: fmt.date(a.started_at) }),
      cell({ label: t('Мониторинг GPU'), value: a.gpu_feature ? t('включён (NVML)') : t('не собран') }),
    )),
    group(t('Папки'), list(
      cell({ label: t('Настройки'), value: a.paths.settings_file, wrap: true, stack: true }),
      cell({ label: t('База'), value: a.paths.db, wrap: true, stack: true }),
      cell({ label: t('Журналы'), value: a.paths.log_dir, wrap: true, stack: true }),
    )),
    group(t('Безопасность'), list(cell({ label: t('Выдать новый токен администратора'), kind: 'danger', onClick: async () => {
      if (!(await confirmDialog(t('Новый токен администратора?'), t('Старый токен и все открытые сессии в браузерах перестанут действовать.'), { ok: t('Выдать'), danger: true }))) return;
      try {
        const r = await api('/admin/api/admin-token/reset', { method: 'POST' });
        showSecret(t('Токен администратора'), t('Сохраните токен — больше он показан не будет.'), r.token);
      } catch (e) {
        errorToast(e);
      }
    } })), t('Окно приложения входит своим токеном и не зависит от этого.')),
    h('p.footnote', t('Лицензия Apache-2.0. Темы оформления — из каталога «Борды».')),
  );
}
