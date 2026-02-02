mod handler;

use actix_web::{App, HttpServer};
use env_logger::{Builder, Env};
use handler::{dim, favicon, img, ok};
use log::info;
use std::env;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    Builder::from_env(Env::default())
        .format_timestamp(None)
        .write_style(env_logger::WriteStyle::Always) // Force colors
        .init();

    let port: u16 = env::var("PORT")
        .unwrap_or_else(|_| String::from("8080"))
        .parse()
        .expect("PORT must be a number");

    let binding_interface = format!("0.0.0.0:{port}");
    info!("Listening at {binding_interface}");
    HttpServer::new(|| {
        App::new()
            .service(ok)
            .service(img)
            .service(favicon)
            .service(dim)
    })
    .bind(binding_interface)?
    .run()
    .await
}
