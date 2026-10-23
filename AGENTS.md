# AGENTS.md — как работать с репозиторием LMQ 2

Руководство для ИИ-агентов и разработчиков. Что делает продукт и как его настраивать, описано в
[README.md](README.md); здесь — устройство кода, правила и проверки.

## Что это

LMQ — сервер очереди между Telegram-ботом UniversalBot и локальными OpenAI-совместимыми моделями. Бот ставит
задание (`POST /v1/chat/completions` → `202 "Added to queue"`), LMQ выполняет его на апстриме и присылает
результат колбэком. Написан на Rust (edition 2024), версия в `Cargo.toml` → `[workspace.package]`.

Источники истины:

- [queue-server.md](queue-server.md) — ТЗ и контракт с ботом. Ссылки вида «§5.4 ТЗ» в коде ведут сюда.
  Раздел §11 — приёмочные тесты, §12 — чего делать нельзя.
- [docs/universalbot-lmq-integration.md](docs/universalbot-lmq-integration.md) — что должно быть на стороне бота.
- [config.example.yaml](config.example.yaml) — полный пример `settings.yaml`.

## Карта репозитория

| Путь | Что там |
|---|---|
| `crates/lmq-core` | Вся логика: очередь, диспетчер, колбэки, SQLite, статистика, движки, HTTP API, панель. |
| `crates/lmq-server` | Бинарник `lmq-server`: CLI (clap), служба Windows, systemd; метаданные `.deb`. |
| `crates/lmq-desktop` | Бинарник `LMQ.exe` (только Windows): нативное окно Slint или веб-окно Tauri, трей. |
| `web/` | Веб-панель: ванильный JS (ES-модули), `css/base.css`, `css/app.css`, 26 тем, `i18n/en.json`. |
| `packaging/debian` | Unit systemd, `/etc/default/lmq`, `settings.yaml` по умолчанию, скрипты пакета. |
| `.github/workflows/ci.yml` | CI: Windows (fmt, clippy, тесты, NSIS) и Debian bookworm (clippy, тесты, `.deb`). |
| `src/` | Остатки старого проекта на C# WPF. В `.gitignore`, не трогать и не использовать. |
| `dist/<версия>/` | Готовые релизы (установщик, portable-zip, `SHA256SUMS.txt`). В `.gitignore`. |
| `target/` | Сборка Cargo. |

`default-members` — только `lmq-core` и `lmq-server`: голый `cargo build` / `cargo test` не собирает окно.
Для него нужен `--workspace` или `-p lmq-desktop`.

### Модули `lmq-core`

У каждого файла есть шапка `//!` с назначением — читайте её первой.

- **Путь задания:** `api/client.rs` (клиентский API) → `submit.rs` (проверки, лимиты, вытеснение, резерв,
  синхронный режим) → `store.rs` (SQLite WAL) + `queue.rs` (очередь в памяти: ярусы, round-robin по чатам) →
  `dispatcher.rs` (выдача по слотам) → `routing.rs` (модели → апстримы, выключатель) → `upstream.rs` →
  `callback.rs` (доставка боту с повторами).
- **Состояние задания** (`store.rs`): `state` — итог выполнения (`queued | running | done | failed | expired |
  cancelled | superseded`), `delivery` — доставка (`none | pending | delivered | dead`). Это два разных поля.
- **Панель:** `admin.rs` — сервисный слой, общий для admin HTTP API (`api/admin.rs`) и нативного окна (оно
  вызывает его напрямую, без HTTP). Новую функцию панели добавляйте в `admin.rs`, а не в обработчик HTTP.
- **Конфигурация:** `config/` — приоритет: умолчания < `settings.yaml` < окружение < CLI. `settings.rs` — схема,
  `paths.rs` — где лежат файлы, `secrets.rs` — `secrets.bin`.
- **Прочее:** `engines.rs` (запуск llama-server и др.), `stats.rs`, `metrics.rs` (Prometheus), `events.rs`
  (шина для SSE и окна), `net.rs` / `netdiag.rs` (маршруты через интерфейс или прокси), `schedule.rs`,
  `gpu.rs` (NVML, фича `gpu`), `updates.rs` (minisign), `telegram.rs` (протокол 1, выключен по умолчанию),
  `themes.rs`, `assets.rs` (вшитая панель), `crypto.rs`, `logging.rs`, `debug.rs` (приёмник колбэков).

## Сборка и запуск

Окружение — Windows, PowerShell. `cargo` и `tauri-cli` установлены.

```powershell
cargo build                                     # ядро и сервер
cargo build --workspace                         # плюс LMQ.exe
cargo run -p lmq-server -- --home .\dev run     # сервер без окна, все данные в .\dev
cargo run -p lmq-desktop -- --home .\dev        # окно; --mode web — веб-окно
cargo run -p lmq-core --example mock_upstream -- 127.0.0.1:18080   # фиктивный OpenAI-апстрим
cargo run -p lmq-core --example net_probe -- <url> [интерфейс] [прокси]
cd crates\lmq-desktop; cargo tauri build --bundles nsis             # установщик
```

Для опытов всегда используйте `--home .\dev`: без него LMQ возьмёт настоящие `%APPDATA%\LMQ` и
`%LOCALAPPDATA%\LMQ` пользователя. Папки `dev/`, `data/`, файлы `*.sqlite`, `secrets.bin` уже в `.gitignore`.

Веб-панель вшивается в бинарник (`rust-embed` из `web/`) и служит фронтендом Tauri. После правок в `web/`
пересоберите бинарник, иначе изменений не будет видно.

## Проверки перед сдачей работы

Те же, что в CI. Работа не готова, пока все три не проходят:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Debian-задача CI собирает только `lmq-core` и `lmq-server` под Linux. Код, завязанный на Windows
(`windows-sys`, DPAPI, Job Object, служба), держите под `#[cfg(windows)]` и давайте Unix-ветку.

Тесты:

- `crates/lmq-core/tests/acceptance.rs` — приёмка §11 ТЗ, по тесту на пункт;
- `crates/lmq-core/tests/features.rs` — выключатель, алиасы, расписание, `dead`, статистика, admin API,
  профили, экспорт/импорт, протокол 1, движки;
- `crates/lmq-core/tests/web_ui.rs` — `en.json` покрывает все `t('…')` панели, импорты JS и ссылки
  `index.html` существуют;
- `crates/lmq-desktop/tests/i18n.rs` — переводы нативного окна полны;
- юнит-тесты в `themes.rs` — 26 тем, обязательные токены, `index.json` актуален.

Интеграционные тесты поднимают ядро во временной папке, а апстрим и бота — на wiremock
(`tests/common/mod.rs`). Новое поведение покрывайте тестом там же, через эту обвязку.

## Правила кода

- **Стиль:** `rustfmt.toml` — `max_width = 160`, `use_small_heuristics = "Max"`. Не переносите строки вручную
  короче, чем сделает rustfmt.
- **Язык:** комментарии, `//!`-шапки, сообщения ошибок и тестов — по-русски, как в остальном коде. Каждому
  новому модулю — шапка `//!` из одной-двух фраз о назначении.
- **Зависимости:** ставьте новые крейты только при явной нужде. `reqwest` — без TLS по умолчанию, только
  `rustls`; не добавляйте OpenSSL (ломает сборку под Debian и Windows).

### Безопасность и ТЗ (не нарушать)

- Тексты сообщений и ответов не попадают в журнал без `LOG_CONTENT` / `logging.content`. Секреты (ключи,
  токены, `callback.token`, токен Telegram) не пишутся никогда: просто не передавайте их в поля `tracing`.
- Ключи клиентов хранятся только как хэш `sha256:`, остальные секреты — `enc:` (ChaCha20-Poly1305), экспорт —
  `pw:` (Argon2id). Токены сравнивайте за постоянное время — `util::ct_eq`.
- `callback.url` проверяется по белому списку хостов до любого запроса наружу (защита от SSRF).
- Admin API: изменяющие запросы с cookie требуют заголовок `X-LMQ: 1`.
- Ответ модели не обрезать и не форматировать — это делает бот. Задания не терять при перезапуске.
- Панель работает под строгим CSP (`assets.rs`, `tauri.conf.json`): никаких inline-скриптов и внешних
  CDN, шрифтов, картинок. Всё — локальными файлами в `web/`.

## Локализация (RU + EN)

Исходные строки интерфейса пишутся **по-русски**, английский — перевод. Тесты падают, если перевода нет.

| Где строка | Как писать | Куда добавить перевод |
|---|---|---|
| Веб-панель (`web/js`) | `t('Русский текст')`, `tn(n, 'одно', 'два', 'пять')` | `web/i18n/en.json` |
| Rust нативного окна (`crates/lmq-desktop/src/native`) | `t("…")`, `tf("…")`, `tn(…)`, подписи полей | `web/i18n/en.json` |
| Нативное окно, `.slint` | `@tr("…")` | `crates/lmq-desktop/translations/en/LC_MESSAGES/lmq-desktop.po` |

`en.json` — словарь «русская строка → английская». Подстановки — `{name}`.

## Веб-панель и нативное окно

- Экраны панели — `web/js/screens/*.js`, общие компоненты — `web/js/ui.js`, запросы — `web/js/api.js`.
  Разметка CSS по БЭМ.
- Окно управления бывает двух видов, и функции должны быть в обоих:
  - **нативное** (по умолчанию) — `crates/lmq-desktop/ui/*.slint` + `src/native/`;
  - **веб** — та же панель из `web/` в WebView2.
- Поля экрана «Настройки» продублированы: `web/js/screens/settings.js` и `crates/lmq-desktop/src/native/fields.rs`.
  Новую настройку добавляйте в оба места с теми же разделами и подписями.
- Статистика нужна везде: новая функция должна показывать свои числа на обзоре или в своём разделе.

### Темы

Тема — `web/css/themes/<id>.css` с паспортом `/*!theme … */` и всеми 46 токенами из
`themes::REQUIRED_TOKENS`. Тем ровно 26 (это проверяет тест). После правки паспорта пересоберите индекс:

```powershell
cargo run -p lmq-core --example themes_index
```

## Версия и релиз

- Версия — только в `Cargo.toml` (`[workspace.package] version`); Tauri берёт её оттуда.
- При смене версии обновите пример `lmq_<версия>-1_amd64.deb` в README.
- Сборки в CI: `LMQ.exe`, `lmq-server.exe`, NSIS-установщик; `lmq-server` и `.deb` для Debian 12.
- Локальные релизы лежат в `dist/<версия>/`: `LMQ_<версия>_x64-setup.exe`,
  `LMQ-<версия>-windows-x64-portable.zip`, `SHA256SUMS.txt`. Портативный режим включает файл `portable`
  рядом с exe.

## Порядок работы для агентов

- **Git не трогать.** Не выполняйте `git` (commit, push, checkout, reset и т. п.): репозиторием управляет
  владелец. Из git-файлов можно править только `.gitignore` — добавляйте туда новые артефакты сборки и
  локальные данные.
- Недостающие инструменты ставьте через `winget`.
- Не удаляйте и не перезаписывайте пользовательские данные в `%APPDATA%\LMQ`, `%LOCALAPPDATA%\LMQ` и `dist/`.
- Меняется контракт с ботом — сверьтесь с `queue-server.md` и обновите `docs/universalbot-lmq-integration.md`.
- Меняется поведение для пользователя (настройки, CLI, эндпоинты, переменные окружения) — обновите
  README и `config.example.yaml`.
