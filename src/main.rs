use lazy_static::lazy_static;
use libvips::{self, ops, VipsApp, VipsImage};
use reqwest::{self, header, redirect::Policy, ClientBuilder, StatusCode};
use serde::Deserialize;
use std::{env, error::Error, fmt, time::Duration};
use warp::{hyper::body::Bytes, Filter};

lazy_static! {
    static ref VIPS_APP: VipsApp = {
        let app = VipsApp::new("image-resize", true).expect("Can't initialize Vips");
        app.concurrency_set(20);
        app
    };
}

#[tokio::main]
async fn main() {
    let resize_route = warp::get()
        .and(warp::path("img"))
        .and(warp::query::<RequestQuery>())
        .and(warp::path::end())
        .and_then(resize)
        .with(warp::log("resize::api"));

    let port: u16 = env::var("PORT")
        .unwrap_or_else(|_| String::from("8080"))
        .parse()
        .expect("PORT must be a number");

    let health_route = warp::path!("ping").map(|| StatusCode::OK);
    let routes = (health_route).or(resize_route);

    warp::serve(routes).run(([0, 0, 0, 0], port)).await
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

async fn resize(query: RequestQuery) -> Result<Box<dyn warp::Reply>, warp::Rejection> {
    println!("Resizing for url [{}]", query.url);

    if !query.url.starts_with("http") {
        return Ok(Box::new(warp::reply::with_status(
            "Invalid url",
            StatusCode::INTERNAL_SERVER_ERROR,
        )));
    }
    let bytes = fetch(&query.url).await;
    let bytes = match bytes {
        Ok(bytes) => bytes,
        Err(err) => {
            return Ok(Box::new(warp::reply::with_status(
                err.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            )))
        }
    };

    let img = VipsImage::new_from_buffer(&bytes, "");
    let img = match img {
        Ok(img) => img,
        Err(_) => {
            return Ok(Box::new(warp::reply::with_status(
                "Error converting image from remote url",
                StatusCode::INTERNAL_SERVER_ERROR,
            )))
        }
    };
    let desired_size = Size {
        width: query.w,
        height: query.h,
    };

    let original_width = img.get_width();
    let resized = get_target_size(original_width, img.get_height(), desired_size);
    let resized = match resized {
        Ok(resized) => resized,
        Err(_) => {
            return Ok(Box::new(warp::reply::with_status(
                "Error getting image size",
                StatusCode::INTERNAL_SERVER_ERROR,
            )))
        }
    };
    let scale_factor = f64::from(resized.0) / f64::from(original_width);
    let resized_image = ops::resize(&img, scale_factor);
    let resized_image = match resized_image {
        Ok(resized_image) => resized_image,
        Err(err) => {
            println!("{:?}", err);
            return Ok(Box::new(warp::reply::with_status(
                "Error resizing image",
                StatusCode::INTERNAL_SERVER_ERROR,
            )));
        }
    };

    let bytes = ops::jpegsave_buffer(&resized_image).expect("");
    let builder = warp::http::response::Builder::new();
    let builder = builder
        .header("Content-Type", "image/jpeg")
        .header("Cache-Control", "public, max-age=604800, immutable")
        .status(200)
        .body(bytes)
        .unwrap();
    Ok(Box::new(builder))
}

#[derive(Deserialize)]
struct RequestQuery {
    url: String,
    w: Option<i32>,
    h: Option<i32>,
}

fn get_target_size(
    original_width: i32,
    original_height: i32,
    desired_size: Size,
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

#[derive(Debug, Deserialize, Clone)]
pub struct Size {
    pub width: Option<i32>,
    pub height: Option<i32>,
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
