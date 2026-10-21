// Клиент admin API. В браузере — тот же адрес и cookie-сессия; в окне приложения
// (Tauri) адрес и токен передаёт приложение через window.__LMQ__.

const boot = window.__LMQ__ || {};
export const desktop = !!boot.desktop;
export const platform = boot.platform || '';
const base = (boot.base || '').replace(/\/$/, '');
const token = boot.token || null;

export class ApiError extends Error {
  constructor(status, message) {
    super(message);
    this.status = status;
  }
}

let onUnauthorized = null;
export function setUnauthorizedHandler(fn) {
  onUnauthorized = fn;
}

export async function api(path, { method = 'GET', body, raw = false, signal } = {}) {
  const headers = { 'X-LMQ': '1' };
  if (token) headers.Authorization = 'Bearer ' + token;
  if (body !== undefined) headers['Content-Type'] = 'application/json';
  let r;
  try {
    r = await fetch(base + path, {
      method,
      headers,
      body: body !== undefined ? JSON.stringify(body) : undefined,
      credentials: 'same-origin',
      signal,
    });
  } catch (e) {
    if (e.name === 'AbortError') throw e;
    throw new ApiError(0, 'нет связи с сервером');
  }
  if (raw) {
    if (!r.ok) throw new ApiError(r.status, r.statusText);
    return r;
  }
  const text = await r.text();
  let data = null;
  try {
    data = text ? JSON.parse(text) : null;
  } catch (_) {
    data = text;
  }
  if (!r.ok) {
    const msg = (data && data.error && data.error.message) || r.statusText || 'ошибка';
    if (r.status === 401 && onUnauthorized) onUnauthorized();
    throw new ApiError(r.status, msg);
  }
  return data;
}

/** Поток событий сервера (SSE). Переподключается сам. */
export function events(onEvent, onState) {
  const url = base + '/admin/api/events' + (token ? '?token=' + encodeURIComponent(token) : '');
  let es = null;
  let closed = false;
  let retry = 1000;
  const names = ['tick', 'job', 'log', 'engine', 'engine_log', 'upstream', 'debug_callback', 'config', 'lagged'];
  const open = () => {
    if (closed) return;
    es = new EventSource(url, { withCredentials: !token });
    es.onopen = () => {
      retry = 1000;
      onState && onState(true);
    };
    es.onerror = () => {
      onState && onState(false);
      es.close();
      setTimeout(open, retry);
      retry = Math.min(retry * 2, 15000);
    };
    for (const n of names) {
      es.addEventListener(n, (ev) => {
        let data = null;
        try {
          data = JSON.parse(ev.data);
        } catch (_) {
          data = ev.data;
        }
        onEvent(n, data);
      });
    }
  };
  open();
  return () => {
    closed = true;
    if (es) es.close();
  };
}

/** POST с ответом-потоком SSE (песочница). */
export async function streamPost(path, body, onEvent, signal) {
  const r = await api(path, { method: 'POST', body, raw: true, signal });
  const reader = r.body.getReader();
  const dec = new TextDecoder();
  let buf = '';
  let ev = 'message';
  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    buf += dec.decode(value, { stream: true });
    let i;
    while ((i = buf.indexOf('\n')) >= 0) {
      const line = buf.slice(0, i).replace(/\r$/, '');
      buf = buf.slice(i + 1);
      if (line === '') {
        ev = 'message';
        continue;
      }
      if (line.startsWith('event:')) ev = line.slice(6).trim();
      else if (line.startsWith('data:')) {
        const d = line.slice(5).trim();
        let data = d;
        try {
          data = JSON.parse(d);
        } catch (_) { /* как есть */ }
        onEvent(ev, data);
      }
    }
  }
}

/** Вызов функции настольного приложения (Tauri). Вне приложения — null. */
export async function desktopCall(cmd, args = {}) {
  const inv = window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke;
  if (!inv) return null;
  return inv(cmd, args);
}

export function sseTokenQuery() {
  return token ? '?token=' + encodeURIComponent(token) : '';
}
