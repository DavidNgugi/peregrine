//! The engine-backed media source a downloading title is played through.
//!
//! The loopback server knows only the `TorrentMedia` trait (its own crate must not depend on the
//! engine, `AGENTS R8`), so this module is where the two meet: it holds the session's handle, opens a
//! fresh reader per request — the port has no seek — and narrows the engine's chunk vocabulary to the
//! bytes a response body wants.

use std::future::Future;
use std::io;
use std::num::NonZeroU32;
use std::pin::Pin;
use std::sync::Arc;

use peregrine_stream::{MediaReader, TorrentMedia};
use peregrine_torrent::{
    Bytes, StreamChunk, StreamRequest, TorrentEngine, TorrentHandle, TorrentStream,
};

/// How far ahead of the reader the engine is asked to raise piece priority.
///
/// The port accepts a window and the librqbit adapter ignores it (that engine has no byte-window API),
/// so this is honest steering rather than a promise: it names the intent a streaming read has.
const READ_AHEAD: u64 = 4 * 1024 * 1024;

/// One selected file of a live torrent, opened at an offset per HTTP request.
pub struct EngineMedia {
    engine: Arc<dyn TorrentEngine>,
    handle: TorrentHandle,
    file: usize,
    len: u64,
    content_type: String,
}

impl EngineMedia {
    /// Builds a source over one selected file, taking the content type from the file's name.
    #[must_use]
    pub fn new(
        engine: Arc<dyn TorrentEngine>,
        handle: TorrentHandle,
        file: usize,
        len: u64,
        path: &str,
    ) -> Self {
        Self {
            engine,
            handle,
            file,
            len,
            content_type: content_type_for(path).to_owned(),
        }
    }
}

impl TorrentMedia for EngineMedia {
    fn len(&self) -> u64 {
        self.len
    }

    fn content_type(&self) -> &str {
        &self.content_type
    }

    fn open<'a>(
        &'a self,
        offset: u64,
    ) -> Pin<Box<dyn Future<Output = io::Result<Box<dyn MediaReader>>> + Send + 'a>> {
        let engine = Arc::clone(&self.engine);
        let request = StreamRequest::new(
            self.handle,
            self.file,
            Bytes::new(offset),
            Bytes::new(READ_AHEAD),
        );
        Box::pin(async move {
            let stream = engine
                .stream(&request)
                .await
                .map_err(|_| io::Error::other("the torrent could not be read"))?;
            Ok(Box::new(EngineReader(stream)) as Box<dyn MediaReader>)
        })
    }
}

/// The engine's reader, narrowed to the "next bytes or the end" a response body needs.
struct EngineReader(Box<dyn TorrentStream>);

impl MediaReader for EngineReader {
    fn read<'a>(
        &'a mut self,
        max: usize,
    ) -> Pin<Box<dyn Future<Output = io::Result<Option<Vec<u8>>>> + Send + 'a>> {
        Box::pin(async move {
            let max = NonZeroU32::new(u32::try_from(max).unwrap_or(u32::MAX).max(1))
                .ok_or_else(|| io::Error::other("a read of zero bytes"))?;
            match self.0.read(max).await {
                Ok(StreamChunk::Data(bytes)) => Ok(Some(bytes)),
                Ok(StreamChunk::End) => Ok(None),
                Err(_) => Err(io::Error::other("the torrent read failed")),
            }
        })
    }
}

/// The content type a media element is told, from the file's extension.
///
/// The webview's own demuxer is what plays this build's files (`ADR 0023`), and it reads `mp4`/`webm`;
/// anything else is labelled generically rather than guessed, because a wrong type is worse than none.
fn content_type_for(path: &str) -> &'static str {
    match path
        .rsplit('.')
        .next()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("mp4" | "m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mov") => "video/quicktime",
        _ => "application/octet-stream",
    }
}
