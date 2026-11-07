//! LMQ — сервер очереди между ботом UniversalBot и локальными OpenAI-совместимыми
//! моделями (llama.cpp, vLLM, Ollama…). Контракт — `queue-server.md`.
//!
//! Бот ставит задание (`POST /v1/chat/completions` → `202 "Added to queue"`), сервер
//! выполняет его по очереди со слотами, справедливостью по чатам и приоритетами, а
//! результат присылает обратно на `callback.url` боту.

pub mod admin;
pub mod api;
pub mod app;
pub mod assets;
pub mod authenticode;
pub mod callback;
pub mod config;
pub mod crypto;
pub mod debug;
pub mod dispatcher;
pub mod engines;
pub mod events;
pub mod gpu;
pub mod logging;
pub mod metrics;
pub mod net;
pub mod netdiag;
pub mod queue;
pub mod routing;
pub mod schedule;
pub mod socks;
pub mod stats;
pub mod store;
pub mod submit;
pub mod telegram;
pub mod tgrelay;
pub mod themes;
pub mod updates;
pub mod upstream;
pub mod util;

pub use app::{App, Core, StartOptions, VERSION};
