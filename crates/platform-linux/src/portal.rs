use std::{collections::HashMap, os::fd::OwnedFd};

use zbus::{
    blocking::{Connection, Proxy},
    zvariant::{OwnedFd as ZbusOwnedFd, OwnedObjectPath, OwnedValue, Value},
};

use pinray_core::{PinrayError, Result};

/// Cursor capture mode for the XDG Desktop Portal screencast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorMode {
    /// Do not capture the cursor.
    Hidden = 1,
    /// Render the cursor onto the captured framebuffer.
    Embedded = 2,
}

impl CursorMode {
    fn bits(self) -> u32 {
        self as u32
    }
}

/// A single screen/window stream from the portal.
///
/// `node_id` is the PipeWire node id to connect to.
/// `width`/`height` are optional and may not be set by all compositors.
#[derive(Debug, Clone)]
pub struct ScreenCastStream {
    pub node_id: u32,
    #[allow(dead_code)]
    pub width: Option<u32>,
    #[allow(dead_code)]
    pub height: Option<u32>,
}

/// An active screencast session with an open PipeWire fd.
///
/// The D-Bus connection must outlive this object — if the underlying
/// `PortalClient` is dropped, the session is torn down and the fd becomes
/// invalid, causing "no more input formats" from PipeWire.
#[derive(Debug)]
pub struct ActiveScreenCast {
    fd: OwnedFd,
    pub streams: Vec<ScreenCastStream>,
    pub restore_token: Option<String>,
}

impl ActiveScreenCast {
    /// Consume the session and return the raw parts: PipeWire fd, streams, restore token.
    pub fn into_parts(self) -> (OwnedFd, Vec<ScreenCastStream>, Option<String>) {
        (self.fd, self.streams, self.restore_token)
    }
}

/// D-Bus client for `org.freedesktop.portal.ScreenCast`.
///
/// Opens a D-Bus session connection to the portal. The connection must stay
/// alive for as long as the PipeWire fd obtained from `start_screen_cast` is
/// in use.
///
/// # Lifecycle
///
/// ```ignore
/// let portal = PortalClient::new()?;
/// let cast = portal.start_screen_cast(true, None)?;
/// let (fd, streams, _) = cast.into_parts();
/// // portal MUST outlive the worker thread that uses `fd`
/// ```
pub struct PortalClient {
    connection: Connection,
}

impl PortalClient {
    /// Open a new D-Bus session connection to the portal.
    pub fn new() -> Result<Self> {
        let connection =
            Connection::session().map_err(|error| PinrayError::Platform(error.to_string()))?;
        Ok(Self { connection })
    }

    /// Request a screencast session through the portal.
    ///
    /// This triggers the user permission dialog (unless a valid `restore_token`
    /// from a previous session is provided). Returns an `ActiveScreenCast` with
    /// the PipeWire fd and stream metadata.
    ///
    /// The caller must ensure `self` (the `PortalClient`, i.e. the D-Bus
    /// connection) outlives the returned `ActiveScreenCast` — dropping this
    /// connection invalidates the fd.
    pub fn start_screen_cast(
        &self,
        include_cursor: bool,
        restore_token: Option<&str>,
    ) -> Result<ActiveScreenCast> {
        let desktop = Proxy::new(
            &self.connection,
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.ScreenCast",
        )
        .map_err(|error| PinrayError::Platform(error.to_string()))?;

        tracing::debug!("portal: creating session...");
        let session = self.create_session(&desktop)?;
        tracing::debug!(?session, "portal: session created");

        tracing::debug!("portal: selecting sources...");
        self.select_sources(&desktop, &session, include_cursor, restore_token)?;
        tracing::debug!("portal: sources selected");

        tracing::debug!("portal: starting capture...");
        let response = self.start(&desktop, &session)?;
        tracing::debug!(streams = ?response.streams, "portal: capture started");

        tracing::debug!("portal: opening PipeWire remote...");
        let fd = self.open_pipewire_remote(&desktop, &session)?;
        tracing::debug!("portal: PipeWire remote opened");

        Ok(ActiveScreenCast {
            fd,
            streams: response
                .streams
                .unwrap_or_default()
                .into_iter()
                .map(|(node_id, props)| ScreenCastStream {
                    node_id,
                    width: extract_size(&props).map(|value| value.0),
                    height: extract_size(&props).map(|value| value.1),
                })
                .collect(),
            restore_token: response.restore_token,
        })
    }

    fn create_session(&self, desktop: &Proxy<'_>) -> Result<OwnedObjectPath> {
        let handle_token = next_token();
        let request = self.request_proxy(&handle_token)?;
        let session_handle_token = next_token();

        let mut signal = request
            .receive_signal("Response")
            .map_err(|error| PinrayError::Platform(error.to_string()))?;

        let options = HashMap::from([
            ("handle_token", Value::from(handle_token.as_str())),
            (
                "session_handle_token",
                Value::from(session_handle_token.as_str()),
            ),
        ]);

        desktop
            .call_method("CreateSession", &(options))
            .map_err(|error| PinrayError::Platform(error.to_string()))?;

        let response = wait_response(&mut signal)?;
        let session_handle = response.get("session_handle").ok_or_else(|| {
            PinrayError::Platform("portal response missing session_handle".into())
        })?;
        let session_handle = String::try_from(session_handle.clone())
            .map_err(|error| PinrayError::Platform(error.to_string()))?;
        OwnedObjectPath::try_from(session_handle)
            .map_err(|error| PinrayError::Platform(error.to_string()))
    }

    fn select_sources(
        &self,
        desktop: &Proxy<'_>,
        session: &OwnedObjectPath,
        include_cursor: bool,
        restore_token: Option<&str>,
    ) -> Result<()> {
        let handle_token = next_token();
        let request = self.request_proxy(&handle_token)?;

        let mut signal = request
            .receive_signal("Response")
            .map_err(|error| PinrayError::Platform(error.to_string()))?;

        let mut options = HashMap::from([
            ("handle_token", Value::from(handle_token.as_str())),
            ("types", Value::from(3_u32)),
            ("multiple", Value::from(false)),
            (
                "cursor_mode",
                Value::from(
                    if include_cursor {
                        CursorMode::Embedded
                    } else {
                        CursorMode::Hidden
                    }
                    .bits(),
                ),
            ),
            ("persist_mode", Value::from(2_u32)),
        ]);

        if let Some(restore_token) = restore_token {
            options.insert("restore_token", Value::from(restore_token));
        }

        desktop
            .call_method("SelectSources", &(session, options))
            .map_err(|error| PinrayError::Platform(error.to_string()))?;

        let _ = wait_response(&mut signal)?;
        Ok(())
    }

    fn start(&self, desktop: &Proxy<'_>, session: &OwnedObjectPath) -> Result<StartResponse> {
        let handle_token = next_token();
        let request = self.request_proxy(&handle_token)?;

        let mut signal = request
            .receive_signal("Response")
            .map_err(|error| PinrayError::Platform(error.to_string()))?;

        let options = HashMap::from([("handle_token", Value::from(handle_token.as_str()))]);

        desktop
            .call_method("Start", &(session, "", options))
            .map_err(|error| PinrayError::Platform(error.to_string()))?;

        let response = wait_response(&mut signal)?;
        let streams = response
            .get("streams")
            .cloned()
            .map(parse_streams)
            .transpose()?;
        let restore_token = response
            .get("restore_token")
            .cloned()
            .map(String::try_from)
            .transpose()
            .map_err(|error| PinrayError::Platform(error.to_string()))?;

        Ok(StartResponse {
            streams,
            restore_token,
        })
    }

    fn open_pipewire_remote(
        &self,
        desktop: &Proxy<'_>,
        session: &OwnedObjectPath,
    ) -> Result<OwnedFd> {
        let options: HashMap<&str, Value<'_>> = HashMap::new();
        let fd: ZbusOwnedFd = desktop
            .call("OpenPipeWireRemote", &(session, options))
            .map_err(|error| PinrayError::Platform(error.to_string()))?;
        Ok(fd.into())
    }

    fn request_proxy(&self, handle_token: &str) -> Result<Proxy<'_>> {
        let unique_identifier = self
            .connection
            .unique_name()
            .ok_or_else(|| PinrayError::Platform("missing dbus unique name".into()))?
            .trim_start_matches(':')
            .replace('.', "_");
        let path =
            format!("/org/freedesktop/portal/desktop/request/{unique_identifier}/{handle_token}");

        Proxy::new(
            &self.connection,
            "org.freedesktop.portal.Desktop",
            path,
            "org.freedesktop.portal.Request",
        )
        .map_err(|error| PinrayError::Platform(error.to_string()))
    }
}

fn wait_response(
    signal: &mut zbus::blocking::proxy::SignalIterator<'_>,
) -> Result<HashMap<String, OwnedValue>> {
    let message = signal
        .next()
        .ok_or_else(|| PinrayError::Platform("portal response signal missing".into()))?;

    let (code, body): (u32, HashMap<String, OwnedValue>) = message
        .body()
        .deserialize()
        .map_err(|error| PinrayError::Platform(error.to_string()))?;

    match code {
        0 => Ok(body),
        1 => Err(PinrayError::Unsupported(
            "portal request cancelled by user".into(),
        )),
        other => Err(PinrayError::Platform(format!(
            "portal request failed with response code {other}"
        ))),
    }
}

fn parse_streams(value: OwnedValue) -> Result<Vec<(u32, HashMap<String, OwnedValue>)>> {
    let entries = Vec::<(u32, HashMap<String, OwnedValue>)>::try_from(value)
        .map_err(|error| PinrayError::Platform(error.to_string()))?;
    Ok(entries)
}

fn extract_size(props: &HashMap<String, OwnedValue>) -> Option<(u32, u32)> {
    let size = props.get("size")?;
    let (width, height) = <(i32, i32)>::try_from(size.clone()).ok()?;
    Some((width.max(0) as u32, height.max(0) as u32))
}

fn next_token() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(1);
    format!("pinray_{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

#[derive(Debug)]
struct StartResponse {
    streams: Option<Vec<(u32, HashMap<String, OwnedValue>)>>,
    restore_token: Option<String>,
}
