mod handler;

use actix_web::{App, HttpServer};
use handler::{img, ok};
use std::env;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let port: u16 = env::var("PORT")
        .unwrap_or_else(|_| String::from("8080"))
        .parse()
        .expect("PORT must be a number");

    let binding_interface = format!("0.0.0.0:{}", port);
    println!("Listening at {}", binding_interface);
    HttpServer::new(|| App::new().service(ok).service(img))
        .bind(binding_interface)?
        .run()
        .await
}
