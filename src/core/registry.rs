//! 注册表：把 `供应商id/模型名` 路由到具体供应商。

use std::collections::BTreeMap;
use std::sync::Arc;

use super::provider::{ModelInfo, Provider};

pub struct Registry {
    providers: BTreeMap<&'static str, Arc<dyn Provider>>,
}

/// 模型名拆分结果。
pub struct ModelRoute<'a> {
    pub provider: &'a str,
    pub model: &'a str,
}

impl Registry {
    pub fn new() -> Self {
        Self {
            providers: BTreeMap::new(),
        }
    }

    pub fn register(&mut self, provider: Arc<dyn Provider>) {
        self.providers.insert(provider.id(), provider);
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn Provider>> {
        self.providers.get(id).cloned()
    }

    pub fn ids(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.providers.keys().copied()
    }

    pub fn all(&self) -> impl Iterator<Item = &Arc<dyn Provider>> {
        self.providers.values()
    }

    /// 解析 `供应商id/模型名`。
    pub fn route(model: &str) -> Option<ModelRoute<'_>> {
        let (provider, rest) = model.split_once('/')?;
        if provider.is_empty() || rest.is_empty() {
            return None;
        }
        Some(ModelRoute {
            provider,
            model: rest,
        })
    }

    /// 聚合全部供应商的模型列表（单个供应商失败不影响整体）。
    pub async fn list_models(&self) -> Vec<(&'static str, Vec<ModelInfo>)> {
        let mut out = Vec::new();
        for provider in self.providers.values() {
            let models = provider.models().await;
            out.push((provider.id(), models));
        }
        out
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}
