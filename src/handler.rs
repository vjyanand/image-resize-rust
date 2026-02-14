use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Json, debug_handler};
use bytes::Bytes;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, PngEncoder};
use image::imageops::FilterType::{self};
use reqwest::ClientBuilder;
use reqwest::{self, header, redirect::Policy};
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::{env, fmt};
use std::{error::Error, time::Duration};
use tracing::{debug, error, info, warn};
use url::form_urlencoded::byte_serialize;

pub(crate) async fn ok() -> &'static str {
    "Ok 🦀"
}

pub(crate) async fn favicon(Query(query): Query<FavIconRequestQuery>) -> impl IntoResponse {
    if query.domain.is_empty() || query.domain.len() < 3 {
        return (StatusCode::BAD_REQUEST, "Missing domain").into_response();
    }
    let fetch_url = format!(
        "https://t0.gstatic.com/faviconV2?client=SOCIAL&type=FAVICON&size=12&fallback_opts=TYPE,SIZE,URL&url=http://{}",
        query.domain
    );

    let result = fetch(&fetch_url).await;
    match result {
        Ok(bytes) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "image/x-icon"),
                (header::CACHE_CONTROL, "public, max-age=604800, immutable"),
            ],
            bytes,
        )
            .into_response(),
        Err(e) => {
            warn!("Google Favicon for domain failed [{}] {}", query.domain, e);
            let fetch_url = format!("https://www.faviconextractor.com/favicon/{}", query.domain);
            let result = fetch(&fetch_url).await;
            match result {
                Ok(bytes) => (
                    StatusCode::OK,
                    [
                        (header::CONTENT_TYPE, "image/x-icon"),
                        (header::CACHE_CONTROL, "public, max-age=604800, immutable"),
                    ],
                    bytes,
                )
                    .into_response(),
                Err(e1) => {
                    let error_msg = format!("Favicon for domain failed [{}] {}", query.domain, e1);
                    error!(error_msg);
                    (StatusCode::INTERNAL_SERVER_ERROR, error_msg).into_response()
                }
            }
        }
    }
}

#[debug_handler]
pub(crate) async fn img(Query(mut query): Query<ImgRequestQuery>) -> impl IntoResponse {
    if query.url.starts_with("//") {
        query.url = format!("https:{}", query.url);
    }

    if !query.url.starts_with("http") {
        let alt_url = env::var("IMAGE_FALLBACK_URL");
        if let Ok(alt_url) = alt_url {
            query.url = alt_url;
        } else {
            let error_msg = format!("Resizing for [{}] failed ", query.url);
            error!(error_msg);
            return (
                StatusCode::BAD_REQUEST,
                [(
                    header::CACHE_CONTROL,
                    "public, max-age=7200, must-revalidate",
                )],
                error_msg,
            )
                .into_response();
        }
    }
    debug!("Resizing for url [{}]", query.url);
    let result = resize_image(&query.url, query.w, query.h).await;

    match result {
        Some((img_bytes, is_png)) => {
            let content_type = if is_png { "image/png" } else { "image/jpeg" };
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, content_type),
                    (header::CACHE_CONTROL, "public, max-age=604800, immutable"),
                ],
                img_bytes,
            )
                .into_response()
        }
        None => {
            let error_msg = format!("Resizing for [{}] failed", query.url);
            error!(error_msg);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                [(
                    header::CACHE_CONTROL,
                    "public, max-age=7200, must-revalidate",
                )],
                error_msg,
            )
                .into_response()
        }
    }
}

async fn resize_image(url: &str, w: Option<u32>, h: Option<u32>) -> Option<(Vec<u8>, bool)> {
    let bytes = match fetch(url).await {
        Ok(bytes) => bytes,
        Err(err) => {
            warn!("Failed fetching with err {}", err);
            let url_encoded: String = byte_serialize(url.as_bytes()).collect();
            let url = format!("https://webkit.extruct.iavian.net/webkit/proxy?url={url_encoded}");
            info!("Fetching from proxy {url}");

            match fetch(&url).await {
                Ok(bytes) => bytes,
                Err(err) => {
                    warn!("Fetching from proxy {url} failed {}", err);
                    return None;
                }
            }
        }
    };

    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .unwrap();
    let image = match reader.decode() {
        Ok(image) => image,
        Err(_) => return None,
    };

    let desired_size = Size {
        width: w,
        height: h,
    };

    let resized = get_target_size(image.width(), image.height(), &desired_size);
    let resized = match resized {
        Ok(resized) => resized,
        Err(_) => return None,
    };

    let image = image.resize(resized.0, resized.1, FilterType::Lanczos3);
    let mut img_bytes = vec![];
    let write_cursor = &mut Cursor::new(&mut img_bytes);

    let encoder = JpegEncoder::new_with_quality(write_cursor, 80);
    let result = image.write_with_encoder(encoder);

    if let Err(err) = result {
        warn!("Failed resizing to jpeg image {url} - {err:?}");
        let mut img_bytes = vec![];
        let write_cursor = &mut Cursor::new(&mut img_bytes);
        let encoder = PngEncoder::new_with_quality(
            write_cursor,
            CompressionType::Default,
            image::codecs::png::FilterType::Adaptive,
        );
        let result = image.write_with_encoder(encoder);
        if let Err(err) = result {
            error!("Error resizing to png image {url} - {err:?}");
            return None;
        }
        return Some((img_bytes, true));
    }
    Some((img_bytes, false))
}

pub(crate) async fn dim(Query(mut query): Query<ImgRequestQuery>) -> impl IntoResponse {
    if query.url.starts_with("//") {
        query.url = format!("https:{}", query.url);
    }

    if !query.url.starts_with("http") {
        let alt_url = env::var("IMAGE_FALLBACK_URL");
        if let Ok(alt_url) = alt_url {
            query.url = alt_url;
        } else {
            let error_msg = format!("Resizing for [{}] failed ", query.url);
            error!(error_msg);
            return (
                StatusCode::BAD_REQUEST,
                [(
                    header::CACHE_CONTROL,
                    "public, max-age=7200, must-revalidate",
                )],
                error_msg,
            )
                .into_response();
        }
    }
    debug!("Resizing for url [{}]", query.url);
    let result = dimension_image(&query.url).await;

    match result {
        Some(size) => {
            let content_type = "application/json";
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, content_type),
                    (header::CACHE_CONTROL, "public, max-age=604800, immutable"),
                ],
                Json(size),
            )
                .into_response()
        }
        None => {
            let error_msg = format!("Dimension for [{}] failed", query.url);
            error!(error_msg);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                [(
                    header::CACHE_CONTROL,
                    "public, max-age=7200, must-revalidate",
                )],
                error_msg,
            )
                .into_response()
        }
    }
}

async fn dimension_image(url: &str) -> Option<Size> {
    let bytes = match fetch(url).await {
        Ok(bytes) => bytes,
        Err(err) => {
            warn!("Failed fetching with err {}", err);
            let url_encoded: String = byte_serialize(url.as_bytes()).collect();
            let url = format!("https://webkit.extruct.iavian.net/webkit/proxy?url={url_encoded}");
            info!("Fetching from proxy {url}");

            match fetch(&url).await {
                Ok(bytes) => bytes,
                Err(err) => {
                    warn!("Fetching from proxy {url} failed {}", err);
                    return None;
                }
            }
        }
    };

    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .unwrap();
    let image = match reader.decode() {
        Ok(image) => image,
        Err(_) => return None,
    };
    Some(Size {
        height: Some(image.height()),
        width: Some(image.width()),
    })
}

#[derive(Debug)]
struct InvalidResponseError {
    msg: String,
}

impl fmt::Display for InvalidResponseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Invalid response {}", self.msg)
    }
}

impl Error for InvalidResponseError {}

#[derive(Deserialize)]
pub(crate) struct ImgRequestQuery {
    url: String,
    w: Option<u32>,
    h: Option<u32>,
}

#[derive(Deserialize)]
pub(crate) struct FavIconRequestQuery {
    domain: String,
}

async fn fetch(url: &str) -> Result<Bytes, InvalidResponseError> {
    // Set up headers to mimic a real browser
    let mut headers = header::HeaderMap::new();
    headers.insert(
        header::REFERER,
        header::HeaderValue::from_static("https://www.google.com"),
    );
    headers.insert(
        header::USER_AGENT,
        header::HeaderValue::from_static(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
        ),
    );
    headers.insert(
        header::ACCEPT,
        header::HeaderValue::from_static("image/jpeg,image/png,image/webp,image/*,*/*;q=0.8"),
    );
    headers.insert(
        header::ACCEPT_LANGUAGE,
        header::HeaderValue::from_static("en-US,en;q=0.9"),
    );
    headers.insert(
        header::ACCEPT_ENCODING,
        header::HeaderValue::from_static("gzip, deflate, br, zstd"),
    );
    headers.insert(
        header::CONNECTION,
        header::HeaderValue::from_static("keep-alive"),
    );

    // Build HTTP client with stealthy configurations
    let client = ClientBuilder::new()
        .timeout(Duration::from_secs(40))
        .redirect(Policy::limited(3)) // Allow slightly more redirects
        .default_headers(headers)
        .gzip(true) // Enable gzip compression
        .brotli(true) // Enable brotli compression
        .zstd(true)
        .deflate(true)
        .http2_adaptive_window(true) // Optimize HTTP/2 performance
        .build();

    let client = match client {
        Ok(client) => client,
        Err(err) => {
            let error_string = format!("Failed to create HTTP client: {err}");
            error!(error_string);
            return Err(InvalidResponseError { msg: error_string });
        }
    };

    let response = client.get(url).send().await;

    let response = match response {
        Ok(r) => r,
        Err(err) => {
            let error_string = format!("Error fetching {url} from remote:{err:#?}");
            warn!(error_string);
            return Err(InvalidResponseError { msg: error_string });
        }
    };

    if !response.status().is_success() {
        let error_string = format!(
            "Error fetching {url} from remote, status code:{}",
            response.status().as_str()
        );
        warn!("{error_string}");
        return Err(InvalidResponseError { msg: error_string });
    }

    let bytes = response.bytes().await;
    match bytes {
        Ok(bytes) => Ok(bytes),
        Err(err) => {
            let error_string = format!("Error fetching bytes {url} from response: {err}");
            warn!(error_string);
            Err(InvalidResponseError { msg: error_string })
        }
    }
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
        s if is_negative_or_zero(s) => Err(InvalidSizeError::new(desired_size)),
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
    (size.height.is_some() && size.height.unwrap() == 0)
        || (size.width.is_some() && size.width.unwrap() == 0)
}

fn get_ratio(desired_measure: u32, original_measure: u32, opposite_orig_measure: u32) -> u32 {
    let ratio = desired_measure as f32 / original_measure as f32;
    (opposite_orig_measure as f32 * ratio) as u32
}

#[derive(Debug, Deserialize, Serialize)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_resize_image() {
        let url = "https://bigthink.com/wp-content/uploads/2019/12/origin-130.jpg";
        let result = resize_image(url, Some(100), Some(100)).await;
        assert!(result.is_some());
    }
}
