use anyhow::Context;
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use clap::{Parser, ValueEnum};
use serde::Serialize;
use std::{net::SocketAddr, sync::Arc};
use studiobridge_beacn::BeacnStudioBackend;
use studiobridge_core::{
    AppSnapshot, BridgeError, MixerBackend, MockMixerBackend, MockStudioBackend,
    SetLinkAssignmentRequest, SetMicrophoneRequest, SetMuteRequest, SetRouteRequest,
    SetVolumeRequest, StudioBackend, StudioBridgeService,
};
use studiobridge_pipeweaver::PipeweaverBackend;
use tower_http::{cors::CorsLayer, services::ServeDir, trace::TraceLayer};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(about = "Unofficial Linux control daemon for BEACN Studio")]
struct Args {
    #[arg(long, value_enum, default_value_t = StudioMode::Mock)]
    studio: StudioMode,

    #[arg(long, value_enum, default_value_t = MixerMode::Mock)]
    mixer: MixerMode,

    #[arg(long, default_value = "http://127.0.0.1:14565")]
    pipeweaver_url: String,

    #[arg(
        long,
        help = "Enable BEACN gain, phantom-power, and Link assignment writes"
    )]
    allow_hardware_writes: bool,

    #[arg(
        long,
        hide = true,
        help = "Legacy alias; mock mode is already the default"
    )]
    _mock: bool,

    #[arg(long, default_value = "127.0.0.1")]
    bind: String,

    #[arg(long, default_value_t = 17840)]
    port: u16,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum StudioMode {
    Mock,
    Beacn,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum MixerMode {
    Mock,
    Pipeweaver,
}

#[derive(Clone)]
struct AppState {
    service: StudioBridgeService,
    runtime: RuntimeInfo,
}

#[derive(Debug, Serialize)]
struct ApiMessage {
    ok: bool,
    message: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct RuntimeInfo {
    studio_mode: &'static str,
    mixer_mode: &'static str,
    hardware_writes_enabled: bool,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    ok: bool,
    message: &'static str,
    #[serde(flatten)]
    runtime: RuntimeInfo,
}

#[derive(Debug)]
struct ApiError(BridgeError);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.0 {
            BridgeError::InvalidValue(_) => StatusCode::BAD_REQUEST,
            BridgeError::BackendUnavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            BridgeError::Backend(_) => StatusCode::BAD_GATEWAY,
        };
        (
            status,
            Json(serde_json::json!({ "error": self.0.to_string() })),
        )
            .into_response()
    }
}

impl From<BridgeError> for ApiError {
    fn from(value: BridgeError) -> Self {
        Self(value)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let args = Args::parse();
    let studio: Arc<dyn StudioBackend> = match args.studio {
        StudioMode::Mock => Arc::new(MockStudioBackend::default()),
        StudioMode::Beacn => Arc::new(BeacnStudioBackend::spawn(args.allow_hardware_writes)?),
    };
    let mixer: Arc<dyn MixerBackend> = match args.mixer {
        MixerMode::Mock => Arc::new(MockMixerBackend::default()),
        MixerMode::Pipeweaver => Arc::new(PipeweaverBackend::new(&args.pipeweaver_url)),
    };
    let service = StudioBridgeService::new(studio, mixer);
    let state = AppState {
        service,
        runtime: RuntimeInfo {
            studio_mode: match args.studio {
                StudioMode::Mock => "mock",
                StudioMode::Beacn => "beacn",
            },
            mixer_mode: match args.mixer {
                MixerMode::Mock => "mock",
                MixerMode::Pipeweaver => "pipeweaver",
            },
            hardware_writes_enabled: args.allow_hardware_writes,
        },
    };

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/state", get(snapshot))
        .route("/api/studio/microphone", post(set_microphone))
        .route("/api/studio/link-assignment", post(set_link_assignment))
        .route("/api/mixer/volume", post(set_volume))
        .route("/api/mixer/mute", post(set_mute))
        .route("/api/mixer/route", post(set_route))
        .fallback_service(ServeDir::new("web/dist").append_index_html_on_directories(true))
        .layer(
            CorsLayer::new()
                .allow_origin([
                    HeaderValue::from_static("http://127.0.0.1:5173"),
                    HeaderValue::from_static("http://localhost:5173"),
                ])
                .allow_methods([Method::GET, Method::POST])
                .allow_headers([header::CONTENT_TYPE]),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let address: SocketAddr = format!("{}:{}", args.bind, args.port)
        .parse()
        .context("invalid bind address")?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    info!(
        %address,
        studio = ?args.studio,
        mixer = ?args.mixer,
        hardware_writes = args.allow_hardware_writes,
        "StudioBridge daemon listening"
    );
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        ok: true,
        message: "studiobridge is ready",
        runtime: state.runtime,
    })
}

async fn snapshot(State(state): State<AppState>) -> Result<Json<AppSnapshot>, ApiError> {
    Ok(Json(state.service.snapshot().await?))
}

async fn set_microphone(
    State(state): State<AppState>,
    Json(request): Json<SetMicrophoneRequest>,
) -> Result<Json<ApiMessage>, ApiError> {
    state.service.set_microphone(request).await?;
    Ok(ok())
}

async fn set_link_assignment(
    State(state): State<AppState>,
    Json(request): Json<SetLinkAssignmentRequest>,
) -> Result<Json<ApiMessage>, ApiError> {
    state
        .service
        .set_link_assignment(&request.application, request.channel)
        .await?;
    Ok(ok())
}

async fn set_volume(
    State(state): State<AppState>,
    Json(request): Json<SetVolumeRequest>,
) -> Result<Json<ApiMessage>, ApiError> {
    state
        .service
        .set_volume(&request.channel_id, request.mix, request.volume)
        .await?;
    Ok(ok())
}

async fn set_mute(
    State(state): State<AppState>,
    Json(request): Json<SetMuteRequest>,
) -> Result<Json<ApiMessage>, ApiError> {
    state
        .service
        .set_mute(&request.channel_id, request.state)
        .await?;
    Ok(ok())
}

async fn set_route(
    State(state): State<AppState>,
    Json(request): Json<SetRouteRequest>,
) -> Result<Json<ApiMessage>, ApiError> {
    state
        .service
        .set_route(&request.source_id, &request.target_id, request.enabled)
        .await?;
    Ok(ok())
}

fn ok() -> Json<ApiMessage> {
    Json(ApiMessage {
        ok: true,
        message: "updated",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_health_exposes_the_hardware_write_gate() {
        let response = HealthResponse {
            ok: true,
            message: "studiobridge is ready",
            runtime: RuntimeInfo {
                studio_mode: "beacn",
                mixer_mode: "pipeweaver",
                hardware_writes_enabled: false,
            },
        };
        let json = serde_json::to_value(response).unwrap();
        assert_eq!(json["studio_mode"], "beacn");
        assert_eq!(json["mixer_mode"], "pipeweaver");
        assert_eq!(json["hardware_writes_enabled"], false);
    }
}
