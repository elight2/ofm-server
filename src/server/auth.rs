//! 下游 api key 校验：`Authorization: Bearer <key>`。
//!
//! 开关就是配置里的 `api_keys`：[`ApiKeys::from_config`] 空数组表示**不校验**
//! （默认行为，兼容本地自用的各类客户端）；非空时请求须带上其中某一个 key，
//! 否则返回 OpenAI 风格的 401。
//!
//! 只作用在需要它的出口接口（`/v1/chat/completions`、`/v1/models`）；
//! `/health` 永远不校验，便于探活。

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::server::AppState;
use crate::server::openai;

/// 一组可用 key。`None`（空列表）表示不校验。
#[derive(Debug, Clone, Default)]
pub struct ApiKeys {
    keys: Option<Vec<String>>,
}

impl ApiKeys {
    /// 从配置取 key 列表；空白项被丢弃（配置文件里写了 `""` 不该等于
    /// 「任何空 key 都能过」）。全部为空则视为不校验。
    pub fn from_config(keys: &[String]) -> Self {
        let keys: Vec<String> = keys
            .iter()
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
            .collect();
        Self {
            keys: (!keys.is_empty()).then_some(keys),
        }
    }

    /// 是否需要校验。
    pub fn enabled(&self) -> bool {
        self.keys.is_some()
    }

    /// 该请求是否被允许。
    pub fn allows(&self, req: &Request<Body>) -> bool {
        let Some(keys) = &self.keys else {
            return true; // 未配置 → 不校验
        };
        match bearer_token(req) {
            Some(token) => keys.iter().any(|k| k == token),
            None => false,
        }
    }
}

/// 从 `Authorization: Bearer <key>` 取 key（scheme 不区分大小写）。
fn bearer_token(req: &Request<Body>) -> Option<&str> {
    let value = req.headers().get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

/// axum 中间件：未通过校验时直接回 401。
pub async fn require_key(State(app): State<AppState>, req: Request<Body>, next: Next) -> Response {
    if app.api_keys.allows(&req) {
        return next.run(req).await;
    }
    let message = if req.headers().contains_key(header::AUTHORIZATION) {
        "api key 无效"
    } else {
        "缺少 api key（请在请求头带 `Authorization: Bearer <key>`）"
    };
    openai::error_response(
        StatusCode::UNAUTHORIZED,
        message,
        "invalid_request_error",
        Some("invalid_api_key".into()),
    )
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req_with(auth: Option<&str>) -> Request<Body> {
        let mut b = Request::builder().uri("/v1/models");
        if let Some(v) = auth {
            b = b.header(header::AUTHORIZATION, v);
        }
        b.body(Body::empty()).unwrap()
    }

    /// 空列表 = 不校验，任何请求（含无 key）都放行。
    #[test]
    fn 空列表不校验() {
        let keys = ApiKeys::from_config(&[]);
        assert!(!keys.enabled());
        assert!(keys.allows(&req_with(None)));
        assert!(keys.allows(&req_with(Some("Bearer whatever"))));
    }

    /// 只写空白项同样等于不校验。
    #[test]
    fn 空白项视为未配置() {
        let keys = ApiKeys::from_config(&["".into(), "   ".into()]);
        assert!(!keys.enabled());
        assert!(keys.allows(&req_with(None)));
    }

    /// 非空列表：命中任一 key 放行，其它一律拒绝。
    #[test]
    fn 命中才放行() {
        let keys = ApiKeys::from_config(&["k1".into(), " k2 ".into()]);
        assert!(keys.enabled());
        assert!(keys.allows(&req_with(Some("Bearer k1"))));
        // 配置里的首尾空白被剥掉，客户端带不带空白都可以匹配到去空白后的值
        assert!(keys.allows(&req_with(Some("Bearer k2"))));
        assert!(keys.allows(&req_with(Some("bearer k1")))); // scheme 不区分大小写
        assert!(!keys.allows(&req_with(Some("Bearer k3"))));
        assert!(!keys.allows(&req_with(Some("Bearer"))));
        assert!(!keys.allows(&req_with(Some("Basic k1"))));
        assert!(!keys.allows(&req_with(None)));
    }

    /// 空 key 不能匹配「无 key」。
    #[test]
    fn 空_token_不算命中() {
        let keys = ApiKeys::from_config(&["k1".into()]);
        assert!(!keys.allows(&req_with(Some("Bearer "))));
        assert!(!keys.allows(&req_with(Some("Bearer   "))));
    }
}
