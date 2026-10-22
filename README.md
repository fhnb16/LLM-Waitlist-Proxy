# LMQ 2 — сервер очереди нейросети для UniversalBot

LMQ принимает задания бота, ставит их в очередь и отдаёт локальным моделям (llama.cpp, vLLM, Ollama,
KoboldCpp или любой OpenAI-совместимый сервер). Готовый ответ LMQ присылает боту колбэком. Контракт описан в
[queue-server.md](queue-server.md): протокол 2, ответ `202 "Added to queue"` и колбэк с результатом.

- **Очередь не теряет задания.** Она хранится в SQLite (WAL) и переживает перезапуск: задания в работе
  возвращаются в очередь.
- **Порядок выдачи справедливый.** У каждого чата одновременно выполняется одно задание, чаты обслуживаются по
  кругу (round-robin). Личные чаты идут раньше групп, синхронные запросы — первыми.
- **Нагрузка ограничена слотами апстримов.** Действуют лимиты `MAX_QUEUE` и `MAX_WAIT`; поддерживаются
  вытеснение старого задания чата, отмена, переключение на другой апстрим, повторы и выключатель для упавших
  апстримов.
- **Колбэк защищён и надёжен.** Адрес колбэка проверяется по белому списку хостов (защита от SSRF). Доставка
  повторяется с паузами 2/5/15/30/60 с, после исчерпания повторов задание становится `dead`. Такие задания
  можно доставить заново из панели.
- **Модели настраиваются гибко.** Есть алиасы и цепочки резервных моделей (`fallback`), а также расписание:
  паузы и «тихие часы».
- **Движки управляются из LMQ.** LMQ запускает llama-server и другие бинарники, следит за ними и показывает их
  журнал в реальном времени.
- **Статистика везде.** На обзоре — графики очереди, ожидания, ответа, токенов в секунду и ошибок. Есть
  мониторинг GPU через NVML и Prometheus `/metrics`.
- **Веб-панель** — 26 тем оформления, интерфейс на русском и английском.
- **Окно для Windows 10/11** — нативное (Fluent + Mica) или веб (WebView2 + Mica/Acrylic). Есть значок в
  трее, автозапуск и уведомления.

## Что внутри

| Сборка | Для чего |
|---|---|
| `LMQ.exe` | Windows 10/11: сервер очереди в одном процессе с окном управления. |
| `lmq-server.exe` | Windows без окна; может работать как служба (`service install`). |
| `lmq-server` + `lmq_*.deb` | Debian 12 (bookworm) x64: служба systemd и веб-панель. |

Сборки делает CI (`.github/workflows/ci.yml`): установщик NSIS для Windows и `.deb` для Debian.

## Быстрый старт

### Windows (приложение)

1. Установите LMQ из `LMQ_*_x64-setup.exe` или запустите `LMQ.exe` без установки. Если рядом с exe лежит
   файл `portable`, LMQ хранит всё в папке `data` рядом с собой.
2. При первом запуске откроется окно. Создайте ключ для бота на экране **Клиенты** (ключ показывается
   один раз), затем добавьте модель в полной панели (кнопка **Панель в браузере**).
3. По умолчанию API слушает адрес `0.0.0.0:8800`, панель — `127.0.0.1:8801/ui/`. Окно входит в панель само,
   токен для этого не нужен.

Где LMQ хранит файлы:

| Что | Где |
|---|---|
| Настройки, профили, `secrets.bin` (защищён DPAPI) | `%APPDATA%\LMQ` |
| Очередь `queue.sqlite`, журналы, обновления | `%LOCALAPPDATA%\LMQ` |

Вид окна задаётся в разделе **Настройки → Оформление**:

- **нативный** (по умолчанию) — элементы Windows 11 и фон Mica; в Windows 10 вместо Mica будет Acrylic;
- **веб** — окно WebView2 со всеми 26 темами панели.

При переключении вида приложение перезапускается.

### Windows (служба, без входа пользователя)

```powershell
lmq-server.exe service install --service-home C:\ProgramData\LMQ   # от администратора
sc start LMQ
lmq-server.exe --home C:\ProgramData\LMQ admin-token reset           # токен входа в панель
lmq-server.exe --home C:\ProgramData\LMQ client add bot              # ключ для бота
```

### Debian 12

```bash
sudo apt install ./lmq_2.0.3-1_amd64.deb
sudo systemctl start lmq
# ключ для бота и токен панели (выполняются от имени службы):
sudo -u lmq env LMQ_CONFIG_DIR=/etc/lmq LMQ_DATA_DIR=/var/lib/lmq lmq-server client add bot
sudo -u lmq env LMQ_CONFIG_DIR=/etc/lmq LMQ_DATA_DIR=/var/lib/lmq lmq-server admin-token reset
journalctl -u lmq -f
```

Файлы службы:

- настройки — `/etc/lmq/settings.yaml`;
- окружение — `/etc/default/lmq`;
- данные — `/var/lib/lmq`.

Служба работает от пользователя `lmq` с `ProtectSystem=strict`. Панель доступна на `127.0.0.1:8801/ui/`.
Её можно открыть через SSH-туннель (`ssh -L 8801:127.0.0.1:8801 server`) или опубликовать через прокси
(пример ниже).

## Настройка бота (§10 ТЗ)

1. **Нейросеть → «Новый провайдер»**:
   - тип **«Сервер очереди»**;
   - адрес `https://queue.example.com/v1/chat/completions`;
   - API-ключ — ключ клиента из LMQ (экран **Клиенты** или `lmq-server client add bot`);
   - «Загрузить модели» — выберите основную и резервные (в списке будут и алиасы LMQ);
   - таймаут 10–15 с: это время только на постановку задания.
2. **Настройки → Вебхук → «Токен для сервера очереди»** — сгенерируйте токен. Бот кладёт его в
   `callback.token` каждого задания.
3. Разрешите хост бота: **Настройки → Доставка боту → Разрешённые хосты**
   (`CALLBACK_ALLOWED_HOSTS=bot.example.com`). Задания с колбэком на другой хост отклоняются с `400`.
4. Если у бота нет публичного адреса, задайте в LMQ **Адрес колбэка** и **Токен колбэка**
   (`BOT_CALLBACK_URL`, `BOT_CALLBACK_TOKEN`).
5. Добавьте резервный провайдер (например, облачный) с бо́льшим номером приоритета. Когда LMQ отвечает
   `429` или `503`, бот уйдёт на него сам.

Адрес провайдера — полный путь `…/v1/chat/completions`. LMQ принимает и варианты, которые шлют разные
клиенты:

- с завершающим `/`;
- без `/v1`;
- с префиксом прокси (`/lmq/v1/…`);
- базу с повтором пути (`/v1/chat/completions/chat/completions`).

`GET` этого адреса в браузере отвечает `405` с подсказкой «use POST» — это нормально. Описание сервера
отдаёт `GET /`.

Если клиент получает ошибку, откройте **Журнал**. Туда пишутся:

- неизвестные пути — «unknown API path» с методом и путём;
- отклонённые ключи — «api key rejected», без самого ключа;
- синхронные запросы — «sync request done».

Проверка вручную:

```bash
# постановка, как это делает бот
curl -s https://queue.example.com/v1/chat/completions \
  -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
  -d '{"model":"qwen3-27b","messages":[{"role":"user","content":"Привет"}],
       "protocol":2,"delivery":"bot","job_id":"test-1","chat_id":-1001234567890,"reply_to":null,"thread_id":null,
       "callback":{"url":"https://bot.example.com/webhook/llm","token":"'$CB'"}}'
# → {"status":"Added to queue","job_id":"test-1","position":1,"eta_seconds":…}

# синхронная проверка, как кнопка «Проверить» в боте
curl -s https://queue.example.com/v1/chat/completions -H "Authorization: Bearer $KEY" \
  -H "Content-Type: application/json" -d '{"model":"qwen3-27b","messages":[{"role":"user","content":"ping"}],"chat_id":0}'
```

Чтобы отладить колбэк без бота, используйте встроенный приёмник: **Панель → Отладка**, адрес
`http://127.0.0.1:8801/admin/debug/callback`. Там видно тело, заголовки и код ответа. Приёмник можно
заставить отвечать `403`, `500` или с задержкой, чтобы проверить повторы.

## Модели и движки

Таблица моделей — раздел `models:` в настройках (формат §5.4 ТЗ). Её можно вынести и во внешний файл
`MODELS_CONFIG`. Полный пример со всеми разделами — [config.example.yaml](config.example.yaml).

```yaml
models:
  qwen3-27b:
    upstreams:
      - engine: llama-main        # url и slots берутся из движка
    defaults: { temperature: 0.7, max_tokens: 1024 }
    fallback: [gpt-oss-20b]
aliases:
  fast: gpt-oss-20b
engines:
  - id: llama-main
    kind: llama_cpp
    exe: C:\llama.cpp\llama-server.exe
    llama: { model: C:\models\qwen3-27b-q4_k_m.gguf, port: 8080, ctx_size: 16384, parallel: 2, n_gpu_layers: 99 }
    autostart: true
```

**Слоты** — число одновременных запросов к апстриму, у llama.cpp это `--parallel`. LMQ не отдаёт апстриму
больше заданий, чем у него слотов.

Как LMQ управляет движком:

- ждёт его готовности по `/health` и только потом выдаёт задания;
- перезапускает при падении, но не чаще `max_restarts_per_hour`;
- останавливает вместе с собой: на Windows через Job Object, на Linux через группу процессов.

## Сеть: VPN для моделей, прямой путь к боту

Каждое направление может выходить через свой интерфейс или прокси:

- `network.upstream` — модели (у апстрима можно задать свой маршрут);
- `network.callback` — колбэки боту;
- `network.telegram` — Telegram, только протокол 1.

Типичный случай: зарубежный провайдер (Groq, OpenAI) доступен только через VPN, а сервер бота
не принимает соединения из-за рубежа.

```yaml
network:
  upstream: { proxy: socks5h://127.0.0.1:10808 }   # SOCKS-прокси VPN-клиента (xray, v2rayN, Happ)
  callback: { interface: Ethernet, proxy: none }   # напрямую, мимо VPN в режиме TUN
```

Как задаются поля:

- **интерфейс** — имя (`Ethernet`, `happ-tun`, `eth0`) или IP. На Windows соединение
  привязывается к адресу интерфейса, на Linux — к самому интерфейсу (`SO_BINDTODEVICE`);
- **прокси** — `socks5h://` (DNS через прокси), `socks5://` или `http://`. `none` — напрямую,
  пусто — системные `HTTP(S)_PROXY`.

Проверить маршруты можно тремя способами:

- кнопка **Настройки → Сеть и маршруты → Проверить маршруты**;
- то же в нативном окне;
- из командной строки:

```powershell
lmq-server interfaces
lmq-server check-url https://bot.example.com/webhook/llm --route callback
lmq-server check-url https://api.groq.com/openai/v1/models --proxy socks5h://127.0.0.1:10808
```

Проверка показывает DNS, соединение с каждым адресом, HTTP-ответ и внешний IP, с которого маршрут
выходит в интернет. Сразу видно, идёт ли запрос через VPN. Запросы — `GET` без данных.

## Отладка запросов

- **Подробный журнал** (`logging.requests: true`, или **Настройки → Хранение и журнал**). Для каждого
  задания пишет:
  - строение запроса бота — параметры, сообщения, части, картинки с размером;
  - что ушло модели и какие параметры убраны;
  - ответ апстрима;
  - запрос боту и его ответ.

  Тексты пишутся только вместе с `logging.content`; картинки base64 в журнале сокращаются.
- **Отклонённые параметры.** Если апстрим отвергает параметр («property 'mirostat' is unsupported»),
  LMQ убирает его, повторяет запрос и дальше его не отправляет. Постоянный список задаётся в
  `drop_params` апстрима.
- **Картинки.** В обычном журнале у «job queued» есть `images=N`. Если модель понимает картинки,
  включите у неё `vision`: `/v1/models` сообщит об этом боту.
- **Задания из базы.** Подробности смотрят командами:
  - `lmq-server job list --state dead`;
  - `lmq-server job show <job_id>` — параметры и строение сообщений без текстов.

  У недоставленных заданий payload хранится до `RETENTION_HOURS`.
- **Ошибки.** Неудачи доставки пишутся с полной причиной и маршрутом
  («network to bot.example.com: … connect timed out (интерфейс Ethernet, 10012 ms)»). Ошибки кнопок
  панели и отказы до постановки (400/429/503) тоже попадают в журнал.

## Конфигурация

Порядок приоритета: значения по умолчанию < `settings.yaml` < переменные окружения < флаги командной строки.
Поддерживаются все переменные из §6 ТЗ:

- `LISTEN`, `API_KEYS`, `BOT_CALLBACK_URL`, `BOT_CALLBACK_TOKEN`, `CALLBACK_ALLOWED_HOSTS`;
- `MODELS_CONFIG`, `DB_PATH`, `RETENTION_HOURS`, `MAX_BODY_MB`;
- `MAX_QUEUE`, `MAX_QUEUE_PER_CHAT`, `MAX_WAIT`, `UPSTREAM_TIMEOUT`, `SYNC_TIMEOUT`;
- `SUPERSEDE_SAME_CHAT`, `PRIVATE_FIRST`, `CALLBACK_RETRIES`, `UPSTREAM_RETRIES`;
- `LOG_LEVEL`, `LOG_CONTENT`.

Дополнительные переменные LMQ:

- `ADMIN_LISTEN`, `CALLBACK_ALLOW_HTTP_PRIVATE`, `STATS_RETENTION_DAYS`, `METRICS_TOKEN`;
- `PROTOCOL1_ENABLED`, `TELEGRAM_BOT_TOKEN`, `SECRET_KEY`;
- `LMQ_HOME`, `LMQ_CONFIG_DIR`, `LMQ_DATA_DIR`.

Поле, заданное окружением, панель показывает заблокированным.

Секреты не хранятся открытым текстом:

- ключи клиентов — только как хэш `sha256:`;
- токены и ключи апстримов — зашифрованы (`enc:`, ChaCha20-Poly1305).

Мастер-ключ лежит в `secrets.bin`; `SECRET_KEY` из окружения его перекрывает. Экспорт настроек с секретами
шифруется паролем (Argon2id).

Протокол 1 (сервер сам пишет в Telegram) реализован, но выключен по умолчанию. При нём теряются
форматирование, темы форума и память, поэтому включайте его, только если без него не обойтись.

## Публикация наружу

API и панель слушают локальные адреса, наружу их публикует обратный прокси с TLS.

**Caddy:**

```caddy
queue.example.com {
    reverse_proxy 127.0.0.1:8800
}
# Панель — только с паролем или из VPN.
lmq-admin.example.com {
    basicauth { admin $2a$14$… }
    reverse_proxy 127.0.0.1:8801
}
```

**nginx:**

```nginx
server {
    listen 443 ssl http2;
    server_name queue.example.com;
    ssl_certificate     /etc/letsencrypt/live/queue.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/queue.example.com/privkey.pem;
    client_max_body_size 8m;            # = MAX_BODY_MB
    location / {
        proxy_pass http://127.0.0.1:8800;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_read_timeout 90s;          # синхронный режим (SYNC_TIMEOUT)
    }
    location /metrics { deny all; }
}
```

Если панель публикуется на другом адресе, добавьте его в `server.admin_cors_origins`.

## Эндпоинты

| Метод и путь | Что делает |
|---|---|
| `POST /v1/chat/completions` | Постановка задания (`202`) или синхронный ответ при `chat_id` 0. |
| `GET /v1/models` | Модели и алиасы в формате OpenAI. |
| `GET /v1/jobs/{id}` | Статус задания, без текстов. |
| `DELETE /v1/jobs/{id}` | Отмена задания, `cancelled`. |
| `GET /v1/queue` | Очередь по моделям, среднее ожидание и время ответа. |
| `GET /healthz` | Проверка живости, без авторизации. |
| `GET /metrics` | Prometheus; за `METRICS_TOKEN` или только с localhost. |
| `/ui/` | Веб-панель на `ADMIN_LISTEN`. |
| `/admin/api/*` | API панели; вход по админ-токену. |

## Разработка

```powershell
cargo test --workspace                          # приёмочные тесты §11, функции, переводы
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo run -p lmq-server -- --home .\dev run     # сервер без окна
cargo run -p lmq-desktop -- --home .\dev        # окно (--mode web — веб-окно)
cargo run -p lmq-core --example mock_upstream   # фиктивный OpenAI-сервер для опытов
cargo tauri build --bundles nsis                # установщик (в crates/lmq-desktop, нужен tauri-cli)
```

Структура:

- `crates/lmq-core` — вся логика: очередь, диспетчер, колбэки, хранение, статистика, движки, API, панель;
- `crates/lmq-server` — запуск без окна, CLI и служба;
- `crates/lmq-desktop` — окно Windows (Slint и Tauri), трей, автозапуск;
- `web/` — веб-панель (BEM, `base.css` и 26 тем);
- `packaging/debian` — unit systemd и скрипты пакета.

Строки интерфейса пишутся по-русски. Английский перевод лежит в двух местах:

- веб-панель и строки Rust — `web/i18n/en.json`;
- нативное окно — `crates/lmq-desktop/translations/en`.

Полноту обоих переводов проверяют тесты.
