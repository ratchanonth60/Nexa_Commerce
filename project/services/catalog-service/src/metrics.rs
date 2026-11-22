use std::{
    collections::BTreeMap,
    fmt::Write,
    sync::{Arc, Mutex},
    time::Instant,
};

use axum::{
    extract::{MatchedPath, Request},
    http::{header, Method},
    middleware::Next,
    response::{IntoResponse, Response},
    Extension,
};

const BUCKETS_MS: [u128; 8] = [25, 50, 100, 250, 500, 1000, 2500, 5000];

#[derive(Clone, Default)]
pub(crate) struct Metrics(Arc<Mutex<BTreeMap<RequestLabels, Observation>>>);

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct RequestLabels {
    route: String,
    method: &'static str,
    status: u16,
}

#[derive(Default)]
struct Observation {
    buckets: [u64; BUCKETS_MS.len()],
    count: u64,
    elapsed_nanos: u128,
}

pub(crate) async fn record(
    Extension(metrics): Extension<Metrics>,
    request: Request,
    next: Next,
) -> Response {
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str)
        .unwrap_or("unmatched")
        .to_owned();
    let method = match *request.method() {
        Method::GET => "GET",
        Method::HEAD => "HEAD",
        Method::POST => "POST",
        Method::PATCH => "PATCH",
        Method::OPTIONS => "OPTIONS",
        _ => "OTHER",
    };
    let started = Instant::now();
    let response = next.run(request).await;
    if route != "/metrics" {
        let elapsed = started.elapsed().as_nanos();
        let mut all = metrics.0.lock().unwrap_or_else(|error| error.into_inner());
        let observation = all
            .entry(RequestLabels {
                route,
                method,
                status: response.status().as_u16(),
            })
            .or_default();
        for (count, threshold) in observation.buckets.iter_mut().zip(BUCKETS_MS) {
            *count += u64::from(elapsed <= threshold * 1_000_000);
        }
        observation.count += 1;
        observation.elapsed_nanos += elapsed;
    }
    response
}

pub(crate) async fn scrape(Extension(metrics): Extension<Metrics>) -> impl IntoResponse {
    let all = metrics.0.lock().unwrap_or_else(|error| error.into_inner());
    let mut body = String::from(
        "# HELP nexa_http_requests_total Completed gateway requests.\n\
         # TYPE nexa_http_requests_total counter\n",
    );
    for (
        RequestLabels {
            route,
            method,
            status,
        },
        value,
    ) in all.iter()
    {
        let _ = writeln!(
            body,
            "nexa_http_requests_total{{route=\"{route}\",method=\"{method}\",status=\"{status}\"}} {}",
            value.count,
        );
    }
    body.push_str(
        "# HELP nexa_http_request_duration_seconds Gateway handler duration in seconds.\n\
         # TYPE nexa_http_request_duration_seconds histogram\n",
    );
    for (
        RequestLabels {
            route,
            method,
            status,
        },
        value,
    ) in all.iter()
    {
        let labels = format!("route=\"{route}\",method=\"{method}\",status=\"{status}\"");
        for (threshold, count) in BUCKETS_MS.into_iter().zip(value.buckets) {
            let seconds = threshold as f64 / 1_000.0;
            let _ = writeln!(
                body,
                "nexa_http_request_duration_seconds_bucket{{{labels},le=\"{seconds}\"}} {count}"
            );
        }
        let _ = writeln!(
            body,
            "nexa_http_request_duration_seconds_bucket{{{labels},le=\"+Inf\"}} {}",
            value.count,
        );
        let _ = writeln!(
            body,
            "nexa_http_request_duration_seconds_count{{{labels}}} {}",
            value.count,
        );
        let _ = writeln!(
            body,
            "nexa_http_request_duration_seconds_sum{{{labels}}} {:.9}",
            value.elapsed_nanos as f64 / 1_000_000_000.0,
        );
    }
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
}
