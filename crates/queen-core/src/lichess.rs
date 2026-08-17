use crate::models::{ChallengeRequest, ChallengeResult, LichessAccount, OnlineBot};
use futures_util::StreamExt;
use reqwest::{
    header::{HeaderMap, RETRY_AFTER},
    Client, Method, Response, StatusCode, Url,
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{fmt, time::Duration};

const API: &str = "https://lichess.org/api";
const ERROR_BODY_CAP: usize = 8 * 1024;
const JSON_BODY_CAP: usize = 1024 * 1024;
pub const NDJSON_LINE_CAP: usize = 256 * 1024;
const ONLINE_BOTS_BODY_CAP: usize = 16 * 1024 * 1024;
const GAME_EXPORT_BODY_CAP: usize = 64 * 1024 * 1024;
const OAUTH_SCOPES_HEADER: &str = "x-oauth-scopes";
pub const MATCHMAKING_SCOPES: [&str; 3] = ["bot:play", "challenge:read", "challenge:write"];
pub const TOKEN_CREATE_URL: &str = "lichess.org/account/oauth/token/create";

#[derive(Clone, Debug)]
pub struct ValidatedAccount {
    pub account: LichessAccount,
    pub scopes: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LichessErrorKind {
    Transport,
    Http,
    Decode,
    Limit,
}

/// A bounded, typed Lichess failure. Decisions are made from status/code and
/// Retry-After; the body is retained only as a short diagnostic and is never
/// searched to infer general HTTP classes.
#[derive(Clone, Debug)]
pub struct LichessError {
    pub kind: LichessErrorKind,
    pub status: Option<StatusCode>,
    pub code: Option<String>,
    pub retry_after: Option<Duration>,
    pub body: String,
    pub operation: &'static str,
    pub ambiguous_write: bool,
}

impl LichessError {
    fn transport(operation: &'static str, error: reqwest::Error, write: bool) -> Self {
        Self {
            kind: LichessErrorKind::Transport,
            status: error.status(),
            code: None,
            retry_after: None,
            body: error.to_string().chars().take(ERROR_BODY_CAP).collect(),
            operation,
            // Once a non-idempotent request begins, a timeout/disconnect cannot
            // prove whether the server committed it.
            ambiguous_write: write,
        }
    }

    fn decode(operation: &'static str, detail: impl Into<String>) -> Self {
        Self {
            kind: LichessErrorKind::Decode,
            status: None,
            code: None,
            retry_after: None,
            body: detail.into().chars().take(ERROR_BODY_CAP).collect(),
            operation,
            ambiguous_write: false,
        }
    }

    fn limit(operation: &'static str, detail: impl Into<String>) -> Self {
        Self {
            kind: LichessErrorKind::Limit,
            status: None,
            code: Some("response_too_large".into()),
            retry_after: None,
            body: detail.into().chars().take(ERROR_BODY_CAP).collect(),
            operation,
            ambiguous_write: false,
        }
    }

    pub fn is_rate_limited(&self) -> bool {
        self.status == Some(StatusCode::TOO_MANY_REQUESTS)
    }

    pub fn is_auth_failure(&self) -> bool {
        matches!(
            self.status,
            Some(StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
        )
    }

    pub fn is_not_found(&self) -> bool {
        self.status == Some(StatusCode::NOT_FOUND)
    }

    pub fn is_server_error(&self) -> bool {
        self.status.is_some_and(|status| status.is_server_error())
    }

    pub fn missing_scope(&self) -> Option<String> {
        if self.status != Some(StatusCode::FORBIDDEN) {
            return None;
        }
        let value: Value = serde_json::from_str(&self.body).ok()?;
        let scope = value
            .get("error")?
            .as_str()?
            .trim()
            .strip_prefix("Missing scope:")?
            .trim();
        (!scope.is_empty()).then(|| scope.to_string())
    }
}

impl fmt::Display for LichessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} failed", self.operation)?;
        if let Some(status) = self.status {
            write!(formatter, " with HTTP {status}")?;
        }
        if let Some(code) = &self.code {
            write!(formatter, " ({code})")?;
        }
        if !self.body.trim().is_empty() {
            write!(formatter, ": {}", self.body.trim())?;
        }
        if let Some(delay) = self.retry_after {
            write!(formatter, " [retry after {}s]", delay.as_secs())?;
        }
        Ok(())
    }
}

impl std::error::Error for LichessError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutgoingChallenge {
    pub id: String,
    pub opponent: String,
    pub status: String,
}

/// Counts the NDJSON entries a response handler had to skip, so one broken
/// stream produces a single diagnostic instead of one per line.
#[derive(Default)]
struct Skipped {
    count: u64,
    first: Option<String>,
}

impl Skipped {
    fn record(&mut self, error: serde_json::Error) {
        self.count += 1;
        self.first.get_or_insert_with(|| error.to_string());
    }

    fn report(self, singular: &str, plural: &str) {
        if self.count == 0 {
            return;
        }
        let noun = if self.count == 1 { singular } else { plural };
        let mut entry = crate::diagnostics::DiagnosticEntry::warn(
            "lichess",
            format!("Skipped {} unreadable {noun}", self.count),
        );
        if let Some(first) = self.first {
            entry = entry.with_detail(first);
        }
        crate::diagnostics::record(entry);
    }
}

pub(crate) fn default_api_base() -> Result<Url, String> {
    Url::parse(API).map_err(|error| format!("Could not initialize the Lichess API URL: {error}"))
}

fn api_url(base: &Url, segments: &[&str]) -> Result<Url, LichessError> {
    let mut url = base.clone();
    url.path_segments_mut()
        .map_err(|_| {
            LichessError::decode("build URL", "Lichess API URL cannot accept path segments")
        })?
        .extend(segments.iter().copied());
    Ok(url)
}

/// Builds a browser-facing Lichess resource URL without allowing an ID to
/// become path syntax. This is intentionally shared with callers so display
/// URLs follow the same encoding rule as API URLs.
pub fn site_url(id: &str) -> Result<String, LichessError> {
    let mut url = Url::parse("https://lichess.org")
        .map_err(|error| LichessError::decode("build site URL", error.to_string()))?;
    url.path_segments_mut()
        .map_err(|_| {
            LichessError::decode("build site URL", "Lichess URL cannot accept path segments")
        })?
        .push(id);
    Ok(url.into())
}

fn request(client: &Client, method: Method, url: Url, token: &str) -> reqwest::RequestBuilder {
    client
        .request(method, url)
        .bearer_auth(token)
        .header("Accept", "application/x-ndjson, application/json")
        .header("User-Agent", "QueenUI/0.1 (Lichess bot manager)")
}

fn parse_retry_after(value: Option<&reqwest::header::HeaderValue>) -> Option<Duration> {
    let raw = value?.to_str().ok()?.trim();
    if let Ok(seconds) = raw.parse::<u64>() {
        return Some(Duration::from_secs(seconds.min(24 * 60 * 60)));
    }
    let at = httpdate::parse_http_date(raw).ok()?;
    Some(
        at.duration_since(std::time::SystemTime::now())
            .unwrap_or_default()
            .min(Duration::from_secs(24 * 60 * 60)),
    )
}

fn oauth_scopes(headers: &HeaderMap) -> Vec<String> {
    let mut scopes = Vec::new();
    for value in headers.get_all(OAUTH_SCOPES_HEADER) {
        let Ok(value) = value.to_str() else {
            continue;
        };
        for scope in value.split(|character: char| character == ',' || character.is_whitespace()) {
            let scope = scope.trim();
            if !scope.is_empty() && !scopes.iter().any(|known| known == scope) {
                scopes.push(scope.to_string());
            }
        }
    }
    scopes
}

pub fn missing_matchmaking_scopes(scopes: &[String]) -> Vec<String> {
    MATCHMAKING_SCOPES
        .iter()
        .filter(|required| !scopes.iter().any(|scope| scope == *required))
        .map(|scope| (*scope).to_string())
        .collect()
}

pub fn actionable_missing_scope_message(error: &LichessError) -> Option<String> {
    let scope = error.missing_scope()?;
    if scope == "bot:play" {
        Some(format!(
            "QueenUI cannot play games because this Lichess token is missing scope {scope}; create a new token at {TOKEN_CREATE_URL} with Play-bot, Read-challenges, and Send-challenges ticked—the current token cannot continue games."
        ))
    } else {
        Some(format!(
            "Matchmaking is paused because this Lichess token is missing scope {scope}; create a new token at {TOKEN_CREATE_URL} with Play-bot, Read-challenges, and Send-challenges ticked—games continue with the current token."
        ))
    }
}

async fn checked(response: Response, operation: &'static str) -> Result<Response, LichessError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let retry_after = parse_retry_after(response.headers().get(RETRY_AFTER));
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    let mut truncated = false;
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                if bytes.is_empty() {
                    bytes.extend_from_slice(error.to_string().as_bytes());
                }
                break;
            }
        };
        let remaining = ERROR_BODY_CAP.saturating_sub(bytes.len());
        if chunk.len() > remaining {
            bytes.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        bytes.extend_from_slice(&chunk);
    }
    let mut body = String::from_utf8_lossy(&bytes).trim().to_string();
    if truncated {
        body.push_str("… [truncated]");
    }
    let value = serde_json::from_slice::<Value>(&bytes).ok();
    let code = value.as_ref().and_then(|value| {
        value
            .pointer("/error/code")
            .or_else(|| value.get("code"))
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    Err(LichessError {
        kind: LichessErrorKind::Http,
        status: Some(status),
        code,
        retry_after,
        body,
        operation,
        ambiguous_write: false,
    })
}

async fn bounded_bytes(
    response: Response,
    operation: &'static str,
    cap: usize,
) -> Result<Vec<u8>, LichessError> {
    if response
        .content_length()
        .is_some_and(|length| length > cap as u64)
    {
        return Err(LichessError::limit(
            operation,
            format!("response exceeds the {cap}-byte safety limit"),
        ));
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| LichessError::transport(operation, error, false))?;
        if bytes.len().saturating_add(chunk.len()) > cap {
            return Err(LichessError::limit(
                operation,
                format!("response exceeds the {cap}-byte safety limit"),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn response_json<T: DeserializeOwned>(
    response: Response,
    operation: &'static str,
) -> Result<T, LichessError> {
    let bytes = bounded_bytes(response, operation, JSON_BODY_CAP).await?;
    serde_json::from_slice(&bytes)
        .map_err(|error| LichessError::decode(operation, format!("invalid JSON: {error}")))
}

/// Splits every complete newline-terminated line out of an NDJSON byte buffer,
/// leaving any trailing partial line in place for the next chunk.
pub fn take_lines(buffer: &mut Vec<u8>) -> Vec<String> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (index, byte) in buffer.iter().enumerate() {
        if *byte == b'\n' {
            let line = String::from_utf8_lossy(&buffer[start..index])
                .trim_end_matches('\r')
                .to_string();
            lines.push(line);
            start = index + 1;
        }
    }
    if start > 0 {
        buffer.drain(..start);
    }
    lines
}

/// Adds a stream chunk while enforcing both total-body and partial-line caps.
pub fn append_ndjson_chunk(
    buffer: &mut Vec<u8>,
    chunk: &[u8],
    received: &mut usize,
    body_cap: usize,
    operation: &'static str,
) -> Result<Vec<String>, LichessError> {
    *received = received
        .checked_add(chunk.len())
        .ok_or_else(|| LichessError::limit(operation, "response size overflowed"))?;
    if *received > body_cap {
        return Err(LichessError::limit(
            operation,
            format!("response exceeds the {body_cap}-byte safety limit"),
        ));
    }
    buffer.extend_from_slice(chunk);
    let lines = take_lines(buffer);
    if buffer.len() > NDJSON_LINE_CAP || lines.iter().any(|line| line.len() > NDJSON_LINE_CAP) {
        return Err(LichessError::limit(
            operation,
            format!("NDJSON line exceeds the {NDJSON_LINE_CAP}-byte safety limit"),
        ));
    }
    Ok(lines)
}

pub fn validate_username(username: &str) -> Result<(), String> {
    let name = username.trim();
    let valid = (2..=30).contains(&name.len())
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        });
    if valid {
        Ok(())
    } else {
        Err(format!(
            "\"{username}\" is not a valid Lichess username (2-30 letters, digits, '_' or '-')."
        ))
    }
}

pub async fn account(
    base: &Url,
    client: &Client,
    token: &str,
) -> Result<ValidatedAccount, LichessError> {
    let response = request(client, Method::GET, api_url(base, &["account"])?, token)
        .send()
        .await
        .map_err(|error| LichessError::transport("load Lichess account", error, false))?;
    let response = checked(response, "load Lichess account").await?;
    let scopes = oauth_scopes(response.headers());
    let account = response_json(response, "load Lichess account").await?;
    Ok(ValidatedAccount { account, scopes })
}

pub async fn online_bots(base: &Url, client: &Client) -> Result<Vec<OnlineBot>, LichessError> {
    let mut url = api_url(base, &["bot", "online"])?;
    url.query_pairs_mut().append_pair("nb", "512");
    let response = client
        .get(url)
        .header("Accept", "application/x-ndjson")
        .header("User-Agent", "QueenUI/0.1 (Lichess bot manager)")
        .send()
        .await
        .map_err(|error| LichessError::transport("discover online bots", error, false))?;
    let response = checked(response, "discover online bots").await?;
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    let mut received = 0usize;
    let mut bots: Vec<OnlineBot> = Vec::new();
    let mut skipped = Skipped::default();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|error| LichessError::transport("discover online bots", error, false))?;
        for line in append_ndjson_chunk(
            &mut buffer,
            &chunk,
            &mut received,
            ONLINE_BOTS_BODY_CAP,
            "discover online bots",
        )? {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str(line.trim()) {
                Ok(bot) => bots.push(bot),
                Err(error) => skipped.record(error),
            }
        }
    }
    if !buffer.iter().all(u8::is_ascii_whitespace) {
        match serde_json::from_slice(&buffer) {
            Ok(bot) => bots.push(bot),
            Err(error) => skipped.record(error),
        }
    }
    skipped.report("online-bot entry", "online-bot entries");
    Ok(bots)
}

pub async fn export_games(
    base: &Url,
    client: &Client,
    token: &str,
    username: &str,
    max: u32,
) -> Result<Vec<Value>, LichessError> {
    let username = username.trim();
    validate_username(username).map_err(|error| LichessError::decode("export games", error))?;
    let mut url = api_url(base, &["games", "user", username])?;
    url.query_pairs_mut()
        .append_pair("max", &max.to_string())
        .append_pair("opening", "true")
        .append_pair("moves", "false")
        .append_pair("sort", "dateDesc")
        .append_pair("perfType", "ultraBullet,bullet,blitz,rapid,classical");
    let download = async {
        let response = client
            .get(url)
            .bearer_auth(token)
            .header("Accept", "application/x-ndjson")
            .header("User-Agent", "QueenUI/0.1 (Lichess bot manager)")
            .send()
            .await
            .map_err(|error| LichessError::transport("export games", error, false))?;
        let response = checked(response, "export games").await?;
        let mut stream = response.bytes_stream();
        let mut buffer = Vec::new();
        let mut received = 0usize;
        let mut games: Vec<Value> = Vec::new();
        let mut skipped = Skipped::default();
        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|error| LichessError::transport("export games", error, false))?;
            for line in append_ndjson_chunk(
                &mut buffer,
                &chunk,
                &mut received,
                GAME_EXPORT_BODY_CAP,
                "export games",
            )? {
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str(line.trim()) {
                    Ok(game) => games.push(game),
                    Err(error) => skipped.record(error),
                }
            }
        }
        if !buffer.iter().all(u8::is_ascii_whitespace) {
            match serde_json::from_slice(&buffer) {
                Ok(game) => games.push(game),
                Err(error) => skipped.record(error),
            }
        }
        skipped.report("exported game", "exported games");
        Ok(games)
    };
    tokio::time::timeout(Duration::from_secs(120), download)
        .await
        .map_err(|_| LichessError {
            kind: LichessErrorKind::Transport,
            status: None,
            code: Some("timeout".into()),
            retry_after: None,
            body: "Downloading the Lichess game history exceeded 120 seconds".into(),
            operation: "export games",
            ambiguous_write: false,
        })?
}

pub async fn event_stream(
    base: &Url,
    client: &Client,
    token: &str,
) -> Result<Response, LichessError> {
    let response = request(
        client,
        Method::GET,
        api_url(base, &["stream", "event"])?,
        token,
    )
    .send()
    .await
    .map_err(|error| LichessError::transport("open account event stream", error, false))?;
    checked(response, "open account event stream").await
}

pub async fn game_stream(
    base: &Url,
    client: &Client,
    token: &str,
    game_id: &str,
) -> Result<Response, LichessError> {
    let response = request(
        client,
        Method::GET,
        api_url(base, &["bot", "game", "stream", game_id])?,
        token,
    )
    .send()
    .await
    .map_err(|error| LichessError::transport("open game stream", error, false))?;
    checked(response, "open game stream").await
}

pub async fn play_move(
    base: &Url,
    client: &Client,
    token: &str,
    game_id: &str,
    chess_move: &str,
) -> Result<(), LichessError> {
    let response = request(
        client,
        Method::POST,
        api_url(base, &["bot", "game", game_id, "move", chess_move])?,
        token,
    )
    .send()
    .await
    .map_err(|error| LichessError::transport("submit move", error, true))?;
    checked(response, "submit move").await.map(|_| ())
}

pub async fn resign(
    base: &Url,
    client: &Client,
    token: &str,
    game_id: &str,
) -> Result<(), LichessError> {
    let response = request(
        client,
        Method::POST,
        api_url(base, &["bot", "game", game_id, "resign"])?,
        token,
    )
    .send()
    .await
    .map_err(|error| LichessError::transport("resign game", error, true))?;
    checked(response, "resign game").await.map(|_| ())
}

pub async fn create_challenge(
    base: &Url,
    client: &Client,
    token: &str,
    challenge: &ChallengeRequest,
) -> Result<ChallengeResult, LichessError> {
    let opponent = challenge.opponent.trim();
    validate_username(opponent).map_err(|error| LichessError::decode("create challenge", error))?;
    let form = [
        ("rated", challenge.rated.to_string()),
        ("clock.limit", challenge.clock_limit.to_string()),
        ("clock.increment", challenge.clock_increment.to_string()),
        ("color", challenge.color.to_lowercase()),
        ("variant", challenge.variant.clone()),
    ];
    let response = request(
        client,
        Method::POST,
        api_url(base, &["challenge", opponent])?,
        token,
    )
    .form(&form)
    .send()
    .await
    .map_err(|error| LichessError::transport("create challenge", error, true))?;
    let response = checked(response, "create challenge")
        .await
        .map_err(|mut error| {
            // A server-side failure can be generated after the POST was committed;
            // only authoritative outgoing-challenge state may resolve it.
            if error.is_server_error() {
                error.ambiguous_write = true;
            }
            error
        })?;
    let value: Value = response_json(response, "create challenge")
        .await
        .map_err(|mut error| {
            // A successful POST with an unreadable/truncated response is also
            // an unknown creation outcome.
            error.ambiguous_write = true;
            error
        })?;
    let core = value.get("challenge").unwrap_or(&value);
    let id = core
        .get("id")
        .or_else(|| value.get("id"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            let mut error =
                LichessError::decode("create challenge", "response omitted challenge id");
            error.ambiguous_write = true;
            error
        })?;
    let status = core
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("created");
    let url = core
        .get("url")
        .and_then(Value::as_str)
        .map(str::to_string)
        .map(Ok)
        .unwrap_or_else(|| site_url(id))?;
    Ok(ChallengeResult {
        id: id.to_string(),
        status: status.to_string(),
        url,
    })
}

pub async fn outgoing_challenges(
    base: &Url,
    client: &Client,
    token: &str,
) -> Result<Vec<OutgoingChallenge>, LichessError> {
    let response = request(client, Method::GET, api_url(base, &["challenge"])?, token)
        .send()
        .await
        .map_err(|error| LichessError::transport("reconcile outgoing challenges", error, false))?;
    let value: Value = response_json(
        checked(response, "reconcile outgoing challenges").await?,
        "reconcile outgoing challenges",
    )
    .await?;
    let outgoing = value
        .get("out")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(outgoing
        .into_iter()
        .filter_map(|challenge| {
            let id = challenge.get("id")?.as_str()?.to_string();
            let opponent = challenge
                .pointer("/destUser/id")
                .or_else(|| challenge.pointer("/destUser/name"))
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            let status = challenge
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("created")
                .to_string();
            Some(OutgoingChallenge {
                id,
                opponent,
                status,
            })
        })
        .collect())
}

pub async fn ongoing_game_ids(
    base: &Url,
    client: &Client,
    token: &str,
) -> Result<Vec<String>, LichessError> {
    let response = request(
        client,
        Method::GET,
        api_url(base, &["account", "playing"])?,
        token,
    )
    .send()
    .await
    .map_err(|error| LichessError::transport("reconcile ongoing games", error, false))?;
    let value: Value = response_json(
        checked(response, "reconcile ongoing games").await?,
        "reconcile ongoing games",
    )
    .await?;
    Ok(value
        .get("nowPlaying")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|game| {
            game.get("gameId")
                .or_else(|| game.get("id"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect())
}

pub async fn cancel_challenge(
    base: &Url,
    client: &Client,
    token: &str,
    challenge_id: &str,
) -> Result<(), LichessError> {
    let response = request(
        client,
        Method::POST,
        api_url(base, &["challenge", challenge_id, "cancel"])?,
        token,
    )
    .send()
    .await
    .map_err(|error| LichessError::transport("cancel challenge", error, true))?;
    checked(response, "cancel challenge").await.map(|_| ())
}

pub async fn decline_challenge(
    base: &Url,
    client: &Client,
    token: &str,
    challenge_id: &str,
    reason: &str,
) -> Result<(), LichessError> {
    let response = request(
        client,
        Method::POST,
        api_url(base, &["challenge", challenge_id, "decline"])?,
        token,
    )
    .form(&[("reason", reason)])
    .send()
    .await
    .map_err(|error| LichessError::transport("decline challenge", error, true))?;
    checked(response, "decline challenge").await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::{
        account, actionable_missing_scope_message, api_url, append_ndjson_chunk, create_challenge,
        default_api_base, outgoing_challenges, parse_retry_after, site_url, take_lines,
        validate_username, LichessErrorKind,
    };
    use crate::models::ChallengeRequest;
    use crate::test_support::{ScriptReply, ScriptedHttp};
    use reqwest::header::HeaderValue;
    use std::time::Duration;

    fn challenge_request() -> ChallengeRequest {
        ChallengeRequest {
            account_id: "bot".into(),
            opponent: "Opponent".into(),
            clock_limit: 180,
            clock_increment: 2,
            rated: false,
            color: "random".into(),
            variant: "standard".into(),
        }
    }

    #[test]
    fn take_lines_splits_chunks_and_keeps_partial_tail() {
        let mut buffer = b"first\r\nsecond\npart".to_vec();
        assert_eq!(take_lines(&mut buffer), ["first", "second"]);
        assert_eq!(buffer, b"part");
        buffer.extend_from_slice(b"ial\n");
        assert_eq!(take_lines(&mut buffer), ["partial"]);
        assert!(buffer.is_empty());
    }

    #[test]
    fn caps_ndjson_lines_and_bodies() {
        let mut buffer = Vec::new();
        let mut received = 0;
        assert_eq!(
            append_ndjson_chunk(&mut buffer, b"{\"ok\":true}\n", &mut received, 32, "test")
                .unwrap(),
            ["{\"ok\":true}"]
        );
        assert!(append_ndjson_chunk(&mut buffer, &[b'x'; 33], &mut received, 32, "test").is_err());
    }

    #[test]
    fn builds_encoded_path_segments() {
        let base = default_api_base().unwrap();
        let url = api_url(&base, &["bot", "game", "game/id", "move", "../resign"]).unwrap();
        assert_eq!(
            url.as_str(),
            "https://lichess.org/api/bot/game/game%2Fid/move/..%2Fresign"
        );
        assert_eq!(
            site_url("challenge/id").unwrap(),
            "https://lichess.org/challenge%2Fid"
        );
    }

    #[test]
    fn parses_retry_after_seconds_date_and_malformed_values() {
        assert_eq!(
            parse_retry_after(Some(&HeaderValue::from_static("17"))),
            Some(Duration::from_secs(17))
        );
        assert_eq!(
            parse_retry_after(Some(&HeaderValue::from_static("not-a-delay"))),
            None
        );
        let future = std::time::SystemTime::now() + Duration::from_secs(60);
        let date = HeaderValue::from_str(&httpdate::fmt_http_date(future)).unwrap();
        let parsed = parse_retry_after(Some(&date)).expect("HTTP-date Retry-After");
        assert!((Duration::from_secs(58)..=Duration::from_secs(60)).contains(&parsed));
    }

    #[test]
    fn validates_lichess_usernames() {
        assert!(validate_username("QueenBot_2024").is_ok());
        assert!(validate_username("ab").is_ok());
        assert!(validate_username(" maia1 ").is_ok());
        assert!(validate_username("a").is_err());
        assert!(validate_username("bad/../path").is_err());
        assert!(validate_username("name with spaces").is_err());
    }

    #[tokio::test]
    async fn account_validation_parses_oauth_scope_header() {
        let http = ScriptedHttp::start().await;
        http.push(
            "GET",
            "/api/account",
            ScriptReply::JsonWithHeaders(
                reqwest::StatusCode::OK,
                r#"{"id":"bot","username":"Bot","title":"BOT"}"#.into(),
                vec![(
                    "x-oauth-scopes",
                    "bot:play, challenge:read challenge:write,challenge:read",
                )],
            ),
        );

        let validation = account(&http.base(), &reqwest::Client::new(), "token")
            .await
            .unwrap();

        assert_eq!(
            validation.scopes,
            ["bot:play", "challenge:read", "challenge:write"]
        );
    }

    #[tokio::test]
    async fn missing_scope_403_is_classified_as_actionable_matchmaking_pause() {
        let http = ScriptedHttp::start().await;
        for scope in ["challenge:read", "bot:play"] {
            http.push(
                "GET",
                "/api/challenge",
                ScriptReply::Json(
                    reqwest::StatusCode::FORBIDDEN,
                    format!(r#"{{"error":"Missing scope: {scope}"}}"#),
                ),
            );
        }

        let error = outgoing_challenges(&http.base(), &reqwest::Client::new(), "token")
            .await
            .unwrap_err();

        assert_eq!(error.missing_scope().as_deref(), Some("challenge:read"));
        assert_eq!(
            actionable_missing_scope_message(&error).as_deref(),
            Some("Matchmaking is paused because this Lichess token is missing scope challenge:read; create a new token at lichess.org/account/oauth/token/create with Play-bot, Read-challenges, and Send-challenges ticked—games continue with the current token.")
        );

        let error = outgoing_challenges(&http.base(), &reqwest::Client::new(), "token")
            .await
            .unwrap_err();
        assert_eq!(
            actionable_missing_scope_message(&error).as_deref(),
            Some("QueenUI cannot play games because this Lichess token is missing scope bot:play; create a new token at lichess.org/account/oauth/token/create with Play-bot, Read-challenges, and Send-challenges ticked—the current token cannot continue games.")
        );
    }

    #[tokio::test]
    async fn create_challenge_marks_committed_server_failures_ambiguous() {
        let http = ScriptedHttp::start().await;
        http.push(
            "POST",
            "/api/challenge/Opponent",
            ScriptReply::Json(
                reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                r#"{"error":"committed then failed"}"#.into(),
            ),
        );
        let error = create_challenge(
            &http.base(),
            &reqwest::Client::new(),
            "token",
            &challenge_request(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind, LichessErrorKind::Http);
        assert_eq!(
            error.status,
            Some(reqwest::StatusCode::INTERNAL_SERVER_ERROR)
        );
        assert!(error.ambiguous_write);
    }

    #[tokio::test]
    async fn create_challenge_marks_transport_send_failures_ambiguous() {
        let http = ScriptedHttp::start().await;
        let base = http.base();
        drop(http);
        tokio::task::yield_now().await;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(1))
            .build()
            .unwrap();
        let error = create_challenge(&base, &client, "token", &challenge_request())
            .await
            .unwrap_err();
        assert_eq!(error.kind, LichessErrorKind::Transport);
        assert!(error.ambiguous_write);
    }
}
