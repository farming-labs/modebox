//! Whole-VM snapshots and sealed continuation tokens.
//!
//! A snapshot is the guest's linear memory, stored as the 4 KiB chunks that
//! differ from the pre-initialized base image. Because the guest only ever
//! yields with an empty wasm stack, memory is the *entire* VM state: the heap,
//! every pending promise, closures, timers. Restoring it resumes execution
//! exactly where it stopped. Nothing is replayed, so side effects never repeat
//! and resume cost doesn't grow with history.
//!
//! Tokens are zstd-compressed and sealed with XChaCha20-Poly1305: clients can
//! neither read nor forge them.

use base64::Engine as _;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Serialize, de::DeserializeOwned};
#[cfg(test)]
use serde::Deserialize;

pub(crate) const CHUNK: usize = 4096;
const TOKEN_PREFIX: &str = "mb1.";

/// Memory chunks that differ from the base image, stored XOR the base: bytes
/// that didn't change become zero, which zstd compresses to almost nothing.
pub(crate) struct MemDiff {
    pub size: usize,
    pub chunks: Vec<u32>,
    pub data: Vec<u8>,
}

impl MemDiff {
    pub fn capture(mem: &[u8], base: &[u8]) -> MemDiff {
        let mut chunks = Vec::new();
        let mut data = Vec::new();
        for (i, chunk) in mem.chunks(CHUNK).enumerate() {
            let start = i * CHUNK;
            if !same_as_base(chunk, base, start) {
                chunks.push(i as u32);
                for (k, &b) in chunk.iter().enumerate() {
                    data.push(b ^ base.get(start + k).copied().unwrap_or(0));
                }
                // Pad a trailing partial chunk so every chunk is CHUNK bytes.
                data.resize(chunks.len() * CHUNK, 0);
            }
        }
        if std::env::var_os("MODEBOX_DEBUG_SNAPSHOT").is_some() {
            let mb = |i: &u32| (*i as usize * CHUNK) >> 20;
            let mut hist = std::collections::BTreeMap::new();
            for i in &chunks {
                *hist.entry(mb(i)).or_insert(0usize) += 1;
            }
            let zeroish = chunks.iter().enumerate().filter(|(n, _)| data[n * CHUNK..(n + 1) * CHUNK].iter().filter(|&&b| b == 0).count() > CHUNK * 3 / 4).count();
            eprintln!("[snapshot] mem {} KiB, base {} KiB, {} changed chunks ({} mostly-zero), by MiB: {hist:?}", mem.len() >> 10, base.len() >> 10, chunks.len(), zeroish);
        }
        MemDiff { size: mem.len(), chunks, data }
    }

    /// Apply onto memory that currently holds the base image.
    pub fn apply(&self, mem: &mut [u8]) {
        for (n, &i) in self.chunks.iter().enumerate() {
            let start = i as usize * CHUNK;
            let end = (start + CHUNK).min(mem.len());
            for (dst, x) in mem[start..end].iter_mut().zip(&self.data[n * CHUNK..]) {
                *dst ^= x;
            }
        }
    }
}

fn same_as_base(chunk: &[u8], base: &[u8], start: usize) -> bool {
    let overlap = base.len().saturating_sub(start).min(chunk.len());
    let base_part = if overlap > 0 { &base[start..start + overlap] } else { &[][..] };
    chunk[..overlap] == *base_part && chunk[overlap..].iter().all(|&b| b == 0)
}

/// Pack a header plus memory diff into one compressed blob.
pub(crate) fn pack<H: Serialize>(header: &H, diff: &MemDiff) -> Vec<u8> {
    let header = serde_json::to_vec(header).expect("header serializes");
    let mut body = Vec::with_capacity(16 + header.len() + diff.chunks.len() * 4 + diff.data.len());
    body.extend_from_slice(&(header.len() as u32).to_le_bytes());
    body.extend_from_slice(&header);
    body.extend_from_slice(&(diff.size as u64).to_le_bytes());
    body.extend_from_slice(&(diff.chunks.len() as u32).to_le_bytes());
    for i in &diff.chunks {
        body.extend_from_slice(&i.to_le_bytes());
    }
    body.extend_from_slice(&diff.data);
    zstd::bulk::compress(&body, 9).expect("zstd")
}

pub(crate) fn unpack<H: DeserializeOwned>(blob: &[u8]) -> Result<(H, MemDiff), String> {
    let body = zstd::stream::decode_all(blob).map_err(|e| format!("corrupt snapshot: {e}"))?;
    let mut r = Reader { b: &body, at: 0 };
    let hlen = r.u32()? as usize;
    let header: H = serde_json::from_slice(r.take(hlen)?).map_err(|e| format!("corrupt snapshot header: {e}"))?;
    let size = r.u64()? as usize;
    let n = r.u32()? as usize;
    let mut chunks = Vec::with_capacity(n);
    for _ in 0..n {
        chunks.push(r.u32()?);
    }
    let data = r.take(n * CHUNK)?.to_vec();
    if chunks.iter().any(|&i| i as usize * CHUNK >= size) {
        return Err("corrupt snapshot: chunk out of range".into());
    }
    Ok((header, MemDiff { size, chunks, data }))
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let s = self.b.get(self.at..self.at + n).ok_or("corrupt snapshot: truncated")?;
        self.at += n;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
}

/// Authenticated encryption for tokens that leave the process.
pub(crate) struct Sealer {
    cipher: XChaCha20Poly1305,
}

impl Sealer {
    pub fn new(secret: &[u8]) -> Self {
        let key = blake3::derive_key("modebox 2026 continuation sealing key v1", secret);
        Sealer { cipher: XChaCha20Poly1305::new((&key).into()) }
    }

    pub fn seal(&self, plaintext: &[u8]) -> String {
        let nonce_bytes: [u8; 24] = rand::random();
        let nonce = XNonce::from_slice(&nonce_bytes);
        let ct = self.cipher.encrypt(nonce, plaintext).expect("encrypt");
        let mut out = Vec::with_capacity(24 + ct.len());
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ct);
        format!("{TOKEN_PREFIX}{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(out))
    }

    pub fn open(&self, token: &str) -> Result<Vec<u8>, String> {
        let body = token.strip_prefix(TOKEN_PREFIX).ok_or("not an modebox continuation token")?;
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(body.trim())
            .map_err(|_| "continuation token is not valid base64url")?;
        if raw.len() < 24 + 16 {
            return Err("continuation token is truncated".into());
        }
        let (nonce, ct) = raw.split_at(24);
        self.cipher
            .decrypt(XNonce::from_slice(nonce), ct)
            .map_err(|_| "continuation token failed authentication (wrong key or tampered)".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize, Deserialize)]
    struct Empty;

    #[test]
    fn diff_roundtrip() {
        let base = vec![7u8; CHUNK * 3 + 100];
        let mut mem = base.clone();
        mem.resize(CHUNK * 5, 0);
        mem[CHUNK + 5] = 1; // change inside base
        mem[CHUNK * 4 + 9] = 2; // change in grown region
        let diff = MemDiff::capture(&mem, &base);
        assert_eq!(diff.chunks, vec![1, 4]);

        let mut restored = base.clone();
        restored.resize(diff.size, 0);
        diff.apply(&mut restored);
        assert_eq!(restored, mem);

        let blob = pack(&Empty, &diff);
        let (_, back): (Empty, MemDiff) = unpack(&blob).unwrap();
        assert_eq!(back.chunks, diff.chunks);
        assert_eq!(back.data, diff.data);
    }

    #[test]
    fn seal_open_and_tamper() {
        let s = Sealer::new(b"0123456789abcdef0123456789abcdef");
        let token = s.seal(b"hello");
        assert_eq!(s.open(&token).unwrap(), b"hello");
        let mut bad = token.clone().into_bytes();
        let last = bad.len() - 3;
        bad[last] = if bad[last] == b'A' { b'B' } else { b'A' };
        assert!(s.open(std::str::from_utf8(&bad).unwrap()).is_err());
        assert!(Sealer::new(b"another key entirely, 32+ bytes!!").open(&token).is_err());
    }
}
