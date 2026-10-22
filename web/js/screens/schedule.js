import { t } from '../i18n.js';
import { h, clear, hero, group, list, cell, field, input, numberInput, timeInput, select, cellSwitch, pick, toast, errorToast, clone, navBtn, state, callout } from '../ui.js';
import { store, saveSettings } from '../store.js';

const DAYS = [
  { value: 'mon', label: 'пн' }, { value: 'tue', label: 'вт' }, { value: 'wed', label: 'ср' }, { value: 'thu', label: 'чт' },
  { value: 'fri', label: 'пт' }, { value: 'sat', label: 'сб' }, { value: 'sun', label: 'вс' },
];
const ACTIONS = [
  { value: 'pause_dispatch', label: 'Пауза выдачи (задания ждут)' },
  { value: 'pause_intake', label: 'Пауза приёма (503 до конца окна)' },
  { value: 'limit_slots', label: 'Ограничить одновременные задания' },
];

export async function render(ctx) {
  ctx.title(t('Расписание'));
  const root = ctx.root;
  const draft = clone(store.settings.file);
  const saveBtn = navBtn(t('Сохранить'), save);
  ctx.actions(saveBtn);
  const box = h('div');
  const err = h('div');
  root.append(hero(t('Расписание'), t('Окна по местному времени: «тихие часы», обслуживание, ночные лимиты.')), err, box);
  const active = (store.overview && store.overview.schedule && store.overview.schedule.active) || [];
  if (active.length) root.insertBefore(callout(t('Сейчас действует: {r}', { r: active.map((a) => a.name).join(', ') }), 'warn'), box);

  async function save() {
    clear(err);
    try {
      await saveSettings(draft);
      toast(t('Расписание сохранено'), 'ok');
    } catch (e) {
      err.append(callout(e.message, 'error'));
      errorToast(e);
    }
  }

  function paint() {
    clear(box);
    const models = Object.keys(draft.models);
    if (!draft.schedule.length) box.append(state(t('Правил нет'), t('Например: с 01:00 до 07:00 — не больше одного задания одновременно.')));
    draft.schedule.forEach((r, i) => {
      const name = input(r.name, { placeholder: t('ночь') });
      name.addEventListener('input', () => { r.name = name.value; });
      const from = timeInput(r.from, { label: t('С'), onChange: (v) => { r.from = v; } });
      const to = timeInput(r.to, { label: t('До'), onChange: (v) => { r.to = v; } });
      const action = select(ACTIONS.map((a) => ({ value: a.value, label: t(a.label) })), r.action);
      const slots = numberInput(r.slots, { min: 1, max: 64, presets: [1, 2, 3, 4], label: t('Не больше'), onChange: (v) => { r.slots = v || 1; } });
      const slotsRow = field(t('Не больше'), slots, { hint: t('заданий одной модели одновременно') });
      slotsRow.hidden = r.action !== 'limit_slots';
      action.addEventListener('change', () => { r.action = action.value; slotsRow.hidden = r.action !== 'limit_slots'; });
      box.append(group(r.name || t('Правило {n}', { n: i + 1 }), h('div.list',
        cellSwitch(t('Включено'), r.enabled, (v) => { r.enabled = v; }),
        field(t('Название'), name),
        field(t('С'), from),
        field(t('До'), to, { hint: t('Если «до» меньше «с» — окно через полночь; равны — весь день') }),
        h('div.field.field--stack', h('span.field__label', t('Дни (пусто — каждый день)')), pick(DAYS.map((d) => ({ value: d.value, label: t(d.label) })), r.days, (v) => { r.days = v; })),
        field(t('Действие'), action),
        slotsRow,
        models.length ? h('div.field.field--stack', h('span.field__label', t('Модели (пусто — все)')), pick(models, r.models, (v) => { r.models = v; })) : null,
        cell({ label: t('Удалить правило'), kind: 'danger', onClick: () => { draft.schedule.splice(i, 1); paint(); } }),
      )));
    });
    box.append(group(null, list(cell({ label: t('Добавить правило'), kind: 'tint', onClick: () => {
      draft.schedule.push({ name: '', enabled: true, days: [], from: '01:00', to: '07:00', action: 'limit_slots', models: [], slots: 1 });
      paint();
    } }))));
  }
  paint();
}
