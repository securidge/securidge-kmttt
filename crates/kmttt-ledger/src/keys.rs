//! 서명 키·암호화 키 보관.
//!
//! - `keys/sign.key`: Ed25519 비밀키(32바이트). Windows는 DPAPI로 감싸서 저장, 그 밖은 0600 파일.
//! - `keys/sign.pub`: 공개키(hex). 검증에 쓴다.
//! - `keys/enc.key`: XChaCha20-Poly1305 키(선택).

use anyhow::{bail, Context, Result};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use ed25519_dalek::SigningKey;
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::path::Path;

fn random32() -> [u8; 32] {
    let mut b = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut b);
    b
}

pub fn fingerprint(pubkey: &[u8]) -> String {
    hex::encode(&Sha256::digest(pubkey)[..8])
}

pub fn load_or_create_signing(dir: &Path) -> Result<SigningKey> {
    let kdir = dir.join("keys");
    std::fs::create_dir_all(&kdir)?;
    let kpath = kdir.join("sign.key");
    let ppath = kdir.join("sign.pub");
    if kpath.exists() {
        let wrapped = std::fs::read(&kpath).context("read sign.key")?;
        let raw = unprotect(&wrapped).context("unprotect sign.key")?;
        let arr: [u8; 32] = raw
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("sign.key has wrong length"))?;
        let key = SigningKey::from_bytes(&arr);
        if !ppath.exists() {
            std::fs::write(&ppath, hex::encode(key.verifying_key().to_bytes()))?;
        }
        return Ok(key);
    }
    let key = SigningKey::from_bytes(&random32());
    write_secret(&kpath, &protect(&key.to_bytes())?)?;
    std::fs::write(&ppath, hex::encode(key.verifying_key().to_bytes()))?;
    Ok(key)
}

pub fn load_or_create_enc(dir: &Path) -> Result<[u8; 32]> {
    let kdir = dir.join("keys");
    std::fs::create_dir_all(&kdir)?;
    let p = kdir.join("enc.key");
    if p.exists() {
        return load_enc(dir);
    }
    let k = random32();
    write_secret(&p, &protect(&k)?)?;
    Ok(k)
}

pub fn load_enc(dir: &Path) -> Result<[u8; 32]> {
    let wrapped = std::fs::read(dir.join("keys").join("enc.key")).context("read enc.key")?;
    let raw = unprotect(&wrapped)?;
    raw.as_slice()
        .try_into()
        .map_err(|_| anyhow::anyhow!("enc.key has wrong length"))
}

pub fn encrypt(key: &[u8; 32], plain: &[u8]) -> Result<String> {
    let cipher = XChaCha20Poly1305::new(&(*key).into());
    let mut nonce = [0u8; 24];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let ct = cipher
        .encrypt(&XNonce::from(nonce), plain)
        .map_err(|_| anyhow::anyhow!("encrypt failed"))?;
    let mut out = nonce.to_vec();
    out.extend(ct);
    Ok(kmttt_proto::b64_encode(&out))
}

pub fn decrypt(key: &[u8; 32], b64: &str) -> Result<Vec<u8>> {
    let raw = kmttt_proto::b64_decode(b64).map_err(|e| anyhow::anyhow!(e.message))?;
    if raw.len() < 24 + 16 {
        bail!("ciphertext too short");
    }
    let (nonce, ct) = raw.split_at(24);
    let nonce: [u8; 24] = nonce.try_into().expect("24 bytes");
    let cipher = XChaCha20Poly1305::new(&(*key).into());
    cipher
        .decrypt(&XNonce::from(nonce), ct)
        .map_err(|_| anyhow::anyhow!("decrypt failed (wrong key or tampered data)"))
}

fn write_secret(path: &Path, bytes: &[u8]) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(bytes)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)?;
        Ok(())
    }
}

/// Windows: DPAPI(현재 사용자 범위)로 감싼다. 다른 사용자·다른 PC에서는 풀 수 없다.
#[cfg(windows)]
fn protect(data: &[u8]) -> Result<Vec<u8>> {
    dpapi::protect(data)
}
#[cfg(windows)]
fn unprotect(data: &[u8]) -> Result<Vec<u8>> {
    dpapi::unprotect(data)
}

/// Unix: 파일 권한(0600)으로 보호한다. OS 키체인 연동은 DECISIONS.md 참고.
#[cfg(not(windows))]
fn protect(data: &[u8]) -> Result<Vec<u8>> {
    Ok(data.to_vec())
}
#[cfg(not(windows))]
fn unprotect(data: &[u8]) -> Result<Vec<u8>> {
    Ok(data.to_vec())
}

#[cfg(windows)]
mod dpapi {
    use anyhow::{bail, Result};
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB,
    };

    fn call(data: &[u8], protect: bool) -> Result<Vec<u8>> {
        let input = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        // SAFETY: input은 data를 빌려 가리키고 호출 동안 살아 있다. output은 성공 시
        // LocalAlloc으로 할당되며, 복사한 뒤 LocalFree로 돌려준다.
        let ok = unsafe {
            if protect {
                CryptProtectData(
                    &input,
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    &mut output,
                )
            } else {
                CryptUnprotectData(
                    &input,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    &mut output,
                )
            }
        };
        if ok == 0 {
            bail!("DPAPI call failed: {}", std::io::Error::last_os_error());
        }
        // SAFETY: 성공했으므로 output.pbData는 cbData 바이트를 가리킨다.
        let out =
            unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
        // SAFETY: DPAPI가 LocalAlloc으로 준 메모리다.
        unsafe {
            LocalFree(output.pbData as _);
        }
        Ok(out)
    }

    pub fn protect(data: &[u8]) -> Result<Vec<u8>> {
        call(data, true)
    }

    pub fn unprotect(data: &[u8]) -> Result<Vec<u8>> {
        call(data, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_round_trip_and_tamper() {
        let k = random32();
        let c = encrypt(&k, b"hello").unwrap();
        assert_eq!(decrypt(&k, &c).unwrap(), b"hello");
        let mut raw = kmttt_proto::b64_decode(&c).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 1;
        assert!(decrypt(&k, &kmttt_proto::b64_encode(&raw)).is_err());
        assert!(decrypt(&random32(), &c).is_err());
    }

    #[test]
    fn signing_key_persists() {
        let d = tempfile::tempdir().unwrap();
        let a = load_or_create_signing(d.path()).unwrap();
        let b = load_or_create_signing(d.path()).unwrap();
        assert_eq!(a.to_bytes(), b.to_bytes());
    }
}
