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

    /// 解析 `供应商id/模型名`，容忍 `/v1/models` 输出的 ` (x倍率)` 后缀。
    ///
    /// 客户端可能将 `id` 原样回传，故须先剔除外展示后缀（见 README）。
    pub fn route(model: &str) -> Option<ModelRoute<'_>> {
        let (provider, rest) = model.split_once('/')?;
        let rest = strip_rate_suffix(rest);
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

/// 剔除模型名尾部的 ` (x<倍率>)` 展示后缀。
///
/// 该后缀由 `/v1/models` 生成（见 README），客户端可能原样回传。
/// 只剥离最后一段且必须以 ` (x` 开头、`)` 结尾，避免误伤正常模型名。
fn strip_rate_suffix(model: &str) -> &str {
    let Some(start) = model.rfind(" (x") else {
        return model;
    };
    let suffix = &model[start + 3..];
    let inner = suffix.strip_suffix(')').unwrap_or("");
    if inner.is_empty() || inner.parse::<f64>().is_err() {
        return model;
    }
    model[..start].trim_end()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_容忍倍率后缀() {
        let r = Registry::route("trae/glm-5.2 (x0.78)").expect("应能解析");
        assert_eq!(r.provider, "trae");
        assert_eq!(r.model, "glm-5.2");
    }

    #[test]
    fn route_无后缀正常() {
        let r = Registry::route("trae/glm-5.2").unwrap();
        assert_eq!(r.model, "glm-5.2");
    }

    #[test]
    fn 不误伤含_x_的模型名() {
        let r = Registry::route("trae/glm-5.3-flashx").unwrap();
        assert_eq!(r.model, "glm-5.3-flashx");
        let r = Registry::route("trae/model (x1)").unwrap();
        assert_eq!(r.model, "model");
        // 括号内不是数字 → 不剥离
        let r = Registry::route("trae/model (xx)").unwrap();
        assert_eq!(r.model, "model (xx)");
    }
}
