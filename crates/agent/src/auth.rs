//! API key authentication middleware.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Create an auth middleware function that validates Bearer tokens.
///
/// Returns a closure suitable for `axum::middleware::from_fn`.
pub fn make_auth_layer(
    expected_key: String,
) -> impl Fn(
    Request<Body>,
    Next,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>>
+ Clone
+ Send
+ Sync
+ 'static {
    move |req: Request<Body>, next: Next| {
        let expected = expected_key.clone();
        Box::pin(async move {
            let auth_header = req
                .headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok());

            match auth_header {
                Some(value)
                    if value.split_once(' ').is_some_and(|(scheme, t)| {
                        scheme.eq_ignore_ascii_case("bearer")
                            && ttcore::auth::constant_time_eq(t, &expected)
                    }) =>
                {
                    next.run(req).await
                }
                _ => (
                    StatusCode::UNAUTHORIZED,
                    axum::Json(ttcore::api::ApiResp::<()>::err(
                        "invalid or missing API key",
                    )),
                )
                    .into_response(),
            }
        })
    }
}
