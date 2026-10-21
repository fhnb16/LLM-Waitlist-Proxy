//! Фиктивный OpenAI-совместимый апстрим для ручной проверки и демонстрации панели:
//! `cargo run -p lmq-core --example mock_upstream -- 127.0.0.1:18080`.
//! Отвечает с задержкой 0.3–2 с, умеет стрим, примерно 5% ответов — HTTP 500.

use std::time::Duration;

use axum::Json;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};

fn rnd(n: u64) -> u64 {
    let b = lmq_core::util::random_bytes::<8>();
    u64::from_le_bytes(b) % n
}

async fn chat(Json(body): Json<Value>) -> Response {
    let model = body.get("model").and_then(Value::as_str).unwrap_or("mock").to_string();
    if rnd(100) < 5 {
        return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": {"message": "mock failure"}}))).into_response();
    }
    let text = "Очередь заданий — это список работ, которые выполняются по порядку, когда освобождается исполнитель. Так сервер не перегружается и никто не ждёт бесконечно.";
    let words: Vec<String> = text.split(' ').map(|w| format!("{w} ")).collect();
    let delay = 300 + rnd(1700);
    if body.get("stream").and_then(Value::as_bool) == Some(true) {
        let n = words.len() as u64;
        let stream = async_stream::stream! {
            tokio::time::sleep(Duration::from_millis(delay / 3)).await;
            for w in words {
                tokio::time::sleep(Duration::from_millis(40)).await;
                yield Ok::<_, std::convert::Infallible>(Event::default().data(json!({"choices": [{"delta": {"content": w}}]}).to_string()));
            }
            yield Ok(Event::default().data(json!({"choices": [{"delta": {}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 20, "completion_tokens": n, "total_tokens": 20 + n}}).to_string()));
            yield Ok(Event::default().data("[DONE]"));
        };
        return Sse::new(stream).into_response();
    }
    tokio::time::sleep(Duration::from_millis(delay)).await;
    Json(json!({
        "id": "mock-1",
        "object": "chat.completion",
        "model": model,
        "choices": [{"index": 0, "message": {"role": "assistant", "content": text}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 20, "completion_tokens": words.len(), "total_tokens": 20 + words.len()}
    }))
    .into_response()
}

#[tokio::main]
async fn main() {
    let addr = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1:18080".into());
    let app = axum::Router::new()
        .route("/v1/chat/completions", post(chat))
        .route("/v1/models", get(|| async { Json(json!({"object": "list", "data": [{"id": "mock"}]})) }))
        .route("/health", get(|| async { Json(json!({"status": "ok"})) }));
    let l = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    println!("mock upstream on http://{addr}/v1/chat/completions");
    axum::serve(l, app).await.unwrap();
}
