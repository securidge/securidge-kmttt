//! Windows 실행 파일에 아이콘과 버전 정보를 넣는다(다른 OS에서는 아무것도 하지 않는다).

fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon/kmttt.ico");
    println!("cargo:rerun-if-env-changed=KMTTT_MINISIGN_PUBKEY");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/icon/kmttt.ico")
        .set("ProductName", "Securidge KMttt")
        .set(
            "FileDescription",
            "Securidge KMttt - Trusted Terminal Tracker",
        )
        .set("CompanyName", "Kwangmo Kim (personal project)")
        .set("LegalCopyright", "Kwangmo Kim. MIT OR Apache-2.0")
        .set("OriginalFilename", "kmttt.exe");
    if let Err(e) = res.compile() {
        // 리소스 컴파일러가 없으면 아이콘 없이 빌드한다(빌드 자체는 막지 않는다).
        println!("cargo:warning=could not embed Windows icon: {e}");
    }
}
