mod handler;

use axum::{Router, routing::get};
use handler::{dim, favicon, img, ok};
use lambda_http::{Error, run, tracing};
use std::env;
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Error> {
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

    run(app).await
}
