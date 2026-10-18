use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Json, debug_handler};
use bytes::Bytes;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, PngEncoder};
use image::imageops::FilterType::{self};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::{self, header, redirect::Policy};
use reqwest::{Client, ClientBuilder};
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::LazyLock;
use std::{env, fmt};
use std::{error::Error, time::Duration};
use tracing::{debug, error, info, warn};
use url::form_urlencoded::byte_serialize;
use url::{Host, Url};

/// Every env variable the service reads, with what happens when it is unset.
const ENV_VARS: &[(&str, &str)] = &[
    ("PORT", "defaults to 8080"),
    ("RUST_LOG", "defaults to info"),
    ("IMAGE_FALLBACK_URL", "proxy fallback disabled"),
    ("DIM_FALLBACK_URL", "remote dimensions fallback disabled"),
];

pub(crate) fn log_env() {
    for (name, unset) in ENV_VARS {
        match env::var(name) {
            Ok(value) => info!("env {name}={value}"),
            Err(_) => info!("env {name} not set, {unset}"),
        }
    }
}

pub(crate) async fn ok() -> &'static str {
    "Ok 🦀"
}

pub(crate) async fn favicon(Query(query): Query<FavIconRequestQuery>) -> impl IntoResponse {
    if query.domain.is_empty() || query.domain.len() < 3 {
        return (StatusCode::BAD_REQUEST, "Missing domain").into_response();
    }
    let domain_encoded: String = byte_serialize(query.domain.as_bytes()).collect();
    let fetch_url = format!(
        "https://t0.gstatic.com/faviconV2?client=SOCIAL&type=FAVICON&size=12&fallback_opts=TYPE,SIZE,URL&url=http://{}",
        domain_encoded
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
            warn!("Google favicon for domain [{}] failed: {}", query.domain, e);
            let fetch_url = format!(
                "https://www.faviconextractor.com/favicon/{}",
                domain_encoded
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
                Err(e1) => {
                    let error_msg = format!("Favicon for domain [{}] failed: {}", query.domain, e1);
                    error!("{error_msg}");
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
        let error_msg = format!("Invalid url [{}], expected http(s)", query.url);
        error!("{error_msg}");
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
    debug!("Resizing url [{}]", query.url);
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
            error!("{error_msg}");
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
    let image = fetch_image(url).await?;

    let desired_size = Size {
        width: w,
        height: h,
    };

    let resized = get_target_size(image.width(), image.height(), &desired_size);
    let resized = match resized {
        Ok(resized) => resized,
        Err(err) => {
            warn!("Not resizing {url}: {err}");
            return None;
        }
    };

    let image = image.resize(resized.0, resized.1, FilterType::Lanczos3);
    let mut img_bytes = vec![];
    let write_cursor = &mut Cursor::new(&mut img_bytes);

    let encoder = JpegEncoder::new_with_quality(write_cursor, 80);
    let result = image.write_with_encoder(encoder);

    if let Err(err) = result {
        warn!("Failed encoding {url} as jpeg, trying png: {err}");
        let mut img_bytes = vec![];
        let write_cursor = &mut Cursor::new(&mut img_bytes);
        let encoder = PngEncoder::new_with_quality(
            write_cursor,
            CompressionType::Default,
            image::codecs::png::FilterType::Adaptive,
        );
        let result = image.write_with_encoder(encoder);
        if let Err(err) = result {
            error!("Failed encoding {url} as png: {err}");
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
        let error_msg = format!("Invalid url [{}], expected http(s)", query.url);
        error!("{error_msg}");
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
    debug!("Reading dimensions for url [{}]", query.url);
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
            let error_msg = format!("Dimensions for [{}] failed", query.url);
            error!("{error_msg}");
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
    let (width, height) = match fetch_and_parse(url, read_dimensions).await {
        Some(dims) => dims,
        None => fetch_remote_dimensions(url).await?,
    };
    Some(Size {
        height: Some(height),
        width: Some(width),
    })
}

/// Last resort for `/dim`: asks the webkit service, which renders the image in a
/// real browser, for its dimensions. It answers `{ "width": .., "height": .. }`.
/// `DIM_FALLBACK_URL` is the service prefix the encoded url is appended to,
/// e.g. `https://open.webkit.iavian.net/image?url=`; unset disables the fallback.
async fn fetch_remote_dimensions(url: &str) -> Option<(u32, u32)> {
    let Ok(dim_base_url) = env::var("DIM_FALLBACK_URL") else {
        debug!("DIM_FALLBACK_URL not set, skipping remote dimensions for {url}");
        return None;
    };
    let url_encoded: String = byte_serialize(url.as_bytes()).collect();
    let dim_url = format!("{dim_base_url}{url_encoded}");
    info!("Fetching dimensions from {dim_url}");

    match fetch(&dim_url).await {
        Ok(bytes) => {
            let parsed = parse_remote_dimensions(&bytes);
            if parsed.is_none() {
                warn!("No usable dimensions from {dim_url}");
            }
            parsed
        }
        Err(err) => {
            warn!("Fetching dimensions from {dim_url} failed: {}", err);
            None
        }
    }
}

fn parse_remote_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    #[derive(Deserialize)]
    struct RemoteDimensions {
        width: u32,
        height: u32,
    }

    let dims: RemoteDimensions = serde_json::from_slice(bytes).ok()?;
    (dims.width > 0 && dims.height > 0).then_some((dims.width, dims.height))
}

#[derive(Debug)]
struct InvalidResponseError {
    msg: String,
}

impl fmt::Display for InvalidResponseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.msg)
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

/// Shared across requests so connections, TLS sessions and the loaded root
/// certificates are reused instead of rebuilt for every fetch.
static CLIENT: LazyLock<reqwest::Result<Client>> = LazyLock::new(build_client);

fn build_client() -> reqwest::Result<Client> {
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

    // Build HTTP client with stealthy configurations
    ClientBuilder::new()
        .timeout(Duration::from_secs(40))
        .redirect(Policy::custom(|attempt| {
            if attempt.previous().len() >= 3 {
                attempt.error("too many redirects")
            } else if !is_allowed_url(attempt.url()) {
                attempt.error("redirect to a non-public address")
            } else {
                attempt.follow()
            }
        }))
        .dns_resolver(PublicOnlyResolver)
        .default_headers(headers)
        .gzip(true) // Enable gzip compression
        .brotli(true) // Enable brotli compression
        .zstd(true)
        .deflate(true)
        .http2_adaptive_window(true) // Optimize HTTP/2 performance
        .build()
}

async fn fetch(url: &str) -> Result<Bytes, InvalidResponseError> {
    if !Url::parse(url).is_ok_and(|u| is_allowed_url(&u)) {
        let error_string = format!("Refusing to fetch {url}: not a public http(s) url");
        return Err(InvalidResponseError { msg: error_string });
    }

    let client = match CLIENT.as_ref() {
        Ok(client) => client,
        Err(err) => {
            let error_string = format!("Failed to create HTTP client: {err}");
            return Err(InvalidResponseError { msg: error_string });
        }
    };

    let response = client.get(url).send().await;

    let response = match response {
        Ok(r) => r,
        Err(err) => {
            let error_string = format!("Error fetching {url}: {err:?}");
            return Err(InvalidResponseError { msg: error_string });
        }
    };

    if !response.status().is_success() {
        let error_string = format!(
            "Error fetching {url}: status code {}",
            response.status().as_str()
        );
        return Err(InvalidResponseError { msg: error_string });
    }

    let bytes = response.bytes().await;
    match bytes {
        Ok(bytes) => Ok(bytes),
        Err(err) => {
            let error_string = format!("Error reading body of {url}: {err}");
            Err(InvalidResponseError { msg: error_string })
        }
    }
}

async fn fetch_image(url: &str) -> Option<image::DynamicImage> {
    fetch_and_parse(url, decode_image).await
}

/// Fetches `url` and runs `parse` on the body, falling back to the proxy whenever
/// the direct response cannot be fetched *or* cannot be parsed. Sites that block
/// us often answer with a 200 HTML page, so a parse failure needs the fallback too.
/// `IMAGE_FALLBACK_URL` is the proxy prefix the encoded url is appended to,
/// e.g. `https://webkit.extruct.iavian.net/webkit/proxy?url=`; unset disables it.
async fn fetch_and_parse<T>(url: &str, parse: impl Fn(Bytes) -> Option<T>) -> Option<T> {
    // Don't hand blocked urls to the proxy either, it would fetch them for us.
    if !Url::parse(url).is_ok_and(|u| is_allowed_url(&u)) {
        warn!("Refusing to fetch {url}: not a public http(s) url");
        return None;
    }

    match fetch(url).await {
        Ok(bytes) => match parse(bytes) {
            Some(parsed) => return Some(parsed),
            None => warn!("Response from {url} is not a readable image"),
        },
        Err(err) => warn!("Direct fetch failed: {}", err),
    }

    let Ok(proxy_base_url) = env::var("IMAGE_FALLBACK_URL") else {
        debug!("IMAGE_FALLBACK_URL not set, skipping proxy for {url}");
        return None;
    };
    let url_encoded: String = byte_serialize(url.as_bytes()).collect();
    let proxy_url = format!("{proxy_base_url}{url_encoded}");
    info!("Fetching from proxy {proxy_url}");

    match fetch(&proxy_url).await {
        Ok(bytes) => match parse(bytes) {
            Some(parsed) => Some(parsed),
            None => {
                warn!("Response from proxy {proxy_url} is not a readable image");
                None
            }
        },
        Err(err) => {
            warn!("Fetching from proxy {proxy_url} failed: {}", err);
            None
        }
    }
}

fn decode_image(bytes: Bytes) -> Option<image::DynamicImage> {
    image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()
}

/// Reads only the image header, without decoding any pixels.
fn read_dimensions(bytes: Bytes) -> Option<(u32, u32)> {
    image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
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
    ((opposite_orig_measure as f32 * ratio) as u32).max(1)
}

/// Only plain http(s) urls whose host is a public name or public IP literal.
/// Names are checked again after DNS resolution by `PublicOnlyResolver`.
fn is_allowed_url(url: &Url) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    match url.host() {
        Some(Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            d != "localhost" && !d.ends_with(".localhost") && !d.ends_with(".internal")
        }
        Some(Host::Ipv4(ip)) => is_public_ip(IpAddr::V4(ip)),
        Some(Host::Ipv6(ip)) => is_public_ip(IpAddr::V6(ip)),
        None => false,
    }
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_ipv4(ip),
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(v4) => is_public_ipv4(v4),
            None => is_public_ipv6(ip),
        },
    }
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        || a == 0 // 0.0.0.0/8
        || (a == 100 && (b & 0xc0) == 64) // 100.64.0.0/10 carrier-grade NAT
        || (a == 192 && b == 0 && c == 0) // 192.0.0.0/24
        || (a == 198 && (b & 0xfe) == 18) // 198.18.0.0/15 benchmarking
        || a >= 240) // 240.0.0.0/4 reserved
}

fn is_public_ipv6(ip: Ipv6Addr) -> bool {
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
        || ip.is_unique_local() // fc00::/7, includes Fly's fdaa:: private network
        || ip.is_unicast_link_local()
        || (ip.segments()[0] == 0x2001 && ip.segments()[1] == 0x0db8)) // documentation
}

/// DNS resolver that drops non-public addresses, so a public-looking hostname
/// can't be pointed at internal services (metadata endpoints, private network).
struct PublicOnlyResolver;

impl Resolve for PublicOnlyResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .filter(|addr| is_public_ip(addr.ip()))
                .collect();
            if addrs.is_empty() {
                return Err(format!("{host} does not resolve to a public address").into());
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
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
        let message = format!("Invalid size {:?}", size);
        InvalidSizeError { msg: message }
    }
}

impl Error for InvalidSizeError {}

impl fmt::Display for InvalidSizeError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(width: Option<u32>, height: Option<u32>) -> Size {
        Size { width, height }
    }

    #[test]
    fn target_size_no_dimensions_keeps_original() {
        assert_eq!(
            get_target_size(800, 600, &size(None, None)).unwrap(),
            (800, 600)
        );
    }

    #[test]
    fn target_size_rejects_zero() {
        assert!(get_target_size(800, 600, &size(Some(0), None)).is_err());
        assert!(get_target_size(800, 600, &size(None, Some(0))).is_err());
        assert!(get_target_size(800, 600, &size(Some(100), Some(0))).is_err());
    }

    #[test]
    fn target_size_width_only() {
        assert_eq!(
            get_target_size(800, 600, &size(Some(400), None)).unwrap(),
            (400, 300)
        );
        // Never upscales.
        assert_eq!(
            get_target_size(800, 600, &size(Some(1600), None)).unwrap(),
            (800, 600)
        );
    }

    #[test]
    fn target_size_height_only() {
        assert_eq!(
            get_target_size(800, 600, &size(None, Some(300))).unwrap(),
            (400, 300)
        );
        assert_eq!(
            get_target_size(800, 600, &size(None, Some(1200))).unwrap(),
            (800, 600)
        );
    }

    #[test]
    fn target_size_both_fits_inside_box() {
        // Height is the tighter constraint.
        assert_eq!(
            get_target_size(800, 600, &size(Some(400), Some(150))).unwrap(),
            (200, 150)
        );
        // Width is the tighter constraint.
        assert_eq!(
            get_target_size(800, 600, &size(Some(100), Some(300))).unwrap(),
            (100, 75)
        );
        // Both larger than the original.
        assert_eq!(
            get_target_size(800, 600, &size(Some(1000), Some(1000))).unwrap(),
            (800, 600)
        );
    }

    #[test]
    fn target_size_never_zero_for_extreme_aspect_ratio() {
        assert_eq!(
            get_target_size(10000, 5, &size(Some(10), None)).unwrap(),
            (10, 1)
        );
        assert_eq!(
            get_target_size(5, 10000, &size(None, Some(10))).unwrap(),
            (1, 10)
        );
    }

    fn encoded(format: image::ImageFormat) -> Bytes {
        let mut buf = Vec::new();
        image::DynamicImage::new_rgb8(37, 21)
            .write_to(&mut Cursor::new(&mut buf), format)
            .unwrap();
        Bytes::from(buf)
    }

    #[test]
    fn parse_remote_dimensions_json() {
        assert_eq!(
            parse_remote_dimensions(br#"{ "width": 1200, "height": 800 }"#),
            Some((1200, 800))
        );
        assert_eq!(
            parse_remote_dimensions(br#"{ "width": 0, "height": 800 }"#),
            None
        );
        assert_eq!(parse_remote_dimensions(br#"{ "error": "nope" }"#), None);
        assert_eq!(parse_remote_dimensions(b"<html>blocked</html>"), None);
    }

    #[test]
    fn read_dimensions_from_header() {
        for format in [image::ImageFormat::Jpeg, image::ImageFormat::Png] {
            assert_eq!(read_dimensions(encoded(format)), Some((37, 21)));
        }
        assert_eq!(
            read_dimensions(Bytes::from_static(b"<html>blocked</html>")),
            None
        );
    }

    fn allowed(url: &str) -> bool {
        Url::parse(url).is_ok_and(|u| is_allowed_url(&u))
    }

    #[test]
    fn allows_public_urls() {
        assert!(allowed("https://example.com/a.jpg"));
        assert!(allowed("http://93.184.216.34/a.jpg"));
        assert!(allowed("http://[2606:2800:220:1::]/a.jpg"));
    }

    #[test]
    fn blocks_internal_urls() {
        for url in [
            "ftp://example.com/a.jpg",
            "file:///etc/passwd",
            "http://localhost:8080/",
            "http://foo.localhost/",
            "http://LOCALHOST./",
            "http://_api.internal/",
            "http://127.0.0.1/",
            "http://2130706433/", // 127.0.0.1 as a decimal integer
            "http://0.0.0.0/",
            "http://10.0.0.1/",
            "http://172.16.0.1/",
            "http://192.168.1.1/",
            "http://169.254.169.254/latest/meta-data/",
            "http://100.64.0.1/",
            "http://[::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://[fdaa::3]/",
            "http://[fe80::1]/",
        ] {
            assert!(!allowed(url), "{url} should be blocked");
        }
    }

    #[tokio::test]
    async fn resolver_filters_private_addresses() {
        let name: Name = "localhost".parse().unwrap();
        assert!(PublicOnlyResolver.resolve(name).await.is_err());
    }

    #[tokio::test]
    #[ignore = "needs network access"]
    async fn test_resize_image() {
        let url = "https://bigthink.com/wp-content/uploads/2019/12/origin-130.jpg";
        let result = resize_image(url, Some(100), Some(100)).await;
        assert!(result.is_some());
    }
}
