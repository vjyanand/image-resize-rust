use actix_web::http::StatusCode;
use actix_web::{get, web, HttpRequest, HttpResponse, Responder};
use bytes::Bytes;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType::{self};
use reqwest::ClientBuilder;
use reqwest::{self, header, redirect::Policy};
use serde::Deserialize;
use std::io::Cursor;
use std::{env, fmt, io};
use std::{error::Error, time::Duration};
use url::form_urlencoded::byte_serialize;

#[get("/")]
async fn ok() -> impl Responder {
    HttpResponse::Ok().body("Ok")
}

#[get("/img")]
async fn img(req: HttpRequest) -> impl Responder {
    let mut query = web::Query::<RequestQuery>::from_query(req.query_string()).unwrap();
    if query.url.starts_with("//") {
        query.url = format!("https:{}", query.url);
    }

    if !query.url.starts_with("http") {
        let alt_url = env::var("IMAGE_FALLBACK_URL");
        if let Ok(alt_url) = alt_url {
            query.url = alt_url;
        } else {
            return HttpResponse::build(StatusCode::BAD_REQUEST).finish();
        }
    }
    println!("Resizing for url [{}]", query.url);

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

    let reader = image::io::Reader::new(io::Cursor::new(bytes))
        .with_guessed_format()
        .unwrap();
    let image = match reader.decode() {
        Ok(image) => image,
        Err(_) => return HttpResponse::build(StatusCode::INTERNAL_SERVER_ERROR).finish(),
    };

    let desired_size = Size {
        width: query.w,
        height: query.h,
    };

    let resized = get_target_size(image.width(), image.height(), &desired_size);
    let resized = match resized {
        Ok(resized) => resized,
        Err(_) => return HttpResponse::build(StatusCode::INTERNAL_SERVER_ERROR).finish(),
    };

    let image = image.resize(resized.0, resized.1, FilterType::Lanczos3);
    let mut img_bytes = vec![];
    let write_cursor = &mut Cursor::new(&mut img_bytes);
    let encoder = JpegEncoder::new_with_quality(write_cursor, 80);
    let result = image.write_with_encoder(encoder);
    
    match result {
        Ok(_) => HttpResponse::Ok()
            .content_type("image/jpeg")
            .append_header(("Cache-Control", "public, max-age=604800, immutable"))
            .append_header(("x-server", "iavian-img-1.1"))
            .body(img_bytes),
        Err(_) => HttpResponse::build(StatusCode::BAD_REQUEST).finish(),
    }
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

#[derive(Deserialize)]
struct RequestQuery {
    url: String,
    w: Option<u32>,
    h: Option<u32>,
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

fn get_target_size(
  original_width: u32,
  original_height: u32,
  desired_size: &Size,
) -> Result<(u32, u32), InvalidSizeError> {
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

fn get_ratio(desired_measure: u32, original_measure: u32, opposite_orig_measure: u32) -> u32 {
  let ratio = desired_measure as f32 / original_measure as f32;
  (opposite_orig_measure as f32 * ratio) as u32
}

#[derive(Debug, Deserialize, Clone)]
pub struct Size {
    pub width: Option<u32>,
    pub height: Option<u32>,
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

impl Error for InvalidSizeError {}

impl fmt::Display for InvalidSizeError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Invalid size {}", self.msg)
    }
}
