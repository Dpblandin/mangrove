//! Downloads an archive and checks its minisign signature while it arrives,
//! before anything is installed.
use crate::{http, Asset, Result, UpdateError};
use minisign_verify::{PublicKey, Signature};
use std::{
    fs::File,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
use tempfile::TempDir;

/// The public half of the key that signs release archives, as written by
/// `minisign -G`. The release workflow holds the secret half.
const RELEASE_KEY: &str = include_str!("../release-key.pub");
const MAX_SIGNATURE_BYTES: u64 = 4096;
const CHUNK_BYTES: usize = 64 * 1024;

/// Shared with the interface while an archive downloads.
#[derive(Debug, Default)]
pub struct Progress {
    received: AtomicU64,
    total: AtomicU64,
    cancelled: AtomicBool,
}

impl Progress {
    pub fn received(&self) -> u64 {
        self.received.load(Ordering::Relaxed)
    }

    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }

    /// Stops the download before its next chunk.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(UpdateError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// A verified archive, deleted with its temporary folder when dropped.
#[derive(Debug)]
pub struct Download {
    archive: PathBuf,
    _directory: TempDir,
}

impl Download {
    pub fn archive(&self) -> &Path {
        &self.archive
    }
}

pub fn download(asset: &Asset, progress: &Progress) -> Result<Download> {
    let key = PublicKey::decode(RELEASE_KEY).map_err(|_| UpdateError::MissingKey)?;
    progress.check()?;
    let signature = http::read_body(
        http::get(&asset.signature, "application/octet-stream")?,
        MAX_SIGNATURE_BYTES,
    )?;
    let signature = signature_for(&signature, &asset.name)?;
    progress.check()?;
    let directory = tempfile::Builder::new()
        .prefix("mangrove-update-")
        .tempdir()?;
    let archive = directory.path().join(&asset.name);
    let response = http::get(&asset.url, "application/octet-stream")?;
    if response
        .content_length()
        .is_some_and(|length| length != asset.size)
    {
        return Err(UpdateError::UnexpectedSize);
    }
    let file = File::create(&archive)?;
    copy_verified(response, file, &key, &signature, asset.size, progress)?;
    Ok(Download {
        archive,
        _directory: directory,
    })
}

/// The signature of the archive `name`. Its trusted comment, signed as well,
/// must be that name: an archive cannot pass for another version or platform.
fn signature_for(text: &[u8], name: &str) -> Result<Signature> {
    let signature = std::str::from_utf8(text)
        .ok()
        .and_then(|text| Signature::decode(text).ok())
        .ok_or(UpdateError::InvalidSignature)?;
    if signature.trusted_comment() != name {
        return Err(UpdateError::InvalidSignature);
    }
    Ok(signature)
}

/// Copies exactly `size` bytes, then accepts them only if `signature` holds.
fn copy_verified(
    mut reader: impl Read,
    mut writer: impl Write,
    key: &PublicKey,
    signature: &Signature,
    size: u64,
    progress: &Progress,
) -> Result<()> {
    let mut verifier = key
        .verify_stream(signature)
        .map_err(|_| UpdateError::InvalidSignature)?;
    progress.total.store(size, Ordering::Relaxed);
    progress.received.store(0, Ordering::Relaxed);
    let mut buffer = vec![0; CHUNK_BYTES];
    let mut received = 0_u64;
    loop {
        progress.check()?;
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(http::read_error(error)),
        };
        received += read as u64;
        if received > size {
            return Err(UpdateError::UnexpectedSize);
        }
        verifier.update(&buffer[..read]);
        writer.write_all(&buffer[..read])?;
        progress.received.store(received, Ordering::Relaxed);
    }
    if received != size {
        return Err(UpdateError::UnexpectedSize);
    }
    writer.flush()?;
    verifier
        .finalize()
        .map_err(|_| UpdateError::InvalidSignature)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAME: &str = "mangrove-0.4.0-macos-arm64.dmg";
    const ARCHIVE: &[u8] = b"Mangrove test archive\n";
    // A test key pair made with `minisign -G -W`; its secret half was discarded.
    const TEST_KEY: &str = "untrusted comment: minisign public key BA596F1B8E0153A0
RWSgUwGOG29ZurEpDjwyVeLFmLzEg6ddQnyLq5Xo9uYLHrFvjgS2od5m
";
    const SIGNED: &str = "untrusted comment: signature from minisign secret key
RUSgUwGOG29ZuqRS/adGduYzbxhp/yd6nnfhlXzhHo5TjYTN8cLLatQR3SFMKpJawJjMnNOwYuFxZi8Gk72Hd5IdnJFrL6qa9QM=
trusted comment: mangrove-0.4.0-macos-arm64.dmg
fy1FVB/qRSdZl91qd+RBQxNgKvfCBtrJMWUhKCIRSrLEKuoyxEyFZ6JGd5inh/60Jg83JYTciUDBXCpowUJFCg==
";
    /// The same archive, signed for the name of an older release.
    const RENAMED: &str = "untrusted comment: signature from minisign secret key
RUSgUwGOG29ZuqRS/adGduYzbxhp/yd6nnfhlXzhHo5TjYTN8cLLatQR3SFMKpJawJjMnNOwYuFxZi8Gk72Hd5IdnJFrL6qa9QM=
trusted comment: mangrove-0.3.0-macos-arm64.dmg
31FD0XM0rpGCf5NBKkxc6/N/RXnGvhU4SfDb5l8z9ZFhk/d7UaQ9ZFvgP4517k9SnAxrwWeYbyJ1poqaioIRBA==
";
    /// `minisign -l`: the archive itself is signed, not its hash.
    const LEGACY: &str = "untrusted comment: signature from minisign secret key
RWSgUwGOG29ZujjshqJudbYdPPp9h8iZZ0FUHQWjWrv2yhwHZfYZOzKL66S24hXU4ES+3mcFwxaV3vYJAd1xqKnpC95JUcLBoQ0=
trusted comment: mangrove-0.4.0-macos-arm64.dmg
l8BURejf+h+1XRx19lQd+UJ+G1NZZv70J9Wm3M2r4MJ0SCaDj5zzImkZ+0eK47CqtnJvTFWrPgXW0kdDh8qICg==
";
    const OTHER_KEY: &str = "untrusted comment: signature from minisign secret key
RUSu31csh5i1kC3Q0KH8SbeJ1u575FrtGYadRCcJRnq3+h+tsmsAulDhyxRnn5bBe3hkOL4rafVv2xnmzwV7kCRUn2CJbKyQ7Ag=
trusted comment: mangrove-0.4.0-macos-arm64.dmg
CPBnbSHdMmuQnPlAQzMyDufWqS6b2+ZCpYCCYqsOr3jNaWREBt+CuOQxQYFkdxtsmIZSOKuo3K+a1cQoOHPwCg==
";

    fn verify(archive: &[u8], signature: &str, size: u64) -> Result<Vec<u8>> {
        let key = PublicKey::decode(TEST_KEY).unwrap();
        let signature = signature_for(signature.as_bytes(), NAME)?;
        let mut copy = Vec::new();
        copy_verified(
            archive,
            &mut copy,
            &key,
            &signature,
            size,
            &Progress::default(),
        )?;
        Ok(copy)
    }

    #[test]
    fn the_release_key_decodes() {
        PublicKey::decode(RELEASE_KEY).expect("release-key.pub holds a minisign public key");
    }

    #[test]
    fn a_signed_archive_is_copied_and_progress_reaches_its_size() {
        let key = PublicKey::decode(TEST_KEY).unwrap();
        let signature = signature_for(SIGNED.as_bytes(), NAME).unwrap();
        let progress = Progress::default();
        let mut copy = Vec::new();
        copy_verified(
            ARCHIVE,
            &mut copy,
            &key,
            &signature,
            ARCHIVE.len() as u64,
            &progress,
        )
        .unwrap();
        assert_eq!(copy, ARCHIVE);
        assert_eq!(
            (progress.received(), progress.total()),
            (ARCHIVE.len() as u64, ARCHIVE.len() as u64)
        );
    }

    #[test]
    fn altered_truncated_or_extended_archives_are_refused() {
        let size = ARCHIVE.len() as u64;
        let mut altered = ARCHIVE.to_vec();
        altered[0] ^= 1;
        assert!(matches!(
            verify(&altered, SIGNED, size),
            Err(UpdateError::InvalidSignature)
        ));
        assert!(matches!(
            verify(&ARCHIVE[1..], SIGNED, size),
            Err(UpdateError::UnexpectedSize)
        ));
        let extended = [ARCHIVE, b"x"].concat();
        assert!(matches!(
            verify(&extended, SIGNED, size),
            Err(UpdateError::UnexpectedSize)
        ));
    }

    #[test]
    fn signatures_from_another_key_name_or_mode_are_refused() {
        let size = ARCHIVE.len() as u64;
        for signature in [RENAMED, LEGACY, OTHER_KEY, "", "garbage"] {
            assert!(
                matches!(
                    verify(ARCHIVE, signature, size),
                    Err(UpdateError::InvalidSignature)
                ),
                "{signature}"
            );
        }
        // The trusted comment is signed: renaming it breaks the signature.
        let forged = RENAMED.replace("mangrove-0.3.0", "mangrove-0.4.0");
        assert!(matches!(
            verify(ARCHIVE, &forged, size),
            Err(UpdateError::InvalidSignature)
        ));
    }

    #[test]
    fn a_cancelled_download_stops_before_reading() {
        let key = PublicKey::decode(TEST_KEY).unwrap();
        let signature = signature_for(SIGNED.as_bytes(), NAME).unwrap();
        let progress = Progress::default();
        progress.cancel();
        let mut copy = Vec::new();
        assert!(matches!(
            copy_verified(ARCHIVE, &mut copy, &key, &signature, 22, &progress),
            Err(UpdateError::Cancelled)
        ));
        assert!(copy.is_empty());
    }
}
