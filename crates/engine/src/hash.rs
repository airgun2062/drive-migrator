use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::{EngineError, Result};

/// Size of each sampled chunk read from the start and end of a file.
pub const SAMPLE_CHUNK_BYTES: u64 = 64 * 1024;

/// Fixed buffer size used to stream a whole file through BLAKE3. Files are
/// never loaded into memory in full.
const STREAM_BUFFER_BYTES: usize = 256 * 1024;

/// Hashes the first and last [`SAMPLE_CHUNK_BYTES`] of a file (or the whole
/// file if it is smaller than twice that).
///
/// This is a cheap pre-filter, not proof of identity: two files that share a
/// sample hash may still differ elsewhere, but two files with different
/// sample hashes are always different. Sampling may reject a candidate but
/// never confirms one (SPEC.md section 4).
pub fn sample_hash(path: &Path, size: u64) -> Result<blake3::Hash> {
    let mut file = open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; SAMPLE_CHUNK_BYTES.min(size.max(1)) as usize];

    let n = read_fill(&mut file, path, &mut buf)?;
    hasher.update(&buf[..n]);

    if size > SAMPLE_CHUNK_BYTES * 2 {
        file.seek(SeekFrom::End(-(SAMPLE_CHUNK_BYTES as i64)))
            .map_err(|source| EngineError::Seek {
                path: path.to_path_buf(),
                source,
            })?;
        let n = read_fill(&mut file, path, &mut buf)?;
        hasher.update(&buf[..n]);
    }

    Ok(hasher.finalize())
}

/// Streams the whole file through BLAKE3 with a fixed-size buffer.
pub fn full_hash(path: &Path) -> Result<blake3::Hash> {
    let mut file = open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; STREAM_BUFFER_BYTES];

    loop {
        let n = file.read(&mut buf).map_err(|source| EngineError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    Ok(hasher.finalize())
}

/// Streams the whole file through SHA-256 with a fixed-size buffer, returning
/// a lowercase hex digest. Used only for the manifest (SPEC.md section 4:
/// "BLAKE3 for analysis, SHA-256 for the manifest"); duplicate detection
/// elsewhere always uses BLAKE3.
pub fn full_sha256_hex(path: &Path) -> Result<String> {
    let mut file = open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; STREAM_BUFFER_BYTES];

    loop {
        let n = file.read(&mut buf).map_err(|source| EngineError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    Ok(encode_hex(&hasher.finalize()))
}

/// SHA-256 of an in-memory byte slice, as a lowercase hex digest.
pub fn sha256_hex(bytes: &[u8]) -> String {
    encode_hex(&Sha256::digest(bytes))
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn open(path: &Path) -> Result<File> {
    File::open(path).map_err(|source| EngineError::Open {
        path: path.to_path_buf(),
        source,
    })
}

/// Reads until `buf` is full or the file ends, returning the number of bytes
/// actually read.
fn read_fill(file: &mut File, path: &Path, buf: &mut [u8]) -> Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        let n = file
            .read(&mut buf[total..])
            .map_err(|source| EngineError::Read {
                path: path.to_path_buf(),
                source,
            })?;
        if n == 0 {
            break;
        }
        total += n;
    }
    Ok(total)
}
