import { api } from '../api.js';
import { t } from '../i18n.js';
import { h, hero } from '../ui.js';
import { on } from '../store.js';
import { logView } from './logview.js';

function toLine(e) {
  const f = e.fields || {};
  const parts = Object.entries(f).map(([k, v]) => `${k}=${typeof v === 'string' ? v : JSON.stringify(v)}`);
  return { ts: e.ts, level: e.level, src: '', text: e.message || '', fields: parts.join(' ') };
}

export async function render(ctx) {
  ctx.title(t('Журнал'));
  const root = ctx.root;
  root.classList.add('app__screen--wide');
  const box = h('div');
  root.append(
    hero(t('Журнал сервера'), t('JSON-строки без текстов сообщений и секретов. Файлы — в папке журналов.')),
    box,
  );
  const lv = logView(box, {
    load: async () => (await api('/admin/api/logs?tail=3000')).items.map(toLine),
    filename: 'lmq-log.txt',
  });
  ctx.onDestroy(on('log', (e) => lv.push(toLine(e))));
}
