//! 内置供应商注册。
//!
//! 新增供应商的接入点：
//! 1. 实现 [`crate::core::provider::Provider`]（对话/模型/签到等），
//!    流式响应建议直接用 [`crate::core::stream::drive`] 驱动，无需自己写泵。
//! 2. 在 [`build_registry`] 里加一行注册。
//! 3. 在 [`crate::cli::login`] 里加一个登录分支（各家登录流程差异大，无法泛化）；
//!    用 [`crate::login_util`] 的 `CallbackServer`/`open_browser` 做本地回调。
//!
//! 其余命令（`claim`/`refresh`/`status`/`serve`）只依赖 [`crate::core::registry::Registry`]，
//! **无需改动**。

use std::path::Path;
use std::sync::Arc;

pub mod codearts;
pub mod loomy;
pub mod minimax;
pub mod raccoon;
pub mod trae;

use crate::config::Config;
use crate::core::provider::Provider;
use crate::core::registry::Registry;

use codearts::CodeartsProvider;
use loomy::LoomyProvider;
use minimax::MinimaxProvider;
use raccoon::RaccoonProvider;
use trae::TraeProvider;

/// 按配置构造全部供应商。
///
/// 每家只拿到**自己的**配置片段：档位集合不同，不能传共同类型。
pub fn build_registry(root: &Path, config: &Config) -> Registry {
    let mut registry = Registry::new();
    registry.register(Arc::new(TraeProvider::new(
        root.to_path_buf(),
        config.trae.reasoning_effort,
    )) as Arc<dyn Provider>);
    registry.register(Arc::new(RaccoonProvider::new(
        root.to_path_buf(),
        config.raccoon.reasoning_effort,
    )) as Arc<dyn Provider>);
    registry.register(Arc::new(CodeartsProvider::new(
        root.to_path_buf(),
        config.codearts.reasoning_effort,
    )) as Arc<dyn Provider>);
    registry.register(Arc::new(MinimaxProvider::new(
        root.to_path_buf(),
        config.minimax.reasoning_effort,
    )) as Arc<dyn Provider>);
    registry.register(Arc::new(LoomyProvider::new(
        root.to_path_buf(),
        config.loomy.reasoning_effort,
    )) as Arc<dyn Provider>);
    registry
}
