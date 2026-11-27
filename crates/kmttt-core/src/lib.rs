//! 서버·클라이언트가 함께 쓰는 모델: 설정, 경로, 레이아웃 계산, 필터 인터페이스, 다국어 문구.

pub mod client;
pub mod config;
pub mod filter;
pub mod i18n;
pub mod keys;
pub mod layout;
pub mod machines;
pub mod paths;
pub mod spawn;
pub mod tokens;
pub mod usage;

pub use config::Config;
pub use filter::{CoreEvent, Filter, FilterChain};
pub use layout::Rect;

/// 제품 이름과 제작자 표시.
pub const PRODUCT: &str = "Securidge KMttt";
pub const AUTHOR: &str = "Kwangmo Kim";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
