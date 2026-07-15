use reqwest::blocking::Client;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::{error::Error, fmt, thread, time::Duration};
use studiobridge_core::{
    AppSnapshot, CreateMixerSourceRequest, DspWriteModule, LinkChannel, MicrophoneDspSnapshot,
    MicrophoneDspUpdate, MicrophoneDspWriteResult, MixBus, MixerProfileRequest,
    MixerProfilesResponse, MuteState, RemoveMixerSourceRequest, ReorderMixerSourceRequest,
    SetDefaultDeviceRequest, SetLinkAssignmentRequest, SetLinkOutputAssignmentRequest,
    SetMixerApplicationRequest, SetMuteRequest, SetRouteRequest, SetSourceDeviceRequest,
    SetTargetDeviceRequest, SetTargetMuteRequest, SetTargetVolumeRequest, SetVolumeLinkedRequest,
    SetVolumeRequest,
};

const LOCAL_DAEMON_URL: &str = "http://127.0.0.1:17840";
const CONNECT_TIMEOUT: Duration = Duration::from_millis(350);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TIMEOUT: Duration = Duration::from_secs(2);
const DSP_READ_TIMEOUT: Duration = Duration::from_secs(35);
const GET_ATTEMPTS: usize = 2;
const GET_RETRY_DELAY: Duration = Duration::from_millis(40);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonErrorKind {
    Offline,
    Timeout,
    HttpStatus(u16),
    InvalidResponse,
    Transport,
}

#[derive(Debug)]
pub struct DaemonClientError {
    kind: DaemonErrorKind,
    source: reqwest::Error,
}

impl DaemonClientError {
    pub fn kind(&self) -> DaemonErrorKind {
        self.kind
    }
}

impl From<reqwest::Error> for DaemonClientError {
    fn from(source: reqwest::Error) -> Self {
        let kind = if source.is_connect() {
            DaemonErrorKind::Offline
        } else if source.is_timeout() {
            DaemonErrorKind::Timeout
        } else if let Some(status) = source.status() {
            DaemonErrorKind::HttpStatus(status.as_u16())
        } else if source.is_decode() {
            DaemonErrorKind::InvalidResponse
        } else {
            DaemonErrorKind::Transport
        };
        Self { kind, source }
    }
}

impl fmt::Display for DaemonClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind() {
            DaemonErrorKind::Offline => {
                formatter.write_str("StudioBridge daemon is offline on localhost")
            }
            DaemonErrorKind::Timeout => {
                formatter.write_str("StudioBridge daemon request timed out")
            }
            DaemonErrorKind::HttpStatus(status) => {
                write!(formatter, "StudioBridge daemon returned HTTP {status}")
            }
            DaemonErrorKind::InvalidResponse => {
                formatter.write_str("StudioBridge daemon returned an invalid JSON response")
            }
            DaemonErrorKind::Transport => {
                formatter.write_str("StudioBridge daemon transport failed")
            }
        }
    }
}

impl Error for DaemonClientError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

type ClientResult<T> = Result<T, DaemonClientError>;

#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeHealth {
    pub ok: bool,
    pub studio_mode: String,
    pub mixer_mode: String,
    pub hardware_writes_enabled: bool,
    pub link_control_enabled: bool,
    #[serde(default)]
    pub dsp_write_modules: Vec<String>,
    #[serde(default)]
    pub dsp_write_active_module: Option<String>,
    pub dsp_write_lease_seconds: Option<u64>,
}

#[derive(Serialize)]
struct ArmDspRequest {
    module: DspWriteModule,
    acknowledgement: &'static str,
    lease_seconds: u64,
}

#[derive(Clone)]
pub struct DaemonClient {
    base_url: String,
    client: Client,
    read_timeout: Duration,
    dsp_read_timeout: Duration,
    retry_delay: Duration,
}

impl DaemonClient {
    pub fn localhost() -> ClientResult<Self> {
        Self::build(
            LOCAL_DAEMON_URL,
            CONNECT_TIMEOUT,
            REQUEST_TIMEOUT,
            READ_TIMEOUT,
            DSP_READ_TIMEOUT,
            GET_RETRY_DELAY,
        )
    }

    fn build(
        base_url: &str,
        connect_timeout: Duration,
        request_timeout: Duration,
        read_timeout: Duration,
        dsp_read_timeout: Duration,
        retry_delay: Duration,
    ) -> ClientResult<Self> {
        Ok(Self {
            base_url: base_url.trim_end_matches('/').into(),
            client: Client::builder()
                .connect_timeout(connect_timeout)
                .timeout(request_timeout)
                .build()?,
            read_timeout,
            dsp_read_timeout,
            retry_delay,
        })
    }

    #[cfg(test)]
    fn for_test(base_url: &str, timeout: Duration) -> ClientResult<Self> {
        Self::build(
            base_url,
            timeout.min(Duration::from_millis(25)),
            timeout,
            timeout,
            timeout,
            Duration::from_millis(1),
        )
    }

    fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        timeout: Duration,
        retry_timeouts: bool,
    ) -> ClientResult<T> {
        for attempt in 0..GET_ATTEMPTS {
            let result = self
                .client
                .get(format!("{}{path}", self.base_url))
                .timeout(timeout)
                .send()
                .and_then(reqwest::blocking::Response::error_for_status)
                .and_then(|response| response.json());
            match result {
                Ok(value) => return Ok(value),
                Err(error)
                    if attempt + 1 < GET_ATTEMPTS
                        && retryable_get_error(&error, retry_timeouts) =>
                {
                    thread::sleep(self.retry_delay);
                }
                Err(error) => return Err(error.into()),
            }
        }
        unreachable!("GET attempt count is nonzero")
    }

    pub fn health(&self) -> ClientResult<RuntimeHealth> {
        self.get_json("/api/health", self.read_timeout, true)
    }

    pub fn snapshot(&self) -> ClientResult<AppSnapshot> {
        self.get_json("/api/state", self.read_timeout, true)
    }

    pub fn set_volume(&self, channel_id: &str, mix: MixBus, volume: u8) -> ClientResult<()> {
        self.client
            .post(format!("{}/api/mixer/volume", self.base_url))
            .json(&SetVolumeRequest {
                channel_id: channel_id.into(),
                mix,
                volume,
            })
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn set_target_volume(&self, target_id: &str, volume: u8) -> ClientResult<()> {
        self.client
            .post(format!("{}/api/mixer/target-volume", self.base_url))
            .json(&SetTargetVolumeRequest {
                target_id: target_id.into(),
                volume,
            })
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn set_target_mute(&self, target_id: &str, muted: bool) -> ClientResult<()> {
        self.client
            .post(format!("{}/api/mixer/target-mute", self.base_url))
            .json(&SetTargetMuteRequest {
                target_id: target_id.into(),
                muted,
            })
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn set_target_device(
        &self,
        target_id: &str,
        device_node_id: Option<u32>,
    ) -> ClientResult<()> {
        self.client
            .post(format!("{}/api/mixer/target-device", self.base_url))
            .json(&SetTargetDeviceRequest {
                target_id: target_id.into(),
                device_node_id,
            })
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn set_source_device(
        &self,
        channel_id: &str,
        device_node_id: Option<u32>,
    ) -> ClientResult<()> {
        self.client
            .post(format!("{}/api/mixer/source-device", self.base_url))
            .json(&SetSourceDeviceRequest {
                channel_id: channel_id.into(),
                device_node_id,
            })
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn set_link_output_assignment(
        &self,
        output_node_id: u32,
        target_id: Option<&str>,
    ) -> ClientResult<()> {
        self.client
            .post(format!(
                "{}/api/mixer/link-output-assignment",
                self.base_url
            ))
            .json(&SetLinkOutputAssignmentRequest {
                output_node_id,
                target_id: target_id.map(str::to_owned),
            })
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn set_default_input(&self, device_id: &str) -> ClientResult<()> {
        self.set_default_device("input", device_id)
    }

    pub fn set_default_output(&self, device_id: &str) -> ClientResult<()> {
        self.set_default_device("output", device_id)
    }

    fn set_default_device(&self, kind: &str, device_id: &str) -> ClientResult<()> {
        self.client
            .post(format!("{}/api/mixer/default-{kind}", self.base_url))
            .json(&SetDefaultDeviceRequest {
                device_id: device_id.into(),
            })
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn set_mute(&self, channel_id: &str, state: MuteState) -> ClientResult<()> {
        self.client
            .post(format!("{}/api/mixer/mute", self.base_url))
            .json(&SetMuteRequest {
                channel_id: channel_id.into(),
                state,
            })
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn set_volume_linked(&self, channel_id: &str, linked: bool) -> ClientResult<()> {
        self.client
            .post(format!("{}/api/mixer/volume-link", self.base_url))
            .json(&SetVolumeLinkedRequest {
                channel_id: channel_id.into(),
                linked,
            })
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn reorder_source(&self, source_id: &str, position: usize) -> ClientResult<()> {
        self.client
            .post(format!("{}/api/mixer/source/reorder", self.base_url))
            .json(&ReorderMixerSourceRequest {
                source_id: source_id.into(),
                position,
            })
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn microphone_dsp(&self) -> ClientResult<MicrophoneDspSnapshot> {
        self.get_json("/api/studio/microphone-dsp", self.dsp_read_timeout, false)
    }

    pub fn arm_dsp(&self, module: DspWriteModule, lease_seconds: u64) -> ClientResult<()> {
        self.client
            .post(format!("{}/api/studio/dsp-arm", self.base_url))
            .json(&ArmDspRequest {
                module,
                acknowledgement: "I_UNDERSTAND_THIS_CHANGES_AUDIO",
                lease_seconds,
            })
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn disarm_dsp(&self) -> ClientResult<()> {
        self.client
            .post(format!("{}/api/studio/dsp-disarm", self.base_url))
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn set_microphone_dsp(
        &self,
        update: &MicrophoneDspUpdate,
    ) -> ClientResult<MicrophoneDspWriteResult> {
        Ok(self
            .client
            .post(format!("{}/api/studio/microphone-dsp", self.base_url))
            .json(update)
            .send()?
            .error_for_status()?
            .json()?)
    }

    pub fn set_route(&self, source_id: &str, target_id: &str, enabled: bool) -> ClientResult<()> {
        self.client
            .post(format!("{}/api/mixer/route", self.base_url))
            .json(&SetRouteRequest {
                source_id: source_id.into(),
                target_id: target_id.into(),
                enabled,
            })
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn create_source(&self, name: &str) -> ClientResult<()> {
        self.client
            .post(format!("{}/api/mixer/source", self.base_url))
            .json(&CreateMixerSourceRequest { name: name.into() })
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn remove_source(&self, source_id: &str) -> ClientResult<()> {
        self.client
            .post(format!("{}/api/mixer/source/remove", self.base_url))
            .json(&RemoveMixerSourceRequest {
                source_id: source_id.into(),
            })
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn mixer_profiles(&self) -> ClientResult<MixerProfilesResponse> {
        self.get_json("/api/mixer/profiles", self.read_timeout, true)
    }

    pub fn save_mixer_profile(&self, name: &str) -> ClientResult<()> {
        self.profile_command("save", name)
    }

    pub fn create_mixer_profile(&self) -> ClientResult<()> {
        self.client
            .post(format!("{}/api/mixer/profile/create", self.base_url))
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn duplicate_mixer_profile(&self, name: &str) -> ClientResult<()> {
        self.profile_command("duplicate", name)
    }

    pub fn load_mixer_profile(&self, name: &str) -> ClientResult<()> {
        self.profile_command("load", name)
    }

    pub fn delete_mixer_profile(&self, name: &str) -> ClientResult<()> {
        self.profile_command("delete", name)
    }

    fn profile_command(&self, action: &str, name: &str) -> ClientResult<()> {
        self.client
            .post(format!("{}/api/mixer/profile/{action}", self.base_url))
            .json(&MixerProfileRequest { name: name.into() })
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn set_mixer_application(
        &self,
        process: &str,
        name: &str,
        channel_id: Option<String>,
    ) -> ClientResult<()> {
        self.client
            .post(format!("{}/api/mixer/application", self.base_url))
            .json(&SetMixerApplicationRequest {
                process: process.into(),
                name: name.into(),
                channel_id,
            })
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn set_link_application(
        &self,
        application: &str,
        channel: LinkChannel,
    ) -> ClientResult<()> {
        self.client
            .post(format!("{}/api/studio/link-assignment", self.base_url))
            .json(&SetLinkAssignmentRequest {
                application: application.into(),
                channel,
            })
            .send()?
            .error_for_status()?;
        Ok(())
    }
}

fn retryable_get_error(error: &reqwest::Error, retry_timeouts: bool) -> bool {
    error.is_connect()
        || (retry_timeouts && error.is_timeout())
        || matches!(
            error.status().map(|status| status.as_u16()),
            Some(502..=504)
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    const HEALTH_JSON: &str = r#"{
        "ok": true,
        "studio_mode": "mock",
        "mixer_mode": "mock",
        "hardware_writes_enabled": false,
        "link_control_enabled": false,
        "dsp_write_modules": [],
        "dsp_write_active_module": null,
        "dsp_write_lease_seconds": null
    }"#;

    struct TestResponse {
        status: u16,
        body: &'static str,
        delay: Duration,
    }

    struct TestServer {
        base_url: String,
        attempts: Arc<AtomicUsize>,
    }

    impl TestServer {
        fn start(
            maximum_requests: usize,
            response: impl Fn(usize) -> TestResponse + Send + Sync + 'static,
        ) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let attempts = Arc::new(AtomicUsize::new(0));
            let server_attempts = attempts.clone();
            let response = Arc::new(response);
            thread::spawn(move || {
                for index in 0..maximum_requests {
                    let (mut stream, _) = listener.accept().unwrap();
                    server_attempts.fetch_add(1, Ordering::SeqCst);
                    let response = response(index);
                    thread::spawn(move || {
                        let mut request = [0_u8; 4096];
                        let _ = stream.read(&mut request);
                        thread::sleep(response.delay);
                        let reason = match response.status {
                            200 => "OK",
                            400 => "Bad Request",
                            503 => "Service Unavailable",
                            _ => "Test Status",
                        };
                        let message = format!(
                            "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            response.status,
                            reason,
                            response.body.len(),
                            response.body
                        );
                        let _ = stream.write_all(message.as_bytes());
                    });
                }
            });
            Self {
                base_url: format!("http://{address}"),
                attempts,
            }
        }

        fn client(&self, timeout: Duration) -> DaemonClient {
            DaemonClient::for_test(&self.base_url, timeout).unwrap()
        }

        fn attempts(&self) -> usize {
            self.attempts.load(Ordering::SeqCst)
        }
    }

    #[test]
    fn allow_list_does_not_imply_an_active_dsp_lease() {
        let health: RuntimeHealth = serde_json::from_value(serde_json::json!({
            "ok": true,
            "studio_mode": "mock",
            "mixer_mode": "mock",
            "hardware_writes_enabled": false,
            "link_control_enabled": false,
            "dsp_write_modules": ["equalizer", "enhancement_suite"],
            "dsp_write_active_module": null,
            "dsp_write_lease_seconds": null
        }))
        .unwrap();

        assert_eq!(health.dsp_write_modules.len(), 2);
        assert_eq!(health.dsp_write_active_module, None);
        assert_eq!(health.dsp_write_lease_seconds, None);
    }

    #[test]
    fn production_client_is_fixed_to_ipv4_loopback() {
        assert_eq!(
            DaemonClient::localhost().unwrap().base_url,
            LOCAL_DAEMON_URL
        );
    }

    #[test]
    fn offline_refusal_has_stable_classification_and_diagnostic() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let client =
            DaemonClient::for_test(&format!("http://{address}"), Duration::from_secs(1)).unwrap();

        let error = client.health().unwrap_err();
        assert_eq!(error.kind(), DaemonErrorKind::Offline);
        assert_eq!(
            error.to_string(),
            "StudioBridge daemon is offline on localhost"
        );
    }

    #[test]
    fn stalled_get_is_bounded_and_retried_once() {
        let server = TestServer::start(2, |_| TestResponse {
            status: 200,
            body: HEALTH_JSON,
            delay: Duration::from_millis(100),
        });

        let error = server
            .client(Duration::from_millis(20))
            .health()
            .unwrap_err();
        assert_eq!(error.kind(), DaemonErrorKind::Timeout);
        assert_eq!(error.to_string(), "StudioBridge daemon request timed out");
        assert_eq!(server.attempts(), 2);
    }

    #[test]
    fn expensive_dsp_read_timeout_is_not_automatically_replayed() {
        let server = TestServer::start(1, |_| TestResponse {
            status: 200,
            body: "{}",
            delay: Duration::from_millis(100),
        });

        let error = server
            .client(Duration::from_millis(20))
            .microphone_dsp()
            .unwrap_err();
        assert_eq!(error.kind(), DaemonErrorKind::Timeout);
        assert_eq!(server.attempts(), 1);
    }

    #[test]
    fn non_transient_http_error_is_not_retried() {
        let server = TestServer::start(1, |_| TestResponse {
            status: 400,
            body: r#"{"error":"bad request"}"#,
            delay: Duration::ZERO,
        });

        let error = server.client(Duration::from_secs(1)).health().unwrap_err();
        assert_eq!(error.kind(), DaemonErrorKind::HttpStatus(400));
        assert_eq!(error.to_string(), "StudioBridge daemon returned HTTP 400");
        assert_eq!(server.attempts(), 1);
    }

    #[test]
    fn malformed_json_is_classified_without_retry() {
        let server = TestServer::start(1, |_| TestResponse {
            status: 200,
            body: "{not-json",
            delay: Duration::ZERO,
        });

        let error = server.client(Duration::from_secs(1)).health().unwrap_err();
        assert_eq!(error.kind(), DaemonErrorKind::InvalidResponse);
        assert_eq!(
            error.to_string(),
            "StudioBridge daemon returned an invalid JSON response"
        );
        assert_eq!(server.attempts(), 1);
    }

    #[test]
    fn transient_get_status_is_retried_once_then_succeeds() {
        let server = TestServer::start(2, |index| TestResponse {
            status: if index == 0 { 503 } else { 200 },
            body: if index == 0 {
                r#"{"error":"starting"}"#
            } else {
                HEALTH_JSON
            },
            delay: Duration::ZERO,
        });

        let health = server.client(Duration::from_secs(1)).health().unwrap();
        assert!(health.ok);
        assert_eq!(server.attempts(), 2);
    }

    #[test]
    fn mutating_post_is_single_attempt_even_for_transient_status() {
        let server = TestServer::start(1, |_| TestResponse {
            status: 503,
            body: r#"{"error":"starting"}"#,
            delay: Duration::ZERO,
        });

        let error = server
            .client(Duration::from_secs(1))
            .set_volume("game", MixBus::Personal, 42)
            .unwrap_err();
        assert_eq!(error.kind(), DaemonErrorKind::HttpStatus(503));
        assert_eq!(server.attempts(), 1);
    }
}
