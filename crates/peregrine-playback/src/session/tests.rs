//! What a session promises: where it is served, what it is serving, and that it stops.

use peregrine_domain::{PathGuard, SafePath, SessionToken};
use tokio::net::TcpStream;

use super::{PlaybackError, PlaybackSession};

/// A sink that records nothing: what is under test is the URL's presence, not the store.
struct Nowhere;

impl peregrine_stream::ThumbnailSink for Nowhere {
    fn store(
        &self,
        _title_id: i64,
        _bytes: &[u8],
    ) -> Result<std::path::PathBuf, peregrine_stream::ThumbnailError> {
        Err(peregrine_stream::ThumbnailError::Refused)
    }
}

/// A token of the shape the minter produces, fixed so a test is not also testing a random source.
fn token() -> SessionToken {
    SessionToken::new("abc123".repeat(6)).expect("36 ASCII alphanumerics")
}

/// A scratch file, and the `SafePath` that may be served from it.
struct Fixture {
    directory: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("a scratch directory");
        std::fs::write(directory.path().join("film.mp4"), vec![7_u8; 4096])
            .expect("the fixture writes");
        Self { directory }
    }

    fn safe(&self) -> SafePath {
        PathGuard::new(self.directory.path())
            .expect("the scratch directory is absolute")
            .resolve(&self.directory.path().join("film.mp4"))
            .expect("the fixture is inside its own root")
    }
}

/// The port a session's URL names, so a test can reach the socket without parsing the URL.
fn port_of(url: &str) -> u16 {
    url.rsplit(':')
        .next()
        .and_then(|tail| tail.split('/').next())
        .expect("the url carries a port")
        .parse()
        .expect("the port is a number")
}

#[tokio::test]
async fn a_session_serves_the_file_it_was_opened_for_and_says_which_file_that_was() {
    let fixture = Fixture::new();

    let session = PlaybackSession::open(42, 7, &fixture.safe(), token(), None)
        .await
        .expect("the fixture is servable");

    assert_eq!(session.media_file_id(), 42);
    assert_eq!(session.title_id(), 7);
    assert!(
        session.url().starts_with("http://127.0.0.1:"),
        "the surface is loopback and nothing else: {}",
        session.url()
    );
    assert!(
        session
            .url()
            .ends_with(&format!("/media/{}", token().as_str())),
        "the token is in the path, because a media element cannot set a header: {}",
        session.url()
    );
    // The socket is really there, which is the half a URL cannot show.
    TcpStream::connect(("127.0.0.1", port_of(session.url())))
        .await
        .expect("the session is listening");

    assert!(session.close().await);
}

#[tokio::test]
async fn closing_a_session_closes_its_socket() {
    let fixture = Fixture::new();
    let session = PlaybackSession::open(1, 1, &fixture.safe(), token(), None)
        .await
        .expect("the fixture is servable");
    let port = port_of(session.url());

    session.close().await;

    assert!(
        TcpStream::connect(("127.0.0.1", port)).await.is_err(),
        "nothing is listening on {port} after the session closes"
    );
}

#[tokio::test]
async fn a_session_that_is_simply_dropped_also_stops_listening() {
    // The runtime replaces a session by assigning over it, and a listener left behind by the value that
    // used to own it would be a socket nobody holds.
    let fixture = Fixture::new();
    let session = PlaybackSession::open(1, 1, &fixture.safe(), token(), None)
        .await
        .expect("the fixture is servable");
    let port = port_of(session.url());

    drop(session);

    let mut stopped = false;
    for _ in 0..50 {
        if TcpStream::connect(("127.0.0.1", port)).await.is_err() {
            stopped = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        stopped,
        "port {port} is still listening after the session was dropped"
    );
}

#[tokio::test]
async fn the_capture_url_exists_exactly_when_the_caller_brought_a_place_to_put_a_frame() {
    // Two facts a renderer reads before it starts capturing: whether there is a route, and where it is.
    // A session that answered a URL it would refuse would make every play post a frame into a 404.
    let fixture = Fixture::new();

    let without = PlaybackSession::open(1, 1, &fixture.safe(), token(), None)
        .await
        .expect("the fixture is servable");
    assert_eq!(without.thumbnail_url(), None);

    let with = PlaybackSession::open(
        1,
        1,
        &fixture.safe(),
        token(),
        Some(std::sync::Arc::new(Nowhere)),
    )
    .await
    .expect("the fixture is servable");
    let url = with.thumbnail_url().expect("this session keeps a poster");
    assert!(
        url.starts_with("http://127.0.0.1:") && url.contains("/thumbnail/"),
        "the token is in the path here too: {url}"
    );
    assert_ne!(url, with.url(), "the two routes are different addresses");

    without.close().await;
    with.close().await;
}

#[tokio::test]
async fn a_file_that_is_gone_is_refused_when_the_session_opens() {
    // The fact belongs beside the control the user pressed. `media_file.missing_since` is what a scan
    // saw, and a file can move between the scan and the press.
    let fixture = Fixture::new();
    let safe = fixture.safe();
    std::fs::remove_file(fixture.directory.path().join("film.mp4")).expect("the fixture removes");

    let refused = PlaybackSession::open(1, 1, &safe, token(), None)
        .await
        .expect_err("a gone file cannot be served");

    assert!(
        matches!(refused, PlaybackError::Serve(_)),
        "and the reason is the stream layer's, unwrapped: {refused:?}"
    );
}

#[tokio::test]
async fn two_sessions_are_two_sockets_with_two_tokens() {
    // Play-then-play-again is the ordinary case: a second press must not be served from the first
    // session's listener, and it must not fail because the first one still holds a port.
    let first = Fixture::new();
    let second = Fixture::new();

    let a = PlaybackSession::open(1, 1, &first.safe(), token(), None)
        .await
        .expect("the first session opens");
    let b = PlaybackSession::open(
        2,
        8,
        &second.safe(),
        SessionToken::new("z".repeat(32)).unwrap(),
        None,
    )
    .await
    .expect("the second session opens");

    assert_ne!(port_of(a.url()), port_of(b.url()));
    assert_ne!(a.token(), b.token());
    assert_eq!(a.media_file_id(), 1);
    assert_eq!(b.media_file_id(), 2);
    assert_eq!(b.title_id(), 8);

    a.close().await;
    b.close().await;
}
