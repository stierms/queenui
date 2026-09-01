use futures_util::StreamExt;
use queen_core::models::AppSnapshot;
use queen_protocol::{
    command_body_digest, CommandRequest, CommandResponse, EngineBrowseRequest,
    EngineBrowseResponse, EngineRoot, EventEnvelope, HandoverInventory, PairRedeemRequest,
    PairRedeemResponse, RunnerCapabilities, RunnerCommand, RunnerIdentity, SnapshotResponse,
    CAMPAIGN_COMPLETED_GAME_LIMIT_FEATURE, CAMPAIGN_SCHEDULING_FEATURE, CONTENT_SHA256_HEADER,
    PAIRING_PAYLOAD_VERSION, PROTOCOL_VERSION,
};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{verify_tls12_signature, verify_tls13_signature, WebPkiSupportedAlgorithms},
    pki_types::{CertificateDer, ServerName, UnixTime},
    CertificateError, ClientConfig, DigitallySignedStruct, Error as TlsError, SignatureScheme,
};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::{fmt, sync::Arc, time::Duration};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    connect_async_tls_with_config,
    tungstenite::{client::IntoClientRequest, Message},
    Connector, MaybeTlsStream, WebSocketStream,
};
use url::{Host, Url};
use uuid::Uuid;

#[derive(Clone)]
pub struct RunnerClient {
    base_url: Arc<str>,
    token: Arc<str>,
    generation: u64,
    http: reqwest::Client,
    tls: Option<Arc<ClientConfig>>,
}

pub struct RunnerEventStream {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    instance_id: Option<Uuid>,
    last_sequence: Option<u64>,
}

impl RunnerClient {
    pub fn from_identity(identity: RunnerIdentity) -> Result<Self, String> {
        if identity.version != PAIRING_PAYLOAD_VERSION {
            return Err("The saved runner identity has an unsupported version; pair again".into());
        }
        let endpoint = canonical_endpoint(&identity.url)?;
        if identity.bearer.trim().len() < 32 {
            return Err("The saved runner bearer is invalid; pair again".into());
        }
        let tls = if endpoint.starts_with("https://") {
            Some(pinned_tls_config(parse_fingerprint(&identity.cert_fp)?))
        } else {
            None
        };
        let http = runner_http_client(tls.clone())?;
        Ok(Self {
            base_url: endpoint.into(),
            token: identity.bearer.trim().to_string().into(),
            generation: identity.generation,
            http,
            tls,
        })
    }

    /// Cleartext is permitted only for a literal loopback endpoint. This is
    /// intended for an already authenticated SSH tunnel; names such as
    /// `localhost` never qualify and there is no remote-HTTP override.
    pub fn from_loopback(base_url: &str, bearer: String, generation: u64) -> Result<Self, String> {
        let endpoint = canonical_endpoint(base_url)?;
        if !endpoint.starts_with("http://") {
            return Err("Loopback identities must use http".into());
        }
        Self::from_identity(RunnerIdentity {
            version: PAIRING_PAYLOAD_VERSION,
            url: endpoint,
            cert_fp: String::new(),
            bearer,
            generation,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn canonical_endpoint(base_url: &str) -> Result<String, String> {
        canonical_endpoint(base_url)
    }

    pub fn identity_generation(&self) -> u64 {
        self.generation
    }

    pub async fn capabilities(&self) -> Result<RunnerCapabilities, String> {
        let capabilities: RunnerCapabilities = self.get("/v2/capabilities").await?;
        self.check_version(capabilities.protocol_version)?;
        Ok(capabilities)
    }

    pub async fn snapshot(&self) -> Result<AppSnapshot, String> {
        let response: SnapshotResponse = self.get("/v2/snapshot").await?;
        self.check_version(response.protocol_version)?;
        Ok(response.snapshot)
    }

    pub async fn handover_inventory(&self) -> Result<HandoverInventory, String> {
        self.command(RunnerCommand::HandoverInventory).await
    }

    pub async fn engine_roots(&self) -> Result<Vec<EngineRoot>, String> {
        self.get("/v2/engines/roots").await
    }

    pub async fn browse_engines(
        &self,
        request: EngineBrowseRequest,
    ) -> Result<EngineBrowseResponse, String> {
        let response = self
            .http
            .post(self.url("/v2/engines/browse"))
            .bearer_auth(&*self.token)
            .json(&request)
            .send()
            .await
            .map_err(|error| format!("Could not reach the runner: {error}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!(
                "The runner rejected the scoped engine browse with HTTP {status}"
            ));
        }
        response
            .json()
            .await
            .map_err(|error| format!("Runner returned unreadable browser data: {error}"))
    }

    pub async fn command<T: DeserializeOwned>(&self, command: RunnerCommand) -> Result<T, String> {
        let required_features = command_required_features(&command);
        if !required_features.is_empty() {
            let capabilities = self.capabilities().await?;
            for required_feature in required_features {
                if !capabilities
                    .features
                    .iter()
                    .any(|feature| feature == required_feature)
                {
                    return Err(match *required_feature {
                        CAMPAIGN_COMPLETED_GAME_LIMIT_FEATURE => {
                            "Upgrade the paired runner before using a completed-game campaign limit; existing runner features remain available"
                                .into()
                        }
                        _ => {
                            "Upgrade the paired runner before using incoming challenge acceptance or automatic campaign limits; existing runner features remain available"
                                .into()
                        }
                    });
                }
            }
        }
        let request_id = Uuid::new_v4();
        let request = CommandRequest {
            request_id,
            command,
        };
        let body = serde_json::to_vec(&request)
            .map_err(|_| "Could not encode the runner command".to_string())?;
        let digest = command_body_digest(&body);
        let mut transport_error = None;
        let pending_deadline = tokio::time::Instant::now() + Duration::from_secs(2 * 60);
        let response = loop {
            match self
                .http
                .post(self.url("/v2/commands"))
                .bearer_auth(&*self.token)
                .header(CONTENT_SHA256_HEADER, encode_hex(&digest))
                .header(CONTENT_TYPE, "application/json")
                .body(body.clone())
                .send()
                .await
            {
                Ok(response) if response.status() == reqwest::StatusCode::ACCEPTED => {
                    if tokio::time::Instant::now() >= pending_deadline {
                        return Err(format!(
                            "Runner command {request_id} is still pending; retry with the same key"
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
                Ok(response) => break response,
                Err(error) if transport_error.is_none() => {
                    transport_error = Some(error);
                }
                Err(error) => {
                    return Err(format!("Could not reach the runner: {error}"));
                }
            }
        };
        self.decode_command_response(response, request_id).await
    }

    pub async fn events(&self) -> Result<RunnerEventStream, String> {
        let websocket_url = self.websocket_url()?;
        let mut request = websocket_url
            .as_str()
            .into_client_request()
            .map_err(|error| format!("Could not build the runner event request: {error}"))?;
        request.headers_mut().insert(
            AUTHORIZATION,
            format!("Bearer {}", self.token)
                .parse()
                .map_err(|_| "Runner token is not a valid HTTP header value".to_string())?,
        );
        let connector = self.tls.clone().map(Connector::Rustls);
        let (socket, response) = connect_async_tls_with_config(request, None, false, connector)
            .await
            .map_err(|error| format!("Could not connect to runner events: {error}"))?;
        if response.status() != 101 {
            return Err(format!(
                "Runner rejected the event stream with HTTP {}",
                response.status()
            ));
        }
        Ok(RunnerEventStream {
            socket,
            instance_id: None,
            last_sequence: None,
        })
    }

    pub async fn log_export(&self, session_id: &str, mode: &str) -> Result<Vec<u8>, String> {
        let response = self
            .http
            .get(self.url(&format!("/v2/logs/{session_id}/export/{mode}")))
            .bearer_auth(&*self.token)
            .send()
            .await
            .map_err(|error| format!("Could not reach the runner: {error}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!(
                "Runner returned HTTP {status} while exporting the log"
            ));
        }
        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|error| format!("Could not download the runner log: {error}"))
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let response = self
            .http
            .get(self.url(path))
            .bearer_auth(&*self.token)
            .send()
            .await
            .map_err(|error| format!("Could not reach the runner: {error}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("Runner returned HTTP {status}"));
        }
        response
            .json()
            .await
            .map_err(|error| format!("Runner returned unreadable JSON: {error}"))
    }

    async fn decode_command_response<T: DeserializeOwned>(
        &self,
        response: reqwest::Response,
        request_id: Uuid,
    ) -> Result<T, String> {
        let status = response.status();
        if !status.is_success() {
            return Err(format!("Runner returned HTTP {status}"));
        }
        let response: CommandResponse = response.json().await.map_err(|error| {
            format!("Runner returned an unreadable response ({status}): {error}")
        })?;
        self.check_version(response.protocol_version)?;
        if response.request_id != request_id {
            return Err("Runner response did not match the command request".into());
        }
        if !response.ok {
            return Err(response
                .error
                .map(|error| error.message)
                .unwrap_or_else(|| "Runner command failed without an error message".into()));
        }
        serde_json::from_value(response.result.unwrap_or(serde_json::Value::Null))
            .map_err(|error| format!("Could not decode the runner command result: {error}"))
    }

    fn check_version(&self, received: u32) -> Result<(), String> {
        if received != PROTOCOL_VERSION {
            return Err(format!(
                "Runner protocol {received} is incompatible with desktop protocol {PROTOCOL_VERSION}"
            ));
        }
        Ok(())
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    fn websocket_url(&self) -> Result<Url, String> {
        let mut url = Url::parse(&self.url("/v2/events"))
            .map_err(|error| format!("Invalid runner event URL: {error}"))?;
        url.set_scheme(if url.scheme() == "https" { "wss" } else { "ws" })
            .map_err(|_| "Could not select the runner WebSocket scheme".to_string())?;
        Ok(url)
    }
}

fn command_required_features(command: &RunnerCommand) -> &'static [&'static str] {
    const NONE: &[&str] = &[];
    const SCHEDULING: &[&str] = &[CAMPAIGN_SCHEDULING_FEATURE];
    const COMPLETED_GAME_LIMIT: &[&str] = &[
        CAMPAIGN_SCHEDULING_FEATURE,
        CAMPAIGN_COMPLETED_GAME_LIMIT_FEATURE,
    ];
    match command {
        RunnerCommand::StartCampaign { settings } if settings.stop_after_games.is_some() => {
            COMPLETED_GAME_LIMIT
        }
        RunnerCommand::StartCampaign { settings }
            if settings.accept_incoming_challenges || settings.stop_after_minutes.is_some() =>
        {
            SCHEDULING
        }
        _ => NONE,
    }
}

/// Redeem a v2 in-app pairing payload entirely in Rust. The enrollment value
/// is placed in the HTTP body only after rustls has accepted the exact imported
/// certificate fingerprint.
pub async fn redeem_pairing_payload(payload: &str) -> Result<RunnerIdentity, String> {
    let parsed = PairingPayload::parse(payload)?;
    let tls = pinned_tls_config(parsed.cert_fp);
    let http = runner_http_client(Some(tls))?;
    let response = http
        .post(format!("{}/v2/pair/redeem", parsed.url))
        .json(&PairRedeemRequest {
            enroll: parsed.enroll,
        })
        .send()
        .await
        .map_err(|error| format!("Could not establish the pinned runner connection: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Runner rejected the enrollment with HTTP {}",
            response.status()
        ));
    }
    let redeemed: PairRedeemResponse = response
        .json()
        .await
        .map_err(|_| "Runner returned an unreadable enrollment response".to_string())?;
    if redeemed.protocol_version != PROTOCOL_VERSION {
        return Err("Runner enrollment used an incompatible protocol version".into());
    }
    Ok(RunnerIdentity {
        version: PAIRING_PAYLOAD_VERSION,
        url: parsed.url,
        cert_fp: encode_hex(&parsed.cert_fp),
        bearer: redeemed.bearer,
        generation: redeemed.generation,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PairingPayload {
    url: String,
    cert_fp: [u8; 32],
    enroll: String,
}

impl PairingPayload {
    fn parse(payload: &str) -> Result<Self, String> {
        let uri =
            Url::parse(payload.trim()).map_err(|_| "The pairing payload is invalid".to_string())?;
        if uri.scheme() != "queenui"
            || uri.host_str() != Some("pair")
            || !matches!(uri.path(), "" | "/")
        {
            return Err("Pairing payloads must use queenui://pair".into());
        }
        if uri.username() != "" || uri.password().is_some() || uri.fragment().is_some() {
            return Err("The pairing payload contains unsupported fields".into());
        }
        let mut version = None;
        let mut url = None;
        let mut cert_fp = None;
        let mut enroll = None;
        for (key, value) in uri.query_pairs() {
            let slot = match key.as_ref() {
                "v" => &mut version,
                "url" => &mut url,
                "fp" => &mut cert_fp,
                "enroll" => &mut enroll,
                _ => return Err("The pairing payload contains an unknown field".into()),
            };
            if slot.replace(value.into_owned()).is_some() {
                return Err("The pairing payload repeats a field".into());
            }
        }
        let version = version.ok_or_else(|| "The pairing payload has no version".to_string())?;
        if version.parse::<u32>().ok() != Some(PAIRING_PAYLOAD_VERSION) {
            return Err(format!(
                "Pairing payload version {version} is unsupported; expected v{PAIRING_PAYLOAD_VERSION}"
            ));
        }
        let url = canonical_endpoint(
            &url.ok_or_else(|| "The pairing payload has no runner URL".to_string())?,
        )?;
        if !url.starts_with("https://") {
            return Err("Enrollment redemption requires pinned HTTPS".into());
        }
        let cert_fp = parse_fingerprint(
            &cert_fp.ok_or_else(|| "The pairing payload has no certificate pin".to_string())?,
        )?;
        let enroll =
            enroll.ok_or_else(|| "The pairing payload has no enrollment code".to_string())?;
        if enroll.len() < 43 || enroll.len() > 128 {
            return Err("The pairing payload contains an invalid enrollment code".into());
        }
        Ok(Self {
            url,
            cert_fp,
            enroll,
        })
    }
}

fn canonical_endpoint(base_url: &str) -> Result<String, String> {
    let mut parsed =
        Url::parse(base_url.trim()).map_err(|error| format!("Invalid runner URL: {error}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("Runner URLs must use http or https".into());
    }
    if parsed.host_str().is_none()
        || parsed.username() != ""
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(
            "Runner URL must contain only scheme, host, port, and optional base path".into(),
        );
    }
    if parsed.scheme() == "http" && !is_literal_loopback(&parsed) {
        return Err("Cleartext runner URLs are allowed only for literal loopback addresses".into());
    }
    let default_port = match parsed.scheme() {
        "http" => 80,
        "https" => 443,
        _ => unreachable!(),
    };
    if parsed.port() == Some(default_port) {
        parsed
            .set_port(None)
            .map_err(|_| "Could not normalize the runner port".to_string())?;
    }
    let normalized_path = parsed.path().trim_end_matches('/').to_string();
    parsed.set_path(if normalized_path.is_empty() {
        "/"
    } else {
        &normalized_path
    });
    Ok(parsed.as_str().trim_end_matches('/').to_string())
}

fn is_literal_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        Some(Host::Domain(_)) | None => false,
    }
}

fn runner_http_client(tls: Option<Arc<ClientConfig>>) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(35))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy();
    if let Some(config) = tls {
        // reqwest accepts an owned preconfigured rustls value while WSS takes
        // an Arc. Both are clones of this one config and exact verifier.
        builder = builder.tls_backend_preconfigured(config.as_ref().clone());
    }
    builder
        .build()
        .map_err(|error| format!("Could not initialize the runner client: {error}"))
}

fn pinned_tls_config(fingerprint: [u8; 32]) -> Arc<ClientConfig> {
    let provider = rustls::crypto::aws_lc_rs::default_provider();
    let algorithms = provider.signature_verification_algorithms;
    let config = ClientConfig::builder_with_provider(Arc::new(provider))
        .with_safe_default_protocol_versions()
        .expect("the rustls provider supports its default protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(ExactCertificateVerifier {
            fingerprint,
            algorithms,
        }))
        .with_no_client_auth();
    Arc::new(config)
}

#[derive(Clone)]
struct ExactCertificateVerifier {
    fingerprint: [u8; 32],
    algorithms: WebPkiSupportedAlgorithms,
}

impl fmt::Debug for ExactCertificateVerifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ExactCertificateVerifier(<redacted pin>)")
    }
}

impl ServerCertVerifier for ExactCertificateVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        let actual: [u8; 32] = Sha256::digest(end_entity.as_ref()).into();
        if actual == self.fingerprint {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(TlsError::InvalidCertificate(
                CertificateError::UnknownIssuer,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

fn parse_fingerprint(value: &str) -> Result<[u8; 32], String> {
    let value = value.trim();
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Runner certificate fingerprints must be 64 hexadecimal characters".into());
    }
    let mut output = [0u8; 32];
    for (index, byte) in output.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| "Runner certificate fingerprint is invalid".to_string())?;
    }
    Ok(output)
}

fn encode_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

impl RunnerEventStream {
    pub async fn next(&mut self) -> Result<Option<EventEnvelope>, String> {
        while let Some(message) = self.socket.next().await {
            match message.map_err(|error| format!("Runner event stream failed: {error}"))? {
                Message::Text(text) => {
                    let event: EventEnvelope = serde_json::from_str(&text)
                        .map_err(|error| format!("Runner sent an unreadable event: {error}"))?;
                    if event.protocol_version != PROTOCOL_VERSION {
                        return Err(format!(
                            "Runner event protocol {} is incompatible with desktop protocol {}",
                            event.protocol_version, PROTOCOL_VERSION
                        ));
                    }
                    if self
                        .instance_id
                        .is_some_and(|instance_id| instance_id != event.instance_id)
                    {
                        return Err("Runner instance changed within one event stream".into());
                    }
                    if self
                        .last_sequence
                        .is_some_and(|sequence| event.sequence < sequence)
                    {
                        return Err("Runner event sequence moved backwards".into());
                    }
                    self.instance_id = Some(event.instance_id);
                    self.last_sequence = Some(event.sequence);
                    return Ok(Some(event));
                }
                Message::Close(_) => return Ok(None),
                Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {}
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        command_required_features, encode_hex, redeem_pairing_payload, PairingPayload, RunnerClient,
    };
    use queen_core::models::CampaignSettings;
    use queen_protocol::{
        RunnerCommand, RunnerIdentity, CAMPAIGN_COMPLETED_GAME_LIMIT_FEATURE,
        CAMPAIGN_SCHEDULING_FEATURE, PAIRING_PAYLOAD_VERSION,
    };
    use rcgen::{generate_simple_self_signed, CertifiedKey};
    use rustls::{pki_types::PrivateKeyDer, ServerConfig};
    use sha2::{Digest, Sha256};
    use std::{sync::Arc, time::Duration};
    use tokio::{io::AsyncReadExt, net::TcpListener, task::JoinHandle};
    use tokio_rustls::TlsAcceptor;
    use url::Url;

    #[test]
    fn canonical_url_identity_and_literal_loopback_policy() {
        assert_eq!(
            RunnerClient::canonical_endpoint("HTTPS://BÜCHER.example:443/base/").unwrap(),
            "https://xn--bcher-kva.example/base"
        );
        assert_eq!(
            RunnerClient::canonical_endpoint("http://127.9.8.7:7788/").unwrap(),
            "http://127.9.8.7:7788"
        );
        assert_eq!(
            RunnerClient::canonical_endpoint("http://[::1]:80").unwrap(),
            "http://[::1]"
        );
        assert!(RunnerClient::canonical_endpoint("http://localhost:7788").is_err());
        assert!(RunnerClient::canonical_endpoint("http://runner:7788").is_err());
        assert!(RunnerClient::canonical_endpoint("https://user@runner").is_err());
        assert!(RunnerClient::canonical_endpoint("https://runner?token=bad").is_err());
        assert!(RunnerClient::canonical_endpoint("ftp://runner").is_err());
    }

    #[test]
    fn pairing_parser_accepts_only_v2_in_app_payloads() {
        let mut valid = Url::parse("queenui://pair").unwrap();
        valid
            .query_pairs_mut()
            .append_pair("v", "2")
            .append_pair("url", "https://runner.example:443/")
            .append_pair("fp", &"ab".repeat(32))
            .append_pair("enroll", &"e".repeat(43));
        let parsed = PairingPayload::parse(valid.as_str()).unwrap();
        assert_eq!(parsed.url, "https://runner.example");

        for invalid in [
            valid.as_str().replace("v=2", "v=1"),
            valid.as_str().replace("v=2", "v=3"),
            valid.as_str().replace("queenui://", "https://"),
            format!("{}&token=legacy-bearer", valid.as_str()),
        ] {
            assert!(
                PairingPayload::parse(&invalid).is_err(),
                "accepted {invalid}"
            );
        }
    }

    #[test]
    fn only_additive_campaign_controls_require_the_new_runner_feature() {
        let legacy = RunnerCommand::StartCampaign {
            settings: CampaignSettings::default(),
        };
        assert_eq!(command_required_features(&legacy), &[] as &[&str]);

        let settings = CampaignSettings {
            accept_incoming_challenges: true,
            ..CampaignSettings::default()
        };
        let incoming = RunnerCommand::StartCampaign { settings };
        assert_eq!(
            command_required_features(&incoming),
            &[CAMPAIGN_SCHEDULING_FEATURE]
        );

        let settings = CampaignSettings {
            stop_after_games: Some(30),
            ..CampaignSettings::default()
        };
        let completed_limit = RunnerCommand::StartCampaign { settings };
        assert_eq!(
            command_required_features(&completed_limit),
            &[
                CAMPAIGN_SCHEDULING_FEATURE,
                CAMPAIGN_COMPLETED_GAME_LIMIT_FEATURE,
            ]
        );
    }

    #[tokio::test]
    async fn wrong_certificate_receives_zero_enrollment_bytes_over_http() {
        let (url, received, server) = wrong_certificate_server().await;
        let mut payload = Url::parse("queenui://pair").unwrap();
        payload
            .query_pairs_mut()
            .append_pair("v", &PAIRING_PAYLOAD_VERSION.to_string())
            .append_pair("url", &url)
            .append_pair("fp", &"11".repeat(32))
            .append_pair(
                "enroll",
                &"enrollment-secret-never-sent-before-pin!".repeat(2),
            );
        assert!(redeem_pairing_payload(payload.as_str()).await.is_err());
        server.await.unwrap();
        assert_eq!(received.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn changed_pin_never_receives_old_bearer_over_wss() {
        let (url, received, server) = wrong_certificate_server().await;
        let client = RunnerClient::from_identity(RunnerIdentity {
            version: PAIRING_PAYLOAD_VERSION,
            url,
            cert_fp: "22".repeat(32),
            bearer: "bearer-secret-never-sent-before-pin-check".into(),
            generation: 1,
        })
        .unwrap();
        assert!(client.events().await.is_err());
        server.await.unwrap();
        assert_eq!(received.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    async fn wrong_certificate_server(
    ) -> (String, Arc<std::sync::atomic::AtomicUsize>, JoinHandle<()>) {
        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
        let certificate = cert.der().clone();
        let _actual_fingerprint = encode_hex(&Sha256::digest(certificate.as_ref()));
        let key = PrivateKeyDer::try_from(signing_key.serialize_der()).unwrap();
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![certificate], key)
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let received = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let task_received = received.clone();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            if let Ok(Ok(mut stream)) = tokio::time::timeout(
                Duration::from_secs(5),
                TlsAcceptor::from(Arc::new(config)).accept(stream),
            )
            .await
            {
                let mut application = Vec::new();
                let _ = tokio::time::timeout(
                    Duration::from_secs(1),
                    stream.read_to_end(&mut application),
                )
                .await;
                task_received.store(application.len(), std::sync::atomic::Ordering::SeqCst);
            }
        });
        (format!("https://{address}"), received, server)
    }
}
