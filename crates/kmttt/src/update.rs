//! `kmttt update`: GitHub 릴리스에서 새 버전을 받아 minisign 서명을 검증한 뒤에만 교체한다.
//!
//! 공개키는 저장소의 `assets/release/minisign.pub`을 빌드할 때 넣는다(공개키라 비밀이 아니다).
//! 빌드할 때 `KMTTT_MINISIGN_PUBKEY`를 주면 그 키를 대신 쓴다(포크해서 직접 서명할 때).

use anyhow::{bail, Context, Result};
use serde_json::Value;

const REPO: &str = "securidge/securidge-kmttt";
const EMBEDDED_PUB: &str = include_str!("../../../assets/release/minisign.pub");

/// 업데이트 서명 검증용 공개키(base64 한 줄).
fn pubkey() -> Option<&'static str> {
    option_env!("KMTTT_MINISIGN_PUBKEY")
        .filter(|k| !k.trim().is_empty())
        .or_else(|| EMBEDDED_PUB.lines().nth(1))
        .map(str::trim)
        .filter(|k| !k.is_empty())
}

/// 내려받은 파일이 공개키로 서명됐는지 확인한다.
fn verify_signature(pubkey: &str, data: &[u8], sig_text: &str) -> Result<()> {
    let pk = minisign_verify::PublicKey::from_base64(pubkey)
        .map_err(|e| anyhow::anyhow!("bad embedded public key: {e}"))?;
    let signature = minisign_verify::Signature::decode(sig_text)
        .map_err(|e| anyhow::anyhow!("bad signature file: {e}"))?;
    pk.verify(data, &signature, false)
        .map_err(|_| anyhow::anyhow!("SIGNATURE CHECK FAILED — the download was not installed"))
}

/// `sha256sum` 출력 형식(`<해시>  <파일>` 또는 `<해시> *<파일>`)에서 한 파일의 해시를 찾는다.
fn expected_hash(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let (hash, file) = l.trim().split_once(char::is_whitespace)?;
        let file = file.trim_start().trim_start_matches('*');
        (file == name && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn asset_name() -> Option<&'static str> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        Some("kmttt-windows-x86_64.exe")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("kmttt-linux-x86_64")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("kmttt-linux-aarch64")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("kmttt-macos-aarch64")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("kmttt-macos-x86_64")
    } else {
        None
    }
}

/// OS 인증서 저장소를 믿는 HTTP 클라이언트. 회사망의 TLS 검사 프록시 인증서처럼
/// OS에 설치된 루트도 쓰려고 내장 목록(webpki-roots) 대신 플랫폼 검증기를 쓴다.
/// 프록시는 HTTPS_PROXY 등 환경 변수를 따른다(ureq 기본값).
fn agent() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig};
    ureq::Agent::config_builder()
        .tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .new_agent()
}

fn get(url: &str) -> Result<Vec<u8>> {
    let mut res = agent()
        .get(url)
        .header("User-Agent", "kmttt-update")
        .call()
        .with_context(|| format!("GET {url}"))?;
    let body = res
        .body_mut()
        .with_config()
        .limit(256 * 1024 * 1024)
        .read_to_vec()?;
    Ok(body)
}

fn newer(latest: &str, current: &str) -> bool {
    version_key(latest) > version_key(current)
}

/// SemVer 순서로 비교할 키. `1.0.0-beta.1` < `1.0.0-beta.2` < `1.0.0` < `1.0.1`.
/// 정식 버전(사전 배포 표시 없음)은 같은 숫자의 사전 배포보다 뒤에 온다.
fn version_key(s: &str) -> (Vec<u64>, bool, Vec<(u64, String)>) {
    let s = s.trim().trim_start_matches('v');
    let s = s.split('+').next().unwrap_or_default();
    let (core, pre) = match s.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (s, None),
    };
    let mut nums: Vec<u64> = core.split('.').map(|x| x.parse().unwrap_or(0)).collect();
    while nums.len() < 3 {
        nums.push(0);
    }
    let pre_ids = pre
        .map(|p| {
            p.split('.')
                .map(|id| match id.parse::<u64>() {
                    Ok(n) => (n, String::new()),
                    // 숫자가 아닌 식별자는 숫자 식별자보다 뒤에 온다
                    Err(_) => (u64::MAX, id.to_string()),
                })
                .collect()
        })
        .unwrap_or_default();
    (nums, pre.is_none(), pre_ids)
}

pub async fn run(check_only: bool) -> Result<i32> {
    tokio::task::spawn_blocking(move || run_blocking(check_only)).await?
}

fn run_blocking(check_only: bool) -> Result<i32> {
    let current = env!("CARGO_PKG_VERSION");
    let rel: Value = serde_json::from_slice(&get(&format!(
        "https://api.github.com/repos/{REPO}/releases/latest"
    ))?)?;
    let tag = rel["tag_name"].as_str().unwrap_or_default().to_string();
    println!("current {current}, latest {tag}");
    if !newer(&tag, current) {
        println!("already up to date");
        return Ok(0);
    }
    if check_only {
        return Ok(0);
    }
    let Some(pubkey) = pubkey() else {
        bail!("this build has no update signing key; download {tag} manually from https://github.com/{REPO}/releases and check it with SHA256SUMS.txt and SHA256SUMS.txt.minisig");
    };
    let name = asset_name().ok_or_else(|| anyhow::anyhow!("no release asset for this platform"))?;
    let find = |n: &str| -> Option<String> {
        rel["assets"]
            .as_array()?
            .iter()
            .find(|a| a["name"] == n)
            .and_then(|a| a["browser_download_url"].as_str())
            .map(String::from)
    };
    // 릴리스에는 파일마다 서명을 두지 않고, 모든 파일의 SHA-256 목록(SHA256SUMS.txt) 하나에 서명한다.
    // 목록의 서명을 확인한 뒤, 받은 실행 파일의 해시가 목록과 같을 때만 바꾼다.
    let bin_url = find(name).ok_or_else(|| anyhow::anyhow!("release has no {name}"))?;
    let sums_url = find("SHA256SUMS.txt").ok_or_else(|| {
        anyhow::anyhow!("release has no SHA256SUMS.txt — refusing to install an unverified binary")
    })?;
    let sig_url = find("SHA256SUMS.txt.minisig").ok_or_else(|| {
        anyhow::anyhow!(
            "release has no SHA256SUMS.txt.minisig — refusing to install an unsigned binary"
        )
    })?;
    let sums = get(&sums_url)?;
    let sig = String::from_utf8(get(&sig_url)?)?;
    verify_signature(pubkey, &sums, &sig)?;
    let want = expected_hash(&String::from_utf8_lossy(&sums), name)
        .ok_or_else(|| anyhow::anyhow!("SHA256SUMS.txt has no line for {name}"))?;
    let bin = get(&bin_url)?;
    if sha256_hex(&bin) != want {
        bail!("{name} does not match the signed SHA256SUMS.txt — refusing to install");
    }
    let exe = std::env::current_exe()?;
    let new = exe.with_extension("new");
    std::fs::write(&new, &bin)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755))?;
    }
    // Windows는 실행 중인 파일을 덮을 수 없으니 이름을 바꿔 비켜 둔다.
    let old = exe.with_extension("old");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old)?;
    std::fs::rename(&new, &exe)?;
    println!("updated to {tag} (signature verified). Restart the server: kmttt server stop");
    Ok(0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn embedded_key_verifies_release_signatures() {
        let key = super::pubkey().expect("embedded public key");
        let data = include_bytes!("../../../assets/release/selftest.txt");
        let sig = include_str!("../../../assets/release/selftest.txt.minisig");
        super::verify_signature(key, data, sig).expect("valid signature");
        let mut tampered = data.to_vec();
        tampered[0] ^= 1;
        assert!(super::verify_signature(key, &tampered, sig).is_err());
    }

    #[test]
    fn checksum_list_lookup() {
        let h = super::sha256_hex(b"abc");
        assert_eq!(
            h,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let sums = format!("{h}  kmttt-linux-x86_64\n{}  kmttt-desktop-linux-x86_64.deb\n{} *kmttt-windows-x86_64.exe\n", "0".repeat(64), "A".repeat(64));
        assert_eq!(
            super::expected_hash(&sums, "kmttt-linux-x86_64").as_deref(),
            Some(h.as_str())
        );
        assert_eq!(
            super::expected_hash(&sums, "kmttt-windows-x86_64.exe"),
            Some("a".repeat(64))
        );
        assert_eq!(
            super::expected_hash(&sums, "kmttt-linux"),
            None,
            "이름이 정확히 같아야 한다"
        );
        assert_eq!(
            super::expected_hash("xyz  kmttt-linux-x86_64", "kmttt-linux-x86_64"),
            None
        );
    }

    #[test]
    fn version_compare() {
        assert!(super::newer("v0.2.0", "0.1.9"));
        assert!(!super::newer("v0.1.0", "0.1.0"));
        assert!(super::newer("v1.0.0", "1.0.0-beta.1"));
        assert!(super::newer("v1.0.0-beta.2", "1.0.0-beta.1"));
        assert!(super::newer("v1.0.0-rc.1", "1.0.0-beta.9"));
        assert!(!super::newer("v1.0.0-beta.1", "1.0.0"));
        assert!(super::newer("v1.0.1", "1.0.0"));
        assert!(super::newer("1.0.0", "0.9.9"));
    }
}
