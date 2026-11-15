//! 内置供应商注册。
//!
//! 新增供应商：实现 [`crate::core::provider::Provider`]，在 [`build_registry`] 里注册一行。

use std::path::Path;
use std::sync::Arc;

pub mod trae;

use crate::core::provider::Provider;
use crate::core::registry::Registry;

use trae::TraeProvider;

pub fn build_registry(root: &Path) -> Registry {
    let mut registry = Registry::new();
    registry.register(Arc::new(TraeProvider::new(root.to_path_buf())) as Arc<dyn Provider>);
    registry
}
