use actix_web::{get, web, App, HttpRequest, HttpResponse, HttpServer, Responder};
use bytes::Bytes;
use http::StatusCode;
use lazy_static::lazy_static;
use libvips::{self, ops, VipsApp, VipsImage};
use reqwest::{self, header, redirect::Policy, ClientBuilder};
use serde::Deserialize;
use std::{env, error::Error, fmt, time::Duration};
use url::form_urlencoded::byte_serialize;

lazy_static! {
    static ref VIPS_APP: VipsApp = {
        let app = VipsApp::new("image-resize", true).expect("Can't initialize Vips");
        app.concurrency_set(2);
        app
    };
}

#[get("/")]
async fn ok() -> impl Responder {
    HttpResponse::Ok().body("OK")
}

#[get("/img")]
async fn img(req: HttpRequest) -> impl Responder {
    let mut query = web::Query::<RequestQuery>::from_query(req.query_string()).unwrap();

    println!("Resizing for url [{}]", query.url);
    if query.url.starts_with("//") {
        query.url = format!("https:{}", query.url);
    }
    if !query.url.starts_with("http") {
        return HttpResponse::build(StatusCode::BAD_REQUEST).finish();
    }

    let fetch_response = fetch(&query.url).await;
    let mut bytes: Option<Bytes> = None;
    if let Ok(b) = fetch_response {
        bytes = Some(b);
    } else {
        let url_encoded: String = byte_serialize(&query.url.as_bytes()).collect();
        let url = format!("https://images.weserv.nl/?url={}", url_encoded);
        let fetch_response = fetch(&url).await;
        if let Ok(b) = fetch_response {
            bytes = Some(b);
        }
    }
    let bytes = match bytes {
        Some(bytes) => bytes,
        None => return HttpResponse::build(StatusCode::INTERNAL_SERVER_ERROR).finish(),
    };

    let desired_size = Size {
        width: query.w,
        height: query.h,
    };

    let bytes = resize(bytes, &desired_size).await;
    if let Ok(bytes) = bytes {
        HttpResponse::Ok()
            .content_type("image/jpeg")
            .append_header(("Cache-Control", "public, max-age=604800, immutable"))
            .append_header(("Server", "None 1.1"))
            .body(bytes)
    } else {
        println!("Failed resize for image with url [{}]", query.url);
        HttpResponse::build(StatusCode::INTERNAL_SERVER_ERROR).finish()
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let port: u16 = env::var("PORT")
        .unwrap_or_else(|_| String::from("8080"))
        .parse()
        .expect("PORT must be a number");

    let binding_interface = format!("0.0.0.0:{}", port);

    HttpServer::new(|| App::new().service(ok).service(img))
        .bind(binding_interface)?
        .run()
        .await
}

async fn resize(bytes: Bytes, desired_size: &Size) -> Result<Vec<u8>, libvips::error::Error> {
    let image = VipsImage::new_from_buffer(&bytes, "");
    let image = match image {
        Ok(image) => image,
        Err(err) => {
            return Err(err);
        }
    };

    let original_width = image.get_width();
    let resized = get_target_size(original_width, image.get_height(), desired_size);
    let resized = match resized {
        Ok(resized) => resized,
        Err(_) => {
            return Err(libvips::error::Error::InitializationError(""));
        }
    };
    let scale_factor = f64::from(resized.0) / f64::from(original_width);
    let resized_image = ops::resize(&image, scale_factor);

    let resized_image = match resized_image {
        Ok(resized_image) => resized_image,
        Err(err) => {
            println!("{:?}", err);
            return Err(err);
        }
    };
    let bytes = ops::jpegsave_buffer(&resized_image).expect("");
    return Ok(bytes);
}

async fn fetch(url: &str) -> Result<Bytes, Box<dyn std::error::Error>> {
    let mut headers = header::HeaderMap::new();

    headers.insert(
        "Referer",
        header::HeaderValue::from_static("https://google.com"),
    );

    let client = ClientBuilder::new()
        .timeout(Duration::new(10, 0))
        .redirect(Policy::limited(2))
        .default_headers(headers)
        .build();

    let client = match client {
        Ok(client) => client,
        Err(err) => return Err(Box::new(err)),
    };

    let response = client.get(url).send().await;

    let response = match response {
        Ok(r) => r,
        Err(err) => return Err(Box::new(err)),
    };

    if !response.status().is_success() {
        let error_string = format!(
            "Error fetching image from remote, status code:{}",
            response.status().as_str()
        );
        return Err(Box::new(InvalidResponseError { msg: error_string }));
    }

    let bytes = response.bytes().await;
    match bytes {
        Ok(bytes) => return Ok(bytes),
        Err(err) => return Err(Box::new(err)),
    };
}

#[derive(Deserialize)]
struct RequestQuery {
    url: String,
    w: Option<i32>,
    h: Option<i32>,
}

#[derive(Debug)]
struct InvalidResponseError {
    msg: String,
}

impl fmt::Display for InvalidResponseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Invalid size {}", self.msg)
    }
}

impl Error for InvalidResponseError {}

#[derive(Debug, Deserialize, Clone)]
pub struct Size {
    pub width: Option<i32>,
    pub height: Option<i32>,
}

fn get_target_size(
    original_width: i32,
    original_height: i32,
    desired_size: &Size,
) -> Result<(i32, i32), InvalidSizeError> {
    match &desired_size {
        Size {
            width: None,
            height: None,
        } => Ok((original_width, original_height)),
        s if is_negative_or_zero(s) => Err(InvalidSizeError::new(&desired_size)),
        Size {
            width: Some(w),
            height: Some(h),
        } if *h > original_height && *w > original_width => Ok((original_width, original_height)),

        Size {
            width: Some(w),
            height: Some(h),
        } => {
            let diff_height = *h as f32 / original_height as f32;
            let diff_width = *w as f32 / original_width as f32;

            if diff_height < diff_width && diff_height <= 1.0 {
                Ok((get_ratio(*h, original_height, original_width), *h))
            } else {
                Ok((*w, get_ratio(*w, original_width, original_height)))
            }
        }
        Size {
            width: None,
            height: Some(h),
        } => {
            if *h > original_height {
                Ok((original_width, original_height))
            } else {
                Ok((get_ratio(*h, original_height, original_width), *h))
            }
        }
        Size {
            width: Some(w),
            height: None,
        } => {
            if *w > original_width {
                Ok((original_width, original_height))
            } else {
                Ok((*w, get_ratio(*w, original_width, original_height)))
            }
        }
    }
}

fn is_negative_or_zero(size: &Size) -> bool {
    (size.height.is_some() && size.height.unwrap() <= 0)
        || (size.width.is_some() && size.width.unwrap() <= 0)
}

fn get_ratio(desired_measure: i32, original_measure: i32, opposite_orig_measure: i32) -> i32 {
    let ratio = desired_measure as f32 / original_measure as f32;
    (opposite_orig_measure as f32 * ratio) as i32
}

#[derive(Debug)]
struct InvalidSizeError {
    msg: String,
}

impl InvalidSizeError {
    pub fn new(size: &Size) -> InvalidSizeError {
        let message = format!("Size {:?} is not valid.", &size);
        InvalidSizeError { msg: message }
    }
}
impl fmt::Display for InvalidSizeError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Invalid size {}", self.msg)
    }
}

impl Error for InvalidSizeError {}
