//! 对话请求的最小发出间隔（每供应商一份）。
//!
//! 语义是**发出时刻间隔**：同一供应商两次请求的「发出时刻」至少相隔
//! `min_interval`。第 1 个请求发出后，第 2 个请求等够间隔就发出，之后两者
//! **并行跑完**——不等待前一个请求的流结束，因此不构成串行化。
//!
//! 不同供应商各用各的节流器，互不影响：trae 的间隔不会拖慢 codearts。
//! 间隔为 0 时 [`Throttle::acquire`] 直接返回，不加任何延迟。
//!
//! 调度方式：在互斥锁内**只做时隙预占**（算出该等到什么时候并立刻把
//! 「上一次发出时刻」推进到那个时隙），把等待放到锁外 `sleep`。这样并发
//! 请求会各自领到 `t0`、`t0+interval`、`t0+2*interval`……依次排开，既不会
//! 因为持锁等待而互相阻塞，也不会出现「多个请求同时醒来一起发出」。

use std::time::Duration;

use tokio::sync::Mutex;

/// 单个供应商的对话请求节流器。
#[derive(Debug)]
pub struct Throttle {
    /// 最小发出间隔；`ZERO` 表示不限（此时不加锁、不等待）。
    min_interval: Duration,
    /// 下一个可用时隙。`None` 表示还没发过请求；`Some(t)` 表示上一次请求
    /// 发出于 `t`（或更早的已排定时隙）。
    next_slot: Mutex<Option<std::time::Instant>>,
}

impl Throttle {
    /// 构造一个节流器。`min_interval` 为 0 表示不限制。
    pub fn new(min_interval: Duration) -> Self {
        Self {
            min_interval,
            next_slot: Mutex::new(None),
        }
    }

    /// 不做限制的节流器（间隔 0）。单测用。
    #[cfg(test)]
    pub fn unlimited() -> Self {
        Self::new(Duration::ZERO)
    }

    /// 间隔是否为 0（调用方可用于跳过日志）。
    pub fn is_unlimited(&self) -> bool {
        self.min_interval.is_zero()
    }

    /// 申请发出许可：必要时等待到下一个时隙再返回。
    ///
    /// 它不设并发上限——多个请求可以同时持有许可并各自跑完，`acquire` 只保证
    /// **发出时刻**彼此拉开。
    pub async fn acquire(&self) {
        if self.is_unlimited() {
            return;
        }

        // 关键：锁内只算「该等到什么时候」，锁外 sleep。
        // 若在锁内 sleep，并发请求会被串行阻塞（第 2 个请求要等第 1 个 sleep
        // 完才拿得到锁），那就退化成排队了。
        let wait = {
            let mut slot = self.next_slot.lock().await;
            let now = std::time::Instant::now();
            let start = match *slot {
                // 首次：本请求立刻发出，下一个时隙在 interval 之后。
                None => now,
                // 已有上一次：不得早于 next_slot，也不得早于 now（时钟已过则不等）。
                Some(prev) => {
                    let earliest = prev + self.min_interval;
                    if earliest > now { earliest } else { now }
                }
            };
            *slot = Some(start);
            start.saturating_duration_since(now)
        };

        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// 间隔为 0 时立刻返回，不产生任何延迟。
    #[tokio::test]
    async fn 不限速时不等待() {
        let t = Throttle::unlimited();
        assert!(t.is_unlimited());
        let start = Instant::now();
        for _ in 0..50 {
            t.acquire().await;
        }
        assert!(start.elapsed() < Duration::from_millis(100), "不该有延迟");
    }

    /// 连续的 acquire 必须按 interval 排开。
    #[tokio::test]
    async fn 连续请求按时隙排开() {
        let t = Throttle::new(Duration::from_millis(100));
        let start = Instant::now();
        t.acquire().await; // 第 1 个立刻发出
        let first = start.elapsed();
        assert!(first < Duration::from_millis(50), "首个请求不该等");

        t.acquire().await; // 第 2 个等到 100ms
        let second = start.elapsed();
        assert!(second >= Duration::from_millis(95), "第 2 个应等够间隔");

        t.acquire().await; // 第 3 个等到 200ms
        let third = start.elapsed();
        assert!(
            third >= Duration::from_millis(195),
            "第 3 个应排在第 3 个时隙"
        );
    }

    /// 空闲超出一个间隔后，上一次的时隙已过期，新请求不该再等。
    #[tokio::test]
    async fn 空闲后不累积延迟() {
        let t = Throttle::new(Duration::from_millis(50));
        t.acquire().await;
        tokio::time::sleep(Duration::from_millis(80)).await;
        let start = Instant::now();
        t.acquire().await;
        assert!(
            start.elapsed() < Duration::from_millis(30),
            "空闲已超过间隔，新请求应立刻发出"
        );
    }

    /// 并发请求不得同时发出：每个都被排到各自的时隙。
    ///
    /// 这条守住「锁内 sleep」这个易犯的错误——若在锁内 sleep，第 2 个请求要等
    /// 第 1 个睡完才拿锁，总耗时会翻倍且顺序仍对；更糟的是若误在锁外不推进
    /// 时隙，多个请求会同时醒来一起发出。
    #[tokio::test]
    async fn 并发请求各自排到不同时隙() {
        let t = std::sync::Arc::new(Throttle::new(Duration::from_millis(60)));
        let start = Instant::now();
        let mut handles = Vec::new();
        for _ in 0..3 {
            let t = t.clone();
            handles.push(tokio::spawn(async move {
                t.acquire().await;
                start.elapsed()
            }));
        }
        let mut times = Vec::new();
        for h in handles {
            times.push(h.await.expect("任务不应 panic"));
        }
        // 三个请求的发出时刻必须拉开，不能挤在同一时刻。
        assert!(
            times.iter().all(|d| *d < Duration::from_millis(500)),
            "不该串行阻塞：{times:?}"
        );
        // 最晚的至少排到第 3 个时隙。
        let latest = times.iter().max().copied().unwrap();
        assert!(
            latest >= Duration::from_millis(115),
            "应排到时隙上：{times:?}"
        );
    }
}
