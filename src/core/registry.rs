//! 注册表：把 `供应商id/模型名` 路由到具体供应商。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::provider::{ModelInfo, Provider};

/// `/v1/models` 结果的缓存时长。
///
/// 每次请求都真打六家上游既慢又浪费配额，而模型目录变化极少，
/// 故在进程内缓存一分钟（`ofm-server status` 走的是独立路径，不受影响）。
const MODELS_TTL: Duration = Duration::from_secs(60);

/// 全量模型目录快照（按供应商 id 顺序）。
type ModelSnapshot = Vec<(&'static str, Vec<ModelInfo>)>;

pub struct Registry {
    providers: BTreeMap<&'static str, Arc<dyn Provider>>,
    /// 最近一次模型目录快照：`(抓取时刻, 结果)`。
    models_cache: Mutex<Option<(Instant, Arc<ModelSnapshot>)>>,
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
            models_cache: Mutex::new(None),
        }
    }

    pub fn register(&mut self, provider: Arc<dyn Provider>) {
        self.providers.insert(provider.id(), provider);
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn Provider>> {
        self.providers.get(id).cloned()
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
    ///
    /// 各家并发拉取（串行时总耗时等于六家之和），结果按 [`MODELS_TTL`] 缓存。
    pub async fn list_models(&self) -> ModelSnapshot {
        if let Ok(guard) = self.models_cache.lock()
            && let Some((at, cached)) = guard.as_ref()
            && at.elapsed() < MODELS_TTL
        {
            return cached.as_ref().clone();
        }

        let out: ModelSnapshot = futures_util::future::join_all(
            self.providers
                .values()
                .map(|p| async move { (p.id(), p.models().await) }),
        )
        .await;

        if let Ok(mut guard) = self.models_cache.lock() {
            *guard = Some((Instant::now(), Arc::new(out.clone())));
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
