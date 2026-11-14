//! Reader for the single-file ZIP archives cameras serve their XML in.
//!
//! Only what locates and unpacks the first file is read: the end of central
//! directory record, the first central directory entry and its local file
//! header. The headers' extra fields are skipped by their length and never
//! interpreted: a Basler acA2440-20gc puts 20 bytes there that are not the
//! `(id, size, data)` records the ZIP format describes, and a reader that
//! parses them refuses the archive (issue #236).

use std::io::Read;

/// End of central directory record: signature and length without a comment.
const EOCD_SIGNATURE: &[u8; 4] = b"PK\x05\x06";
const EOCD_LEN: usize = 22;
/// Central directory file header: signature and fixed length.
const CENTRAL_SIGNATURE: &[u8; 4] = b"PK\x01\x02";
const CENTRAL_LEN: usize = 46;
/// Local file header: signature and fixed length.
const LOCAL_SIGNATURE: &[u8; 4] = b"PK\x03\x04";
const LOCAL_LEN: usize = 30;

/// Compression methods: stored and DEFLATE.
const METHOD_STORED: u16 = 0;
const METHOD_DEFLATE: u16 = 8;
/// General purpose flag bit 0: the file is encrypted.
const FLAG_ENCRYPTED: u16 = 1;

/// Why an archive could not be read.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ArchiveError {
    /// No end of central directory record: not a ZIP archive, or cut short.
    #[error("no end of central directory record")]
    NoDirectory,
    /// The archive lists no file.
    #[error("the archive is empty")]
    Empty,
    /// A header or the file's data lies outside the archive.
    #[error("the {0} lies outside the archive")]
    OutOfBounds(&'static str),
    /// A header does not start with its signature.
    #[error("the {0} has a wrong signature")]
    Signature(&'static str),
    /// The file is encrypted.
    #[error("the file is encrypted")]
    Encrypted,
    /// The archive uses the ZIP64 format.
    #[error("ZIP64 archives are not supported")]
    Zip64,
    /// The file is compressed with a method other than stored or DEFLATE.
    #[error("compression method {0} is not supported")]
    Method(u16),
    /// The file is larger than the caller's limit.
    #[error("the file exceeds {0} bytes")]
    TooLarge(u64),
    /// The compressed data is not valid DEFLATE.
    #[error("inflate: {0}")]
    Inflate(#[source] std::io::Error),
    /// The unpacked file does not have the declared CRC-32.
    #[error("the file's CRC-32 is {actual:#010x}, the archive declares {declared:#010x}")]
    Crc {
        /// CRC-32 in the central directory.
        declared: u32,
        /// CRC-32 of the unpacked bytes.
        actual: u32,
    },
}

/// Unpack the first file of the ZIP archive in `data`, refusing one larger
/// than `limit` bytes.
///
/// Bytes after the archive are ignored: a device serves the length its URL
/// advertises, which may be padded.
pub(crate) fn first_file(data: &[u8], limit: u64) -> Result<Vec<u8>, ArchiveError> {
    let (directory, entries) = find_directory(data)?;
    if entries == 0 {
        return Err(ArchiveError::Empty);
    }
    let central = slice(data, directory, CENTRAL_LEN, "central directory")?;
    if &central[..4] != CENTRAL_SIGNATURE {
        return Err(ArchiveError::Signature("central directory"));
    }
    let flags = u16_at(central, 8);
    let method = u16_at(central, 10);
    let declared_crc = u32_at(central, 16);
    let compressed = u32_at(central, 20);
    let uncompressed = u32_at(central, 24);
    let local_offset = u32_at(central, 42);
    if flags & FLAG_ENCRYPTED != 0 {
        return Err(ArchiveError::Encrypted);
    }
    if [compressed, uncompressed, local_offset].contains(&u32::MAX) {
        return Err(ArchiveError::Zip64);
    }
    if u64::from(uncompressed) > limit {
        return Err(ArchiveError::TooLarge(limit));
    }

    let local_offset = local_offset as usize;
    let local = slice(data, local_offset, LOCAL_LEN, "local file header")?;
    if &local[..4] != LOCAL_SIGNATURE {
        return Err(ArchiveError::Signature("local file header"));
    }
    // The name and the extra field are skipped, not read. Their lengths come
    // from the local header: they may differ from the central directory's.
    let name_len = usize::from(u16_at(local, 26));
    let extra_len = usize::from(u16_at(local, 28));
    let start = local_offset + LOCAL_LEN + name_len + extra_len;
    let packed = slice(data, start, compressed as usize, "file data")?;

    let file = match method {
        METHOD_STORED => packed.to_vec(),
        METHOD_DEFLATE => {
            let mut file = Vec::with_capacity(uncompressed as usize);
            // The declared size can lie and DEFLATE can expand past it, so
            // the read itself is bounded as well.
            flate2::read::DeflateDecoder::new(packed)
                .take(limit.saturating_add(1))
                .read_to_end(&mut file)
                .map_err(ArchiveError::Inflate)?;
            file
        }
        other => return Err(ArchiveError::Method(other)),
    };
    if file.len() as u64 > limit {
        return Err(ArchiveError::TooLarge(limit));
    }
    let mut crc = flate2::Crc::new();
    crc.update(&file);
    if crc.sum() != declared_crc {
        return Err(ArchiveError::Crc {
            declared: declared_crc,
            actual: crc.sum(),
        });
    }
    Ok(file)
}

/// Find the central directory: its offset and its number of entries.
///
/// The end of central directory record is the last one whose comment and
/// directory fit in `data`, so trailing padding and a comment that happens to
/// contain the signature do not mislead the search.
fn find_directory(data: &[u8]) -> Result<(usize, u16), ArchiveError> {
    let last = data
        .len()
        .checked_sub(EOCD_LEN)
        .ok_or(ArchiveError::NoDirectory)?;
    for pos in (0..=last).rev() {
        let record = &data[pos..pos + EOCD_LEN];
        if &record[..4] != EOCD_SIGNATURE {
            continue;
        }
        let entries = u16_at(record, 10);
        let size = u32_at(record, 12);
        let offset = u32_at(record, 16);
        let comment_len = usize::from(u16_at(record, 20));
        if size == u32::MAX || offset == u32::MAX {
            return Err(ArchiveError::Zip64);
        }
        let directory_end = u64::from(offset) + u64::from(size);
        if pos + EOCD_LEN + comment_len <= data.len() && directory_end <= pos as u64 {
            return Ok((offset as usize, entries));
        }
    }
    Err(ArchiveError::NoDirectory)
}

/// `len` bytes of `data` at `offset`, or an error naming `what` they hold.
fn slice<'a>(
    data: &'a [u8],
    offset: usize,
    len: usize,
    what: &'static str,
) -> Result<&'a [u8], ArchiveError> {
    offset
        .checked_add(len)
        .and_then(|end| data.get(offset..end))
        .ok_or(ArchiveError::OutOfBounds(what))
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The extra field of the Basler acA2440-20gc's XML archive, as reported
    /// on issue #236. Read as a ZIP extra record it is ID `0x4347` with a
    /// size of 12374 bytes, of which 16 follow.
    pub(crate) const BASLER_EXTRA: &[u8] = b"GCV0\x01\0\0\0\x01\0\0\0\x03\0\x08\0\0\0\0\0";

    /// A single-file DEFLATE archive whose headers carry `extra` as their
    /// extra field, written by hand: ZIP writers validate what they are given.
    pub(crate) fn zip_with_extra(name: &str, file: &[u8], extra: &[u8]) -> Vec<u8> {
        build(name, file, extra, METHOD_DEFLATE)
    }

    /// [`zip_with_extra`] declaring `method`: the data is DEFLATE unless the
    /// method is stored.
    fn build(name: &str, file: &[u8], extra: &[u8], method: u16) -> Vec<u8> {
        use std::io::Write;
        let packed = if method == METHOD_STORED {
            file.to_vec()
        } else {
            let mut encoder =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(file).expect("deflate");
            encoder.finish().expect("deflate")
        };
        let mut crc = flate2::Crc::new();
        crc.update(file);

        // Version needed, flags, method, time, date, CRC-32, sizes, lengths:
        // the fields the local and the central header share.
        let mut common = Vec::new();
        for field in [20u16, 0, method, 0, 0] {
            common.extend_from_slice(&field.to_le_bytes());
        }
        for field in [crc.sum(), packed.len() as u32, file.len() as u32] {
            common.extend_from_slice(&field.to_le_bytes());
        }
        for field in [name.len() as u16, extra.len() as u16] {
            common.extend_from_slice(&field.to_le_bytes());
        }

        let mut archive = LOCAL_SIGNATURE.to_vec();
        archive.extend_from_slice(&common);
        archive.extend_from_slice(name.as_bytes());
        archive.extend_from_slice(extra);
        archive.extend_from_slice(&packed);

        let directory = archive.len();
        archive.extend_from_slice(CENTRAL_SIGNATURE);
        archive.extend_from_slice(&0x0314u16.to_le_bytes()); // version made by
        archive.extend_from_slice(&common);
        // Comment length, disk, internal and external attributes, and the
        // local header's offset: all zero.
        archive.extend_from_slice(&[0; 14]);
        archive.extend_from_slice(name.as_bytes());
        archive.extend_from_slice(extra);
        let directory_len = archive.len() - directory;

        archive.extend_from_slice(EOCD_SIGNATURE);
        archive.extend_from_slice(&[0; 4]); // disk numbers
        archive.extend_from_slice(&1u16.to_le_bytes());
        archive.extend_from_slice(&1u16.to_le_bytes());
        archive.extend_from_slice(&(directory_len as u32).to_le_bytes());
        archive.extend_from_slice(&(directory as u32).to_le_bytes());
        archive.extend_from_slice(&0u16.to_le_bytes());
        archive
    }

    const LIMIT: u64 = 1 << 20;

    #[test]
    fn an_extra_field_that_is_not_zip_records_is_skipped() {
        let archive = zip_with_extra("camera.xml", b"<a/>", BASLER_EXTRA);
        // The archive is the one a strict reader refuses.
        let strict = zip::ZipArchive::new(std::io::Cursor::new(&archive));
        assert!(strict.is_err(), "the zip crate reads the archive");
        assert_eq!(first_file(&archive, LIMIT).unwrap(), b"<a/>");
    }

    #[test]
    fn an_archive_without_extra_fields_reads() {
        let archive = zip_with_extra("camera.xml", b"<a/>", &[]);
        assert_eq!(first_file(&archive, LIMIT).unwrap(), b"<a/>");
    }

    #[test]
    fn padding_after_the_archive_is_ignored() {
        let mut archive = zip_with_extra("camera.xml", b"<a/>", BASLER_EXTRA);
        archive.resize(archive.len() + 4096, 0);
        assert_eq!(first_file(&archive, LIMIT).unwrap(), b"<a/>");
    }

    #[test]
    fn a_stored_file_reads() {
        let archive = build("camera.xml", b"<stored/>", BASLER_EXTRA, METHOD_STORED);
        assert_eq!(first_file(&archive, LIMIT).unwrap(), b"<stored/>");
    }

    #[test]
    fn an_unknown_compression_method_is_refused() {
        let archive = build("camera.xml", b"<a/>", &[], 12);
        let err = first_file(&archive, LIMIT).unwrap_err();
        assert!(matches!(err, ArchiveError::Method(12)), "got: {err:?}");
    }

    #[test]
    fn a_wrong_crc_is_refused() {
        let mut archive = zip_with_extra("camera.xml", b"<a/>", &[]);
        let directory = archive
            .windows(4)
            .position(|w| w == CENTRAL_SIGNATURE)
            .unwrap();
        archive[directory + 16] ^= 0xFF;
        let err = first_file(&archive, LIMIT).unwrap_err();
        assert!(matches!(err, ArchiveError::Crc { .. }), "got: {err:?}");
    }

    #[test]
    fn a_file_over_the_limit_is_refused() {
        let archive = zip_with_extra("camera.xml", &[b' '; 4096], &[]);
        let err = first_file(&archive, 4095).unwrap_err();
        assert!(matches!(err, ArchiveError::TooLarge(4095)), "got: {err:?}");
    }

    #[test]
    fn a_file_that_inflates_past_its_declared_size_is_refused_at_the_limit() {
        let mut archive = zip_with_extra("camera.xml", &[b' '; 4096], &[]);
        let directory = archive
            .windows(4)
            .position(|w| w == CENTRAL_SIGNATURE)
            .unwrap();
        archive[directory + 24..directory + 28].copy_from_slice(&4u32.to_le_bytes());
        let err = first_file(&archive, 1024).unwrap_err();
        assert!(matches!(err, ArchiveError::TooLarge(1024)), "got: {err:?}");
    }

    #[test]
    fn offsets_outside_the_archive_are_refused() {
        let archive = zip_with_extra("camera.xml", b"<a/>", BASLER_EXTRA);
        let directory = archive
            .windows(4)
            .position(|w| w == CENTRAL_SIGNATURE)
            .unwrap();

        // The local header's offset points past the end.
        let mut bad = archive.clone();
        bad[directory + 42..directory + 46].copy_from_slice(&0x7FFF_FFFFu32.to_le_bytes());
        let err = first_file(&bad, LIMIT).unwrap_err();
        assert!(matches!(err, ArchiveError::OutOfBounds(_)), "got: {err:?}");

        // The compressed size runs past the end.
        let mut bad = archive.clone();
        bad[directory + 20..directory + 24].copy_from_slice(&0x7FFF_FFFFu32.to_le_bytes());
        let err = first_file(&bad, LIMIT).unwrap_err();
        assert!(matches!(err, ArchiveError::OutOfBounds(_)), "got: {err:?}");

        // The local header's extra field runs past the end.
        let mut bad = archive.clone();
        bad[28..30].copy_from_slice(&u16::MAX.to_le_bytes());
        let err = first_file(&bad, LIMIT).unwrap_err();
        assert!(matches!(err, ArchiveError::OutOfBounds(_)), "got: {err:?}");
    }

    #[test]
    fn no_prefix_of_an_archive_panics() {
        let archive = zip_with_extra("camera.xml", b"<a/>", BASLER_EXTRA);
        for len in 0..archive.len() {
            assert!(first_file(&archive[..len], LIMIT).is_err(), "prefix {len}");
        }
    }

    #[test]
    fn an_empty_archive_is_refused() {
        let mut archive = EOCD_SIGNATURE.to_vec();
        archive.resize(EOCD_LEN, 0);
        let err = first_file(&archive, LIMIT).unwrap_err();
        assert!(matches!(err, ArchiveError::Empty), "got: {err:?}");
    }
}
