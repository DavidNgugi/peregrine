//! The guard's own unit tests, and the loopback server's behaviour over a real socket.
//!
//! **A real socket rather than a mocked service**, and a hand-written request rather than a client
//! library, and both are deliberate. What is under test is HTTP: the status, the headers, the exact
//! bytes of a range, and the refusal of a `Host` this build did not write. A client library would
//! normalise the very things being asserted — it would set its own `Host`, follow its own redirects,
//! and re-encode a range — so a test written against one could pass while the wire behaviour was
//! wrong.

use std::net::Ipv4Addr;
use std::path::Path;
use std::sync::Arc;

use peregrine_domain::{PathGuard, SafePath, SessionToken};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::time::{Duration, timeout};

use super::{Capture, LOOPBACK_HOSTS, MediaServer, SidecarSubtitle, split_host};
use crate::error::StreamError;
use crate::thumbnail::{ThumbnailError, ThumbnailSink};

/// How long any one request may take before the test is wrong rather than slow.
const PATIENCE: Duration = Duration::from_secs(5);

/// The bytes every fixture file holds, so an offset in an answer is checkable by arithmetic.
fn body_of(length: usize) -> Vec<u8> {
    (0..length).map(|index| (index % 251) as u8).collect()
}

/// A token of the shape the minter produces, fixed so a test is not also testing a random source.
fn token(seed: char) -> SessionToken {
    SessionToken::new(seed.to_string().repeat(32)).expect("32 ASCII alphanumerics")
}

/// A directory with one file in it, and the file's bytes.
struct Fixture {
    directory: tempfile::TempDir,
    length: usize,
}

impl Fixture {
    fn new(length: usize) -> Self {
        let directory = tempfile::tempdir().expect("a scratch directory");
        std::fs::write(directory.path().join("film.mp4"), body_of(length))
            .expect("the fixture writes");
        Self { directory, length }
    }

    fn path(&self) -> std::path::PathBuf {
        self.directory.path().join("film.mp4")
    }

    /// The file as the filesystem layer would hand it over: inside a root, via `PathGuard`.
    fn safe(&self) -> SafePath {
        PathGuard::new(self.directory.path())
            .expect("the scratch directory is absolute")
            .resolve(&self.path())
            .expect("the fixture is inside its own root")
    }

    fn expected(&self) -> Vec<u8> {
        body_of(self.length)
    }
}

/// A sink that records what it was asked to store, so the route can be tested without a cache.
///
/// It answers the way a real one does for the interesting cases: the bytes are kept, and a call is
/// counted, so a test can assert both *that* a frame arrived and *how many* were stored — the second
/// being what stops one frame becoming one write per `seeked` event.
#[derive(Default)]
struct RecordingSink {
    stored: std::sync::Mutex<Vec<(i64, Vec<u8>)>>,
    /// When set, every store is refused, which is the sink a broken cache presents.
    refuses: bool,
}

impl RecordingSink {
    fn refusing() -> Self {
        Self {
            refuses: true,
            ..Self::default()
        }
    }

    fn frames(&self) -> Vec<(i64, Vec<u8>)> {
        self.stored
            .lock()
            .expect("the lock is not poisoned")
            .clone()
    }
}

impl ThumbnailSink for RecordingSink {
    fn store(&self, title_id: i64, bytes: &[u8]) -> Result<std::path::PathBuf, ThumbnailError> {
        if self.refuses {
            return Err(ThumbnailError::Refused);
        }
        self.stored
            .lock()
            .expect("the lock is not poisoned")
            .push((title_id, bytes.to_vec()));
        Ok(std::path::PathBuf::from("/cache/images/local-frame"))
    }
}

/// A whole JPEG by its own framing: the start marker, a segment, and the end marker.
///
/// Not a decodable image — the route does not decode and does not claim to — but a body whose first and
/// last bytes are what a JPEG's are, which is exactly what the route checks.
fn jpeg() -> Vec<u8> {
    let mut bytes = vec![0xff, 0xd8, 0xff, 0xe0];
    bytes.extend(std::iter::repeat_n(0x42, 512));
    bytes.extend_from_slice(&[0xff, 0xd9]);
    bytes
}

/// One HTTP response, as it arrived.
struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Answer {
    fn parse(raw: &[u8]) -> Self {
        let end = raw
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("an answer has a header block");
        let head = String::from_utf8_lossy(&raw[..end]).into_owned();
        let body = raw[end + 4..].to_vec();
        let mut lines = head.lines();
        let status = lines
            .next()
            .expect("a status line")
            .split_whitespace()
            .nth(1)
            .expect("a status code")
            .parse()
            .expect("a number");
        let headers = lines
            .filter_map(|line| {
                line.split_once(": ")
                    .map(|(name, value)| (name.to_ascii_lowercase(), value.to_owned()))
            })
            .collect();
        Self {
            status,
            headers,
            body,
        }
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }
}

/// One request line, its headers, and nothing that a client library would have added.
fn request(method: &str, path: &str, host: Option<&str>, extra: &[(&str, &str)]) -> String {
    use std::fmt::Write as _;

    // `write!` rather than `push_str(&format!(..))`, which is the same bytes without a `String` per header.
    let mut text = format!("{method} {path} HTTP/1.1\r\n");
    if let Some(host) = host {
        let _ = write!(text, "Host: {host}\r\n");
    }
    for (name, value) in extra {
        let _ = write!(text, "{name}: {value}\r\n");
    }
    // Without this the connection is kept alive and reading to the end of the stream never returns.
    text.push_str("Connection: close\r\n\r\n");
    text
}

/// One plain-text request as bytes, which is what the socket takes.
fn bytes(request: String) -> Vec<u8> {
    request.into_bytes()
}

/// Sends `raw` to `port` and reads whatever comes back.
///
/// **Bytes rather than a `String`, and a fixture found that the hard way.** A JPEG's first byte is `0xff`,
/// which is not valid UTF-8: building a request by pushing `char::from(byte)` into a `String` re-encodes it
/// as two bytes, so the body the server received did not begin with `FF D8 FF` and the route refused it —
/// while the test's own log said it had sent a JPEG. A request is bytes, and this is where that is true.
async fn ask(port: u16, raw: &[u8]) -> Answer {
    let mut socket = timeout(PATIENCE, TcpStream::connect((Ipv4Addr::LOCALHOST, port)))
        .await
        .expect("a connection attempt must not hang")
        .expect("the listener is bound");
    socket.write_all(raw).await.expect("the request writes");
    socket.flush().await.expect("the request flushes");
    let mut bytes = Vec::new();
    timeout(PATIENCE, socket.read_to_end(&mut bytes))
        .await
        .expect("an answer must not hang")
        .expect("the answer reads");
    Answer::parse(&bytes)
}

/// The request the renderer's media element makes: the token in the path, loopback in `Host`.
async fn ok(server: &MediaServer, extra: &[(&str, &str)]) -> Answer {
    let port = server.port();
    ask(
        port,
        &bytes(request(
            "GET",
            &format!("/media/{}", server.token().as_str()),
            Some(&format!("127.0.0.1:{port}")),
            extra,
        )),
    )
    .await
}

/// A server over a fresh fixture, and the fixture it serves.
async fn served(length: usize) -> (Fixture, MediaServer) {
    let fixture = Fixture::new(length);
    let server = MediaServer::start(&fixture.safe(), token('a'), Vec::new(), None)
        .await
        .expect("the fixture is servable");
    (fixture, server)
}

#[test]
fn a_host_is_split_into_its_name_and_its_port_whatever_shape_it_has() {
    // The shapes a `Host` arrives in, including the two that must be refused rather than read as a
    // header without a port.
    assert_eq!(split_host("127.0.0.1"), Some(("127.0.0.1", None)));
    assert_eq!(
        split_host("127.0.0.1:8080"),
        Some(("127.0.0.1", Some(8080)))
    );
    assert_eq!(split_host("localhost:"), Some(("localhost", None)));
    assert_eq!(split_host("[::1]"), Some(("[::1]", None)));
    assert_eq!(split_host("[::1]:8080"), Some(("[::1]", Some(8080))));
    assert_eq!(split_host("evil.com:80"), Some(("evil.com", Some(80))));
    // A malformed port is not a missing port: reading it as one would accept a header this build
    // cannot parse.
    assert_eq!(split_host("127.0.0.1:notaport"), None);
    assert_eq!(split_host("127.0.0.1:99999999"), None);
    assert_eq!(split_host("[::1]:"), Some(("[::1]", None)));
}

#[test]
fn the_loopback_names_are_the_three_a_client_on_this_machine_sends() {
    // Asserted rather than assumed: the list is the control, and a name added to it is a name that
    // would be accepted from a page that is not ours.
    assert_eq!(LOOPBACK_HOSTS, &["127.0.0.1", "localhost", "[::1]"]);
}

#[tokio::test]
async fn a_full_read_serves_the_whole_file_with_the_headers_the_client_needs() {
    let (fixture, server) = served(4096).await;

    let answer = ok(&server, &[]).await;

    assert_eq!(answer.status, 200);
    assert_eq!(answer.body, fixture.expected());
    assert_eq!(
        answer.header("accept-ranges"),
        Some("bytes"),
        "a client only tries to seek when the server says it can"
    );
    assert_eq!(
        answer.header("content-length"),
        Some("4096"),
        "the element needs the length to size its timeline"
    );
    assert_eq!(
        answer.header("cache-control"),
        Some("no-store"),
        "SECURITY.md T10 asks for it on every response"
    );
    assert_eq!(answer.header("content-type"), Some("video/mp4"));
    assert_eq!(answer.header("x-content-type-options"), Some("nosniff"));

    server.shutdown().await;
}

#[tokio::test]
async fn a_range_is_answered_with_exactly_the_bytes_it_asked_for() {
    // What seeking is: the element asks for a window and gets those bytes and no others.
    let (fixture, server) = served(4096).await;
    let expected = fixture.expected();

    let answer = ok(&server, &[("Range", "bytes=100-199")]).await;

    assert_eq!(answer.status, 206);
    assert_eq!(answer.body, expected[100..200]);
    assert_eq!(answer.header("content-range"), Some("bytes 100-199/4096"));
    assert_eq!(answer.header("content-length"), Some("100"));
    // Every response, including this one: T10 does not exempt the interesting case.
    assert_eq!(answer.header("cache-control"), Some("no-store"));

    server.shutdown().await;
}

#[tokio::test]
async fn a_suffix_and_an_open_ended_range_are_answered_from_the_right_end() {
    // Both forms are legal and both are easy to get backwards: `bytes=-4` is the *last* four bytes and
    // `bytes=4092-` is everything from 4092, which happen to be the same window here — so the second
    // assertion uses a different start to keep the two apart.
    let (fixture, server) = served(4096).await;
    let expected = fixture.expected();

    let suffix = ok(&server, &[("Range", "bytes=-4")]).await;
    assert_eq!(suffix.status, 206);
    assert_eq!(suffix.body, expected[4092..]);
    assert_eq!(suffix.header("content-range"), Some("bytes 4092-4095/4096"));

    let open = ok(&server, &[("Range", "bytes=4000-")]).await;
    assert_eq!(open.status, 206);
    assert_eq!(open.body, expected[4000..]);
    assert_eq!(open.header("content-range"), Some("bytes 4000-4095/4096"));

    server.shutdown().await;
}

#[tokio::test]
async fn a_range_that_starts_past_the_end_is_refused_rather_than_answered_with_nothing() {
    // A `200` here would make a seeking element believe it had the bytes it asked for.
    let (_fixture, server) = served(1024).await;

    let answer = ok(&server, &[("Range", "bytes=5000-6000")]).await;

    assert_eq!(answer.status, 416);
    assert_eq!(answer.header("content-range"), Some("bytes */1024"));

    server.shutdown().await;
}

#[tokio::test]
async fn a_head_answers_the_headers_and_no_body() {
    // What a client does before it decides anything: it asks how big the file is without downloading it.
    let (_fixture, server) = served(4096).await;
    let port = server.port();

    let answer = ask(
        port,
        &bytes(request(
            "HEAD",
            &format!("/media/{}", server.token().as_str()),
            Some(&format!("127.0.0.1:{port}")),
            &[],
        )),
    )
    .await;

    assert_eq!(answer.status, 200);
    assert_eq!(answer.header("content-length"), Some("4096"));
    assert_eq!(answer.body, Vec::<u8>::new(), "a HEAD carries no body");

    server.shutdown().await;
}

#[tokio::test]
async fn a_page_that_rebound_dns_to_loopback_is_refused_before_its_token_is_read() {
    // The request a rebinding attack actually makes: the socket is reached, the token is right, and the
    // `Host` names the attacker. This is the control that stops it, and it is asserted with the
    // *correct* token so a pass cannot be the token check doing the work.
    let (_fixture, server) = served(1024).await;
    let port = server.port();
    let path = format!("/media/{}", server.token().as_str());

    for host in [
        "evil.com",
        "evil.com:80",
        "127.0.0.1.evil.com",
        "localhost.evil.com",
    ] {
        let answer = ask(port, &bytes(request("GET", &path, Some(host), &[]))).await;
        assert_eq!(answer.status, 403, "Host: {host} must be refused");
    }

    // A `Host` naming loopback on a *different* port is a different listener than this one, so it is
    // refused too — a browser always states the port it dialled.
    let wrong_port = ask(
        port,
        &bytes(request("GET", &path, Some("127.0.0.1:1"), &[])),
    )
    .await;
    assert_eq!(wrong_port.status, 403);

    // The same request with a loopback `Host` is served, which is what makes the four refusals above
    // about the header rather than about anything else in the request.
    assert_eq!(ok(&server, &[]).await.status, 200);

    server.shutdown().await;
}

#[tokio::test]
async fn the_other_loopback_spellings_are_accepted() {
    // `localhost` and `[::1]` are what a client sends when it reached this machine by name or by IPv6,
    // and refusing them would be refusing a request that is not an attack.
    let (_fixture, server) = served(64).await;
    let port = server.port();

    for host in ["localhost", "LOCALHOST", "[::1]", "127.0.0.1"] {
        let answer = ask(
            port,
            &bytes(request(
                "GET",
                &format!("/media/{}", server.token().as_str()),
                Some(host),
                &[],
            )),
        )
        .await;
        assert_eq!(answer.status, 200, "Host: {host}");
    }

    server.shutdown().await;
}

#[tokio::test]
async fn a_request_with_no_host_at_all_is_refused() {
    // HTTP/1.1 requires the header. Failing closed means a request this build cannot check is a request
    // it does not answer, which is the direction that cannot be wrong.
    let (_fixture, server) = served(64).await;
    let port = server.port();

    let answer = ask(
        port,
        &bytes(request(
            "GET",
            &format!("/media/{}", server.token().as_str()),
            None,
            &[],
        )),
    )
    .await;

    assert_eq!(answer.status, 403);

    server.shutdown().await;
}

#[tokio::test]
async fn a_missing_or_wrong_token_is_refused_and_says_nothing_about_the_right_one() {
    let (_fixture, server) = served(1024).await;
    let port = server.port();
    let host = format!("127.0.0.1:{port}");

    // No token at all. The route matches an empty segment and the guard refuses it, which is the same
    // answer by a different layer — what matters is that nothing is served and the refusal is short.
    let bare = ask(port, &bytes(request("GET", "/media/", Some(&host), &[]))).await;
    assert_ne!(bare.status, 200);
    assert!(
        bare.body.len() < 64,
        "a refusal says nothing about the token"
    );
    // A path the route does not describe is refused by the guard before the router sees it, and that is
    // the stronger of the two answers: *no* path this server is asked for is served without the token, so
    // a caller who does not have it cannot tell a real route from a missing one.
    let other = ask(port, &bytes(request("GET", "/nope", Some(&host), &[]))).await;
    assert_eq!(other.status, 401);

    // A wrong token of the right shape, a token that is a prefix of the right one, and one that is
    // longer: all refused, and all with the same answer.
    let wrong = token('b');
    let right = server.token().as_str();
    for offered in [
        wrong.as_str().to_owned(),
        right[..right.len() - 1].to_owned(),
        format!("{right}extra"),
        format!("{right}/../film.mp4"),
    ] {
        let answer = ask(
            port,
            &bytes(request(
                "GET",
                &format!("/media/{offered}"),
                Some(&host),
                &[],
            )),
        )
        .await;
        assert_ne!(answer.status, 200, "token {offered:?} must not be served");
    }

    server.shutdown().await;
}

#[tokio::test]
async fn one_session_s_token_does_not_open_another_session() {
    // Two listeners on two ports, each serving its own file. A token that opened both would mean the
    // token was not what scoped the read.
    let first = Fixture::new(512);
    let second = Fixture::new(1024);
    let a = MediaServer::start(&first.safe(), token('a'), Vec::new(), None)
        .await
        .expect("the first session opens");
    let b = MediaServer::start(&second.safe(), token('b'), Vec::new(), None)
        .await
        .expect("the second session opens");

    assert_ne!(a.port(), b.port(), "each session gets its own socket");
    assert_eq!(a.file(), Some(first.path()).as_deref());
    assert_eq!(b.file(), Some(second.path()).as_deref());

    // A's token against B's port.
    let crossed = ask(
        b.port(),
        &bytes(request(
            "GET",
            &format!("/media/{}", a.token().as_str()),
            Some(&format!("127.0.0.1:{}", b.port())),
            &[],
        )),
    )
    .await;
    assert_ne!(crossed.status, 200);

    // Each still serves its own file to its own token.
    assert_eq!(ok(&a, &[]).await.body, first.expected());
    assert_eq!(ok(&b, &[]).await.body, second.expected());

    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test]
async fn no_path_other_than_the_token_route_is_served() {
    // "No listing" is structural rather than a handler that refuses: there is no route that takes a
    // file name, so a request for one matches nothing at all.
    let (_fixture, server) = served(256).await;
    let port = server.port();
    let host = format!("127.0.0.1:{port}");
    let right = server.token().as_str();

    // The positive control: with the file alive and the token right, this exact route *is* served. It is
    // what makes the refusals below about the paths rather than about a server that serves nothing.
    assert_eq!(ok(&server, &[]).await.status, 200);

    for path in [
        "/",
        "/media",
        "/media/",
        &format!("/media/{right}/"),
        &format!("/media/{right}/../../etc/passwd"),
        &format!("/files/{right}"),
        &format!("/media/{right}/film.mp4"),
    ] {
        let answer = ask(port, &bytes(request("GET", path, Some(&host), &[]))).await;
        assert_ne!(answer.status, 200, "{path} must not be served");
    }

    server.shutdown().await;
}

#[tokio::test]
async fn a_file_that_is_gone_is_refused_when_the_session_opens_rather_than_when_it_is_read() {
    // `media_file.missing_since` is what a *scan* saw. Between that and the press of Play the file can
    // be moved, and the refusal belongs beside the control the user pressed rather than in a request
    // nobody is watching.
    let fixture = Fixture::new(64);
    let safe = fixture.safe();
    std::fs::remove_file(fixture.path()).expect("the fixture removes");

    let refused = MediaServer::start(&safe, token('a'), Vec::new(), None)
        .await
        .expect_err("a gone file cannot be served");

    assert!(
        matches!(refused, StreamError::Missing { .. }),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_directory_is_refused_rather_than_served_as_an_empty_file() {
    let directory = tempfile::tempdir().expect("a scratch directory");
    let guard = PathGuard::new(directory.path()).expect("an absolute root");
    let safe = guard
        .resolve(directory.path())
        .expect("the directory is inside its own root");

    let refused = MediaServer::start(&safe, token('a'), Vec::new(), None)
        .await
        .expect_err("a directory is not a file");

    assert!(
        matches!(refused, StreamError::NotAFile { .. }),
        "{refused:?}"
    );
}

#[tokio::test]
async fn shutting_a_session_down_closes_its_socket() {
    // "The socket dies with the session" is the fourth control T10 asks for, and this is the assertion
    // that it is a socket closing rather than a flag a handler consults.
    let (_fixture, server) = served(64).await;
    let port = server.port();
    assert_eq!(ok(&server, &[]).await.status, 200);

    assert!(server.shutdown().await, "the listener stops cleanly");

    assert!(
        TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .is_err(),
        "nothing is listening on port {port} after the session ends"
    );
}

#[tokio::test]
async fn a_session_that_is_dropped_without_being_shut_down_still_stops_listening() {
    // The runtime that replaces a session may simply drop the old one, and a listener that outlived its
    // owner would be a socket nobody can close.
    let fixture = Fixture::new(64);
    let server = MediaServer::start(&fixture.safe(), token('a'), Vec::new(), None)
        .await
        .expect("the fixture is servable");
    let port = server.port();

    drop(server);

    // The signal is sent synchronously by the destructor; the task needs a moment to notice it, and the
    // loop is bounded so a listener that never stops fails rather than hangs.
    let mut stopped = false;
    for _ in 0..50 {
        if TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .is_err()
        {
            stopped = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        stopped,
        "port {port} is still listening after the server was dropped"
    );
}

#[tokio::test]
async fn the_url_names_loopback_and_the_token_and_nothing_else() {
    let (fixture, server) = served(64).await;

    assert_eq!(
        server.url(),
        format!(
            "http://127.0.0.1:{}/media/{}",
            server.port(),
            token('a').as_str()
        )
    );
    assert!(
        !format!("{server:?}").contains(&fixture.path().to_string_lossy().into_owned()),
        "a debug line must not carry a path (SECURITY.md T13)"
    );
    assert!(
        !format!("{server:?}").contains(token('a').as_str()),
        "and it must not carry the credential"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn an_unservable_path_is_never_served_however_it_is_spelled() {
    // `SafePath` is the only thing `start` accepts, and only `PathGuard` makes one — so the containment
    // check is a type rather than a branch here. This asserts the property from this side: a path that
    // climbs out of its root cannot be turned into the argument at all.
    let fixture = Fixture::new(64);
    let guard = PathGuard::new(fixture.directory.path()).expect("an absolute root");

    let climbing = guard
        .resolve(Path::new("/etc/passwd"))
        .expect_err("a path outside the root has no SafePath");

    assert!(
        !climbing.to_string().is_empty(),
        "and the refusal says something a maintainer can read: {climbing}"
    );
}

/// A session that can store a frame, the sink it stores into, and the fixture it serves.
async fn capturing() -> (Fixture, Arc<RecordingSink>, MediaServer) {
    let fixture = Fixture::new(2048);
    let sink = Arc::new(RecordingSink::default());
    let server = MediaServer::start(
        &fixture.safe(),
        token('a'),
        Vec::new(),
        Some(Capture {
            title_id: 7,
            sink: Arc::clone(&sink) as Arc<dyn ThumbnailSink>,
        }),
    )
    .await
    .expect("the fixture is servable");
    (fixture, sink, server)
}

/// A capture request, with the headers a browser would send.
fn capture_request(server: &MediaServer, content_type: &str, body: &[u8]) -> Vec<u8> {
    let head = format!(
        "POST /thumbnail/{} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        server.token().as_str(),
        server.port(),
        body.len()
    );
    let mut raw = head.into_bytes();
    raw.extend_from_slice(body);
    raw
}

#[test]
fn the_jpeg_declaration_is_read_the_way_a_browser_writes_it() {
    // The predicate on its own, so a failure of the route above says which half is wrong.
    use super::is_jpeg_content_type;
    use axum::http::HeaderValue;

    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("image/jpeg"),
    );
    assert!(is_jpeg_content_type(&headers));
    assert!(!is_jpeg_content_type(&axum::http::HeaderMap::new()));
}

#[tokio::test]
async fn a_captured_frame_is_stored_for_the_session_s_title() {
    let (_fixture, sink, server) = capturing().await;
    let frame = jpeg();

    let answer = ask(
        server.port(),
        &capture_request(&server, "image/jpeg", &frame),
    )
    .await;

    assert_eq!(
        answer.status,
        204,
        "refused with: {}",
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        sink.frames(),
        vec![(7, frame)],
        "the frame reaches the sink under the title the session is playing"
    );
    // A bodyless answer, deliberately: the renderer does not need the path, and a path in a response is a
    // filesystem path on the wire.
    assert_eq!(answer.body, Vec::<u8>::new(), "a refusal carries no body");
    assert_eq!(answer.header("cache-control"), Some("no-store"));

    server.shutdown().await;
}

#[tokio::test]
async fn a_capture_declaring_something_other_than_a_jpeg_is_refused_before_the_sink_sees_it() {
    let (_fixture, sink, server) = capturing().await;

    for (content_type, body) in [
        ("image/png", jpeg()),
        ("text/plain", jpeg()),
        ("application/octet-stream", jpeg()),
        ("", jpeg()),
    ] {
        let answer = ask(
            server.port(),
            &capture_request(&server, content_type, &body),
        )
        .await;
        assert_ne!(answer.status, 204, "Content-Type: {content_type}");
    }
    assert_eq!(
        sink.frames(),
        Vec::new(),
        "a refused declaration never reaches the cache"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn bytes_that_are_not_a_jpeg_are_refused_however_they_are_declared() {
    // The check that matters: the sink writes into the directory the asset protocol is scoped to, so a
    // blob that lands there is a blob the webview can be told to render. "Our own renderer sent it" is
    // not verification.
    let (_fixture, sink, server) = capturing().await;

    for body in [
        Vec::new(),
        b"not an image at all".to_vec(),
        // An HTML document with a JPEG's extension would be a stored file the webview renders.
        b"<!doctype html><script>alert(1)</script>".to_vec(),
        // The start marker and nothing else: three bytes is not a frame, and the end marker is what says so.
        vec![0xff, 0xd8, 0xff],
        // **A whole JPEG's head with no tail** — the capture an element that died mid-encode posts. This is
        // the case a start-marker-only check stored, and it renders as a broken image that reports nothing.
        vec![0xff, 0xd8, 0xff, 0xe0, 0x42, 0x42],
        // A PNG, declared as a JPEG.
        vec![0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a],
    ] {
        let answer = ask(
            server.port(),
            &capture_request(&server, "image/jpeg", &body),
        )
        .await;
        assert_ne!(answer.status, 204, "{body:?} must not be stored");
    }
    assert_eq!(sink.frames(), Vec::new());

    server.shutdown().await;
}

#[tokio::test]
async fn a_capture_without_the_session_s_token_is_refused() {
    // The same credential as the media route, and the same answer: this is the route that writes, so a
    // route that could be reached without it would be the worse of the two to get wrong.
    let (_fixture, sink, server) = capturing().await;
    let frame = jpeg();
    let host = format!("127.0.0.1:{}", server.port());

    let wrong = token('b');
    for path in [
        format!("/thumbnail/{}", wrong.as_str()),
        "/thumbnail/".to_owned(),
        format!("/thumbnail/{}", server.token().as_str().to_uppercase()),
    ] {
        let body = frame.clone();
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body.iter()
                .map(|byte| char::from(*byte))
                .collect::<String>()
        );
        let answer = ask(server.port(), &bytes(request)).await;
        assert_ne!(answer.status, 204, "{path}");
    }
    assert_eq!(sink.frames(), Vec::new());

    server.shutdown().await;
}

#[tokio::test]
async fn a_capture_from_a_page_that_rebound_dns_is_refused() {
    let (_fixture, sink, server) = capturing().await;
    let frame = jpeg();
    let body: String = frame.iter().map(|byte| char::from(*byte)).collect();
    let request = format!(
        "POST /thumbnail/{} HTTP/1.1\r\nHost: evil.com\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        server.token().as_str(),
        frame.len()
    );

    let answer = ask(server.port(), &bytes(request)).await;

    assert_eq!(answer.status, 403);
    assert_eq!(sink.frames(), Vec::new());

    server.shutdown().await;
}

#[tokio::test]
async fn a_session_with_nowhere_to_put_a_frame_offers_no_capture_route() {
    // The media-only server, which is what a caller with no image cache gets. The route answers `404`
    // rather than existing and failing, so "this session cannot keep a poster" is a fact about the
    // session rather than about a request.
    let (_fixture, server) = served(1024).await;

    let answer = ask(
        server.port(),
        &capture_request(&server, "image/jpeg", &jpeg()),
    )
    .await;

    assert_eq!(answer.status, 404);
    // And the media route on the same server is untouched: the two capabilities are independent.
    assert_eq!(ok(&server, &[]).await.status, 200);

    server.shutdown().await;
}

#[tokio::test]
async fn a_sink_that_refuses_is_reported_rather_than_answered_with_success() {
    // A cache that will not take the frame is not a stored frame, and the renderer's retry decision
    // depends on the difference.
    let fixture = Fixture::new(1024);
    let sink = Arc::new(RecordingSink::refusing());
    let server = MediaServer::start(
        &fixture.safe(),
        token('a'),
        Vec::new(),
        Some(Capture {
            title_id: 7,
            sink: Arc::clone(&sink) as Arc<dyn ThumbnailSink>,
        }),
    )
    .await
    .expect("the fixture is servable");

    let answer = ask(
        server.port(),
        &capture_request(&server, "image/jpeg", &jpeg()),
    )
    .await;

    assert_ne!(answer.status, 204);
    assert_eq!(sink.frames(), Vec::new());

    server.shutdown().await;
}

#[tokio::test]
async fn a_frame_larger_than_this_build_will_read_is_refused() {
    // The bound is a bound: a route a page can be made to fill this process's memory through would be a
    // denial of service on the machine the user is sitting at.
    let (_fixture, sink, server) = capturing().await;
    let mut huge = jpeg();
    huge.resize(crate::server::MAX_FRAME_BYTES + 1, 0x42);

    let answer = ask(
        server.port(),
        &capture_request(&server, "image/jpeg", &huge),
    )
    .await;

    assert_eq!(answer.status, 413);
    assert_eq!(sink.frames(), Vec::new());

    server.shutdown().await;
}

#[tokio::test]
async fn the_capture_route_is_readable_by_the_renderer_that_captured_the_frame() {
    // **CORS, and the one thing it is for.** A frame drawn from a cross-origin `<video>` taints the
    // canvas, and a tainted canvas refuses `toBlob` — so without this header the renderer cannot turn a
    // frame of the film into the film's poster at all. Asserted on the media response, which is the one
    // the canvas reads.
    let (_fixture, _sink, server) = capturing().await;

    let answer = ok(&server, &[]).await;

    assert_eq!(
        answer.header("access-control-allow-origin"),
        Some("*"),
        "a canvas cannot read a frame it is not allowed to read"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn a_cross_origin_media_request_may_ask_for_a_range() {
    // The player sets `crossorigin` so its text tracks are readable, which puts the media request
    // under CORS; a browser seeking that media sends a Range request. Without Range on the
    // preflight the element paints an error over a file the server can serve.
    let (_fixture, _sink, server) = capturing().await;
    let path = format!("/media/{}", server.token().as_str());
    let host = format!("127.0.0.1:{}", server.port());

    let answer = ask(
        server.port(),
        &bytes(request(
            "OPTIONS",
            &path,
            Some(&host),
            &[
                ("Origin", "http://tauri.localhost"),
                ("Access-Control-Request-Method", "GET"),
                ("Access-Control-Request-Headers", "range"),
            ],
        )),
    )
    .await;

    assert_eq!(answer.status, 200);
    assert!(
        answer
            .header("access-control-allow-headers")
            .unwrap_or("")
            .to_ascii_lowercase()
            .contains("range"),
        "a cross-origin video must be allowed to ask for a range"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn a_frame_may_declare_a_jpeg_with_parameters() {
    // `image/jpeg; charset=binary` is the same declaration as `image/jpeg`, and a check that refused it
    // would be refusing a correct client.
    let (_fixture, sink, server) = capturing().await;

    let answer = ask(
        server.port(),
        &capture_request(&server, "image/jpeg; charset=binary", &jpeg()),
    )
    .await;

    assert_eq!(answer.status, 204);
    assert_eq!(sink.frames().len(), 1);

    server.shutdown().await;
}
/// A session over the fixture with one sidecar subtitle registered.
async fn subtitled() -> (Fixture, SidecarSubtitle, MediaServer) {
    let fixture = Fixture::new(512);
    let path = fixture.directory.path().join("film.en.srt");
    std::fs::write(&path, b"1\n00:00:01,000 --> 00:00:02,000\nHello\n")
        .expect("the sidecar writes");
    let safe = PathGuard::new(fixture.directory.path())
        .expect("the scratch directory is absolute")
        .resolve(&path)
        .expect("the sidecar is inside its own root");
    let sidecar = SidecarSubtitle {
        track_id: 11,
        path: safe,
        content_type: "application/x-subrip",
    };
    let server = MediaServer::start(&fixture.safe(), token('a'), vec![sidecar.clone()], None)
        .await
        .expect("the fixture is servable");
    (fixture, sidecar, server)
}

#[tokio::test]
async fn a_sidecar_subtitle_is_served_over_the_session_s_token() {
    // The whole reason the sidecar route lives on this listener: the renderer's track element needs a
    // URL, and the bytes must not travel over the asset protocol or IPC (AGENTS R11).
    let (_fixture, _sidecar, server) = subtitled().await;
    let path = format!("/subtitle/{}/11", server.token().as_str());

    let answer = ask(
        server.port(),
        &bytes(request(
            "GET",
            &path,
            Some(&format!("127.0.0.1:{}", server.port())),
            &[],
        )),
    )
    .await;

    assert_eq!(answer.status, 200);
    assert_eq!(answer.body, b"1\n00:00:01,000 --> 00:00:02,000\nHello\n");
    assert_eq!(answer.header("content-type"), Some("application/x-subrip"));
    assert_eq!(
        answer.header("cache-control"),
        Some("no-store"),
        "a sidecar is a response like any other under SECURITY.md T10"
    );
    assert_eq!(answer.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(
        answer.header("access-control-allow-origin"),
        Some("*"),
        "a cross-origin track element reads the response with CORS"
    );
    assert_eq!(
        server.subtitle_url(11),
        format!("http://127.0.0.1:{}{path}", server.port()),
        "the URL the core hands the renderer is the route the server serves"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn a_subtitle_id_the_session_does_not_offer_is_refused() {
    // The route takes an id and never a path, so an id the session was not opened with has no file
    // behind it and is the same answer as a route that does not exist.
    let (_fixture, _sidecar, server) = subtitled().await;
    let port = server.port();
    let host = format!("127.0.0.1:{port}");
    let right = server.token().as_str();

    for track in ["12", "notanumber", "-1", "0"] {
        let answer = ask(
            port,
            &bytes(request(
                "GET",
                &format!("/subtitle/{right}/{track}"),
                Some(&host),
                &[],
            )),
        )
        .await;
        assert_eq!(answer.status, 404, "track {track}");
    }

    // A session opened without sidecars offers none of them, even for an id another session serves.
    let plain = MediaServer::start(&Fixture::new(64).safe(), token('b'), Vec::new(), None)
        .await
        .expect("the fixture is servable");
    let answer = ask(
        plain.port(),
        &bytes(request(
            "GET",
            &format!("/subtitle/{}/11", plain.token().as_str()),
            Some(&format!("127.0.0.1:{}", plain.port())),
            &[],
        )),
    )
    .await;
    assert_eq!(answer.status, 404);

    server.shutdown().await;
    plain.shutdown().await;
}

#[tokio::test]
async fn a_sidecar_needs_the_same_token_and_the_same_loopback_host_as_the_media() {
    let (_fixture, _sidecar, server) = subtitled().await;
    let port = server.port();
    let path = format!("/subtitle/{}/11", server.token().as_str());

    let wrong = token('b');
    for offered in [
        format!("/subtitle/{}/11", wrong.as_str()),
        "/subtitle/".to_owned(),
        format!("/subtitle/{}/11/extra", server.token().as_str()),
    ] {
        let answer = ask(
            port,
            &bytes(request(
                "GET",
                &offered,
                Some(&format!("127.0.0.1:{port}")),
                &[],
            )),
        )
        .await;
        assert_ne!(answer.status, 200, "{offered}");
    }

    // A page that rebound DNS to loopback is refused before the token is read, exactly as on the media
    // route. The correct token is used so a pass cannot be the token check doing the work.
    for host in ["evil.com", "127.0.0.1.evil.com"] {
        let answer = ask(port, &bytes(request("GET", &path, Some(host), &[]))).await;
        assert_eq!(answer.status, 403, "Host: {host}");
    }

    server.shutdown().await;
}

#[tokio::test]
async fn a_sidecar_that_is_gone_is_refused_when_it_is_read() {
    // The row outlives the file: a scan wrote it and something removed the sidecar since. The track list
    // the renderer holds is the stale half, and the request must answer 404 rather than an empty track.
    let (fixture, _sidecar, server) = subtitled().await;
    std::fs::remove_file(fixture.directory.path().join("film.en.srt"))
        .expect("the sidecar removes");

    let answer = ask(
        server.port(),
        &bytes(request(
            "GET",
            &format!("/subtitle/{}/11", server.token().as_str()),
            Some(&format!("127.0.0.1:{}", server.port())),
            &[],
        )),
    )
    .await;

    assert_eq!(answer.status, 404);

    server.shutdown().await;
}

#[test]
fn a_sidecar_outside_its_root_has_no_safe_path_to_register() {
    // The route never takes a path, so containment is decided before a session opens: a row that names
    // a file outside every root cannot produce the SafePath the server's constructor requires.
    let fixture = Fixture::new(64);
    let guard = PathGuard::new(fixture.directory.path()).expect("an absolute root");

    let refused = guard
        .resolve(Path::new("/etc/passwd"))
        .expect_err("a sidecar outside the root has no SafePath");

    assert_ne!(refused.to_string(), "");
}
