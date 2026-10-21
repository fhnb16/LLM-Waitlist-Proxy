import { desktop, desktopCall } from '../api.js';
import { t, initI18n } from '../i18n.js';
import { h, clear, hero, group, list, field, select, segmented, cellSwitch, toast, errorToast, clone } from '../ui.js';
import { applyUi, allThemes, isDark, thumbnail } from '../theme.js';
import { store, saveSettings } from '../store.js';

const PRESETS = ['#0078d4', '#007aff', '#6750a4', '#2d73ff', '#1d9bf0', '#00a4ef', '#1ed760', '#3bb300', '#ffcc00', '#ff9000', '#ff4500', '#fe2c55', '#e7d700', '#00e5ff'];

export async function render(ctx) {
  ctx.title(t('Оформление'));
  const root = ctx.root;
  root.classList.add('app__screen--wide');
  const ui = clone(store.settings.file.ui);
  let timer = null;
  const persist = () => {
    clearTimeout(timer);
    timer = setTimeout(async () => {
      try {
        const file = clone(store.settings.file);
        file.ui = { ...file.ui, ...ui };
        await saveSettings(file);
        if (desktop) desktopCall('apply_appearance').catch(() => {});
      } catch (e) {
        errorToast(e);
      }
    }, 300);
  };
  const apply = () => {
    applyUi(ui, store.systemAccent);
    document.documentElement.classList.toggle('vibrancy', desktop && ui.backdrop !== 'none');
    persist();
    paintSkins();
  };

  const skins = h('div.skins', { role: 'listbox', 'aria-label': t('Тема') });
  function paintSkins() {
    clear(skins);
    const dark = isDark(ui.scheme);
    const acc = /^#[0-9a-f]{6}$/i.test(ui.accent) ? ui.accent : ui.accent === 'system' ? store.systemAccent : null;
    for (const th of allThemes()) {
      const on = th.id === ui.skin;
      skins.append(h('button.skins__item', {
        type: 'button',
        role: 'option',
        'aria-selected': String(on),
        title: th.description || th.name,
        class: on ? 'skins__item--active' : null,
        on: { click: () => { ui.skin = th.id; apply(); } },
      }, thumbnail(th, dark, acc), h('span.skins__mode', th.mode === 'dark' ? t('тёмная') : t('светлая')), h('span.skins__name', th.name)));
    }
  }

  const accentBox = h('div.swatches');
  function paintAccent() {
    clear(accentBox);
    const opt = (value, color, label) => {
      const on = ui.accent === value;
      return h('button.swatches__item', {
        type: 'button',
        title: label,
        'aria-label': label,
        'aria-pressed': String(on),
        class: on ? 'swatches__item--active' : null,
        on: { click: () => { ui.accent = value; apply(); paintAccent(); } },
      }, h('span.swatches__dot', { style: { background: color } }));
    };
    accentBox.append(opt('', 'conic-gradient(var(--tint) 0 50%, var(--warn) 0)', t('Акцент темы')));
    if (desktop && store.systemAccent) accentBox.append(opt('system', store.systemAccent, t('Акцент Windows')));
    for (const c of PRESETS) accentBox.append(opt(c, c, c));
    const custom = h('input.swatches__input', { type: 'color', value: /^#/.test(ui.accent) ? ui.accent : '#0078d4', 'aria-label': t('Свой цвет') });
    custom.addEventListener('input', () => { ui.accent = custom.value; apply(); });
    custom.addEventListener('change', paintAccent);
    accentBox.append(h('label.swatches__item.swatches__custom', { title: t('Свой цвет') }, h('span.swatches__dot', { style: { background: 'conic-gradient(red, yellow, lime, aqua, blue, magenta, red)' } }), custom));
  }

  const lang = select([{ value: 'auto', label: t('Как в системе') }, { value: 'ru', label: 'Русский' }, { value: 'en', label: 'English' }], ui.language);
  lang.addEventListener('change', async () => {
    ui.language = lang.value;
    persist();
    await initI18n(ui.language);
    toast(t('Язык изменится после перезагрузки страницы'), 'ok', { label: t('Перезагрузить'), onClick: () => location.reload() });
  });

  const opacity = h('input.field__range', { type: 'range', min: '0.15', max: '1', step: '0.05', value: ui.backdrop_opacity });
  opacity.addEventListener('input', () => { ui.backdrop_opacity = Number(opacity.value); applyUi(ui, store.systemAccent); persist(); });

  root.append(
    hero(t('Оформление'), t('26 тем каталога, светлый и тёмный режим каждой, свой акцент.')),
    group(t('Тема'), skins),
    group(t('Режим'), segmented([
      { value: 'system', label: t('Как в системе') },
      { value: 'light', label: t('Светлый') },
      { value: 'dark', label: t('Тёмный') },
    ], ui.scheme, (v) => { ui.scheme = v; apply(); })),
    group(t('Акцент'), h('div', { style: { padding: '0 var(--row-px)' } }, accentBox), t('Акцент перекрашивает кнопки, индикаторы и графики во всех темах.')),
    group(t('Панели'), list(
      cellSwitch(t('Сплошные панели'), ui.solid_bars, (v) => { ui.solid_bars = v; apply(); }, { sub: t('Без размытия под шапкой и вкладками') }),
    )),
    desktop ? group(t('Окно приложения'), h('div.list',
      h('div', segmented([
        { value: 'mica', label: 'Mica' },
        { value: 'acrylic', label: 'Acrylic' },
        { value: 'none', label: t('Без размытия') },
      ], ui.backdrop, (v) => { ui.backdrop = v; apply(); desktopCall('apply_appearance').catch(() => {}); })),
      field(t('Плотность фона'), opacity, { hint: t('Насколько фон темы закрывает размытие окна') }),
    ), t('Mica — Windows 11, Acrylic — Windows 10 1809+. Без поддержки окно остаётся непрозрачным.')) : null,
    group(t('Язык'), h('div.list', field(t('Язык интерфейса'), lang))),
  );
  paintSkins();
  paintAccent();
}
