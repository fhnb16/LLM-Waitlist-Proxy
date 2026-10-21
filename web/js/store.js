// Общее состояние панели и шина событий.

import { api } from './api.js';

const listeners = new Map();

export const store = {
  settings: null, // ответ GET /admin/api/settings: { file, effective, locked, profile, mask, paths }
  overview: null, // последняя сводка (tick)
  connected: false,
  systemAccent: null,
};

export function on(type, fn) {
  if (!listeners.has(type)) listeners.set(type, new Set());
  listeners.get(type).add(fn);
  return () => listeners.get(type).delete(fn);
}

export function emit(type, data) {
  for (const fn of listeners.get(type) || []) {
    try {
      fn(data);
    } catch (e) {
      console.error(e);
    }
  }
}

export async function reloadSettings() {
  store.settings = await api('/admin/api/settings');
  emit('settings', store.settings);
  return store.settings;
}

/** Сохраняет весь файл настроек (секреты-маски остаются прежними на сервере). */
export async function saveSettings(file) {
  store.settings = await api('/admin/api/settings', { method: 'PUT', body: file });
  emit('settings', store.settings);
  return store.settings;
}

export async function reloadOverview() {
  store.overview = await api('/admin/api/overview');
  emit('tick', store.overview);
  return store.overview;
}

export function isLocked(path) {
  const locked = (store.settings && store.settings.locked) || [];
  return locked.includes(path) || locked.some((l) => path.startsWith(l + '.'));
}
