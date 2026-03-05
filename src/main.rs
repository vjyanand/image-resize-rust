mod handler;

use axum::{Router, routing::get};
use handler::{dim, favicon, img, ok};
use std::env;
use tracing::{debug, info, warn};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let port: u16 = env::var("PORT")
        .unwrap_or_else(|_| String::from("8080"))
        .parse()
        .expect("PORT must be a number");

    let binding_interface = format!("0.0.0.0:{port}");
    info!("Listening at {binding_interface}");

    let app = Router::new()
        .route("/", get(ok))
        .route("/img", get(img))
        .route("/favicon", get(favicon))
        .route("/dim", get(dim));

    let listener = tokio::net::TcpListener::bind(binding_interface)
        .await
        .unwrap();

    match axum::serve(listener, app).await {
        Ok(_) => {
            debug!("App Running");
        }
        Err(_) => {
            warn!("App Not Running");
        }
    }
}
