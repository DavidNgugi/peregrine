//! Torrent statistics, in units that cannot be misread.
//!
//! A rate written as a float of megabytes per second is a number a caller can format the wrong way
//! and a test cannot compare exactly; every figure here is a count of bytes or a count of peers, so
//! the unit is the type (AGENTS §5, `PG-E10-07`). A byte offset and a byte count are the same
//! quantity, so the read-ahead window of
//! [`StreamRequest`](crate::StreamRequest) is a [`Bytes`] too.

use std::time::Duration;

use peregrine_domain::InfoHash;

pub use peregrine_domain::transfer::{ByteRate, Bytes};

/// How many peers a torrent is connected to, and how many of them are seeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct PeerCounts {
    connected: u32,
    seeds: u32,
}

impl PeerCounts {
    /// Counts of `connected` peers, `seeds` of which report a complete copy.
    ///
    /// A seed is a connected peer, so a seed count above the connection count is reported as the
    /// connection count: a statistics record is what a screen draws, and "3 seeds of 2 peers" is
    /// not a picture any screen can draw. The number is clamped here rather than refused because
    /// an engine's seed count is a report, not a request a caller could correct.
    #[must_use]
    pub const fn new(connected: u32, seeds: u32) -> Self {
        Self {
            connected,
            seeds: if seeds > connected { connected } else { seeds },
        }
    }

    /// The peers this torrent is connected to.
    #[must_use]
    pub const fn connected(self) -> u32 {
        self.connected
    }

    /// How many of those report a complete copy.
    #[must_use]
    pub const fn seeds(self) -> u32 {
        self.seeds
    }
}

/// Uploaded bytes against downloaded bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ratio {
    uploaded: Bytes,
    downloaded: Bytes,
}

impl Ratio {
    /// The ratio of what was uploaded to what was downloaded.
    #[must_use]
    pub const fn new(uploaded: Bytes, downloaded: Bytes) -> Self {
        Self {
            uploaded,
            downloaded,
        }
    }

    /// The bytes sent back.
    #[must_use]
    pub const fn uploaded(self) -> Bytes {
        self.uploaded
    }

    /// The bytes received.
    #[must_use]
    pub const fn downloaded(self) -> Bytes {
        self.downloaded
    }

    /// The ratio in hundredths — 200 is 2.00 — or `None` before anything was downloaded.
    ///
    /// `None` rather than zero, because nothing downloaded yet is not a ratio of zero and a
    /// seeding policy that read one would stop a torrent the moment it started. Hundredths rather
    /// than a float, because a target of 2.00 is the integer 200 and an integer comparison is what
    /// a policy wants (`AGENTS` §5). A ratio too large for the return type saturates rather than
    /// wrapping: it is a report, and a wrapped one would read as a small number.
    #[must_use]
    pub fn hundredths(self) -> Option<u32> {
        let downloaded = self.downloaded.get();
        if downloaded == 0 {
            return None;
        }
        let hundredths = self.uploaded.get().saturating_mul(100) / downloaded;
        Some(u32::try_from(hundredths).unwrap_or(u32::MAX))
    }
}

/// What a torrent has moved, and how fast.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Transfer {
    downloaded: Bytes,
    uploaded: Bytes,
    download_rate: ByteRate,
    upload_rate: ByteRate,
}

impl Transfer {
    /// The four figures one progress tick carries (`FLOWS` F6).
    #[must_use]
    pub const fn new(
        downloaded: Bytes,
        uploaded: Bytes,
        download_rate: ByteRate,
        upload_rate: ByteRate,
    ) -> Self {
        Self {
            downloaded,
            uploaded,
            download_rate,
            upload_rate,
        }
    }

    /// Payload bytes received so far.
    #[must_use]
    pub const fn downloaded(self) -> Bytes {
        self.downloaded
    }

    /// Payload bytes sent back so far.
    #[must_use]
    pub const fn uploaded(self) -> Bytes {
        self.uploaded
    }

    /// The current download rate.
    #[must_use]
    pub const fn download_rate(self) -> ByteRate {
        self.download_rate
    }

    /// The current upload rate.
    #[must_use]
    pub const fn upload_rate(self) -> ByteRate {
        self.upload_rate
    }

    /// What was uploaded against what was downloaded, computed rather than carried: the two byte
    /// counts are the facts, and a third field could disagree with them.
    #[must_use]
    pub const fn ratio(self) -> Ratio {
        Ratio::new(self.uploaded, self.downloaded)
    }
}

/// How a torrent is doing, as the piece-starvation rules see it (`ARCHITECTURE` §15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TorrentHealth {
    /// Peers are connected and pieces are arriving.
    Progressing,
    /// Peers are connected, but nothing has arrived for long enough to be a stall.
    Stalled,
    /// There is nothing connected to download from, which is the condition that carries DHT,
    /// tracker, and port detail rather than a bare spinner.
    NoPeers,
}

/// What one torrent's counters say.
///
/// Whether the payload is complete is read from [`TorrentStats::transfer`] and
/// [`TorrentStats::total`] rather than from [`TorrentHealth`], because the two facts are different:
/// a seed that is uploading is healthy and complete, and a transfer that has stalled is unhealthy
/// and probably not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentStats {
    info_hash: InfoHash,
    transfer: Transfer,
    total: Bytes,
    peers: PeerCounts,
    eta: Option<Duration>,
    health: TorrentHealth,
}

impl TorrentStats {
    /// One torrent's figures.
    ///
    /// `eta` is `None` when there is no estimate — no rate to divide by, or nothing left to
    /// download — because an absent estimate is not an estimate of zero.
    #[must_use]
    pub const fn new(
        info_hash: InfoHash,
        transfer: Transfer,
        total: Bytes,
        peers: PeerCounts,
        eta: Option<Duration>,
        health: TorrentHealth,
    ) -> Self {
        Self {
            info_hash,
            transfer,
            total,
            peers,
            eta,
            health,
        }
    }

    /// Which torrent these figures are about.
    ///
    /// A caller matches this against the job's stored hash; the value itself never reaches a log
    /// line, because [`InfoHash`]'s `Debug` prints nothing of it.
    #[must_use]
    pub const fn info_hash(&self) -> InfoHash {
        self.info_hash
    }

    /// What has moved, and how fast.
    #[must_use]
    pub const fn transfer(&self) -> Transfer {
        self.transfer
    }

    /// The payload's full size, as the metadata reports it.
    #[must_use]
    pub const fn total(&self) -> Bytes {
        self.total
    }

    /// The peers this torrent is connected to.
    #[must_use]
    pub const fn peers(&self) -> PeerCounts {
        self.peers
    }

    /// How long the transfer is expected to take, when there is an estimate.
    #[must_use]
    pub const fn eta(&self) -> Option<Duration> {
        self.eta
    }

    /// Whether pieces are arriving, stalled, or impossible for want of peers.
    #[must_use]
    pub const fn health(&self) -> TorrentHealth {
        self.health
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ratio_with_nothing_downloaded_is_absent_rather_than_zero() {
        assert_eq!(Ratio::new(Bytes::new(0), Bytes::new(0)).hundredths(), None);
        assert_eq!(
            Ratio::new(Bytes::new(5_000), Bytes::new(0)).hundredths(),
            None,
            "an upload with nothing downloaded is still not a ratio of zero"
        );
    }

    #[test]
    fn a_ratio_is_whole_hundredths_at_its_boundaries() {
        let cases = [
            (0, 1, 0),
            (1, 100, 1),
            (1, 2, 50),
            (1, 1, 100),
            (2, 1, 200),
            (3, 1, 300),
            (1, 3, 33),
        ];
        for (uploaded, downloaded, expected) in cases {
            assert_eq!(
                Ratio::new(Bytes::new(uploaded), Bytes::new(downloaded)).hundredths(),
                Some(expected),
                "{uploaded} uploaded against {downloaded} downloaded"
            );
        }
    }

    #[test]
    fn an_enormous_ratio_saturates_rather_than_wrapping() {
        assert_eq!(
            Ratio::new(Bytes::new(u64::MAX), Bytes::new(1)).hundredths(),
            Some(u32::MAX)
        );
    }

    #[test]
    fn a_transfer_computes_its_own_ratio() {
        let transfer = Transfer::new(
            Bytes::new(1_000),
            Bytes::new(2_000),
            ByteRate::per_second(100),
            ByteRate::per_second(50),
        );
        assert_eq!(transfer.downloaded(), Bytes::new(1_000));
        assert_eq!(transfer.uploaded(), Bytes::new(2_000));
        assert_eq!(transfer.download_rate(), ByteRate::per_second(100));
        assert_eq!(transfer.upload_rate(), ByteRate::per_second(50));
        assert_eq!(transfer.ratio().hundredths(), Some(200));
    }

    #[test]
    fn a_seed_count_above_the_connection_count_is_reported_as_the_connection_count() {
        assert_eq!(PeerCounts::new(5, 2).seeds(), 2);
        assert_eq!(PeerCounts::new(5, 5).seeds(), 5);
        assert_eq!(PeerCounts::new(2, 3).seeds(), 2);
        assert_eq!(PeerCounts::new(0, 4).seeds(), 0);
    }

    #[test]
    fn stats_answer_with_the_hash_they_were_given_without_printing_it() {
        use peregrine_domain::InfoHash;

        let hash =
            InfoHash::from_hex("0123456789abcdef0123456789abcdef01234567").expect("a valid hash");
        let stats = TorrentStats::new(
            hash,
            Transfer::default(),
            Bytes::new(4_000),
            PeerCounts::new(3, 1),
            Some(Duration::from_secs(10)),
            TorrentHealth::Progressing,
        );

        assert_eq!(stats.info_hash(), hash);
        assert_eq!(stats.total(), Bytes::new(4_000));
        assert_eq!(stats.peers().connected(), 3);
        assert_eq!(stats.eta(), Some(Duration::from_secs(10)));
        assert_eq!(stats.health(), TorrentHealth::Progressing);

        let debug = format!("{stats:?}");
        assert!(
            !debug.contains(&hash.to_hex()),
            "a formatted record carried the hash: {debug}"
        );
    }

    #[test]
    fn a_completed_torrent_reads_as_complete_from_the_counts_not_from_health() {
        // The three health values say nothing about completion, so a caller reads
        // `transfer().downloaded()` against `total()`. This pins that a seed can be complete and
        // healthy at once, which is the case a health-only reader would get wrong.
        let stats = TorrentStats::new(
            peregrine_domain::InfoHash::from_bytes([0; 20]),
            Transfer::new(
                Bytes::new(10),
                Bytes::new(30),
                ByteRate::per_second(0),
                ByteRate::per_second(9),
            ),
            Bytes::new(10),
            PeerCounts::new(1, 1),
            None,
            TorrentHealth::Progressing,
        );
        assert_eq!(stats.transfer().downloaded(), stats.total());
        assert!(stats.eta().is_none(), "a complete transfer has no ETA");
        assert_eq!(stats.transfer().ratio().hundredths(), Some(300));
    }
}
