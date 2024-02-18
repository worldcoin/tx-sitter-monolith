//! src/telemetry.rs
use std::str::FromStr;
use tracing::{info, subscriber::set_global_default, Subscriber};
use tracing_log::LogTracer;
use tracing_subscriber::{layer::SubscriberExt, EnvFilter, Registry};

// see https://github.com/tekul/rust-tracing-otlp
// see https://www.lpalmieri.com/posts/2020-09-27-zero-to-production-4-are-we-observable-yet/

pub fn get_subscriber(
    name: String, 
    env_filter: String
) -> impl Subscriber + Sync + Send {
    info!("Setting up {name} subscriber"); // name to be used in a better way maybe
    let fmt_layer = tracing_subscriber::fmt::layer().pretty().compact();
    let telemetry_layer =
        create_otlp_tracer().map(|t| tracing_opentelemetry::layer().with_tracer(t));
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(env_filter));

    Registry::default()
        .with(fmt_layer)
        .with(telemetry_layer)
        .with(env_filter)
}

pub fn init_subscriber(subscriber: impl Subscriber + Sync + Send) {
    LogTracer::init().expect("Failed to set logger");
    set_global_default(subscriber).expect("Failed to set subscriber");
}

fn create_otlp_tracer() -> Option<opentelemetry_sdk::trace::Tracer> {
    if !std::env::vars().any(|(name, _)| name.starts_with("OTEL_")) {
        return None;
    }
    let protocol = std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL").unwrap_or("grpc".to_string());

    let tracer = opentelemetry_otlp::new_pipeline().tracing();
    let headers = parse_otlp_headers_from_env();

    let tracer = match protocol.as_str() {
        "grpc" => {
            let mut exporter = opentelemetry_otlp::new_exporter()
                .tonic()
                .with_metadata(metadata_from_headers(headers));

            // Check if we need TLS
            if let Ok(endpoint) = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
                if endpoint.starts_with("https") {
                    exporter = exporter.with_tls_config(Default::default());
                }
            }
            tracer.with_exporter(exporter)
        }
        "http/protobuf" => {
            let exporter = opentelemetry_otlp::new_exporter()
                .http()
                .with_headers(headers.into_iter().collect());
            tracer.with_exporter(exporter)
        }
        p => panic!("Unsupported protocol {}", p),
    };
    
    Some(
        tracer
            .install_batch(opentelemetry_sdk::runtime::Tokio)
            .unwrap(),
    )
}

fn metadata_from_headers(headers: Vec<(String, String)>) -> tonic::metadata::MetadataMap {
    use tonic::metadata;

    let mut metadata = metadata::MetadataMap::new();
    headers.into_iter().for_each(|(name, value)| {
        let value = value
            .parse::<metadata::MetadataValue<metadata::Ascii>>()
            .expect("Header value invalid");
        metadata.insert(metadata::MetadataKey::from_str(&name).unwrap(), value);
    });
    metadata
}

// Support for this has now been merged into opentelemetry-otlp so check next release after 0.14
fn parse_otlp_headers_from_env() -> Vec<(String, String)> {
    let mut headers = Vec::new();

    if let Ok(hdrs) = std::env::var("OTEL_EXPORTER_OTLP_HEADERS") {
        hdrs.split(',')
            .map(|header| {
                header
                    .split_once('=')
                    .expect("Header should contain '=' character")
            })
            .for_each(|(name, value)| headers.push((name.to_owned(), value.to_owned())));
    }
    headers
}