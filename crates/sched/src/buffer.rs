//! 内核内存缓冲（契约 §4）：单任务隔离、容量上限、三种溢出策略、只读快照与回执结算。
//!
//! 计量的唯一口径：[`Buffer::estimate_bytes`] = `serde_json` 序列化字节 + 每点 32 B
//! 管理开销。写入时累加、消费时扣减，不做近似。
//!
//! 回执结算刻意做成「先校验、后变更」：水位越界时返回错误且缓冲一个字节都不动，
//! 上层据此把任务记为 protocol 失败，而不是误删数据。

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::error::TaskError;
use crate::exec::Verdict;

pub const POINT_MAX_BYTES: usize = 64 * 1024;
pub const POINT_OVERHEAD_BYTES: usize = 32;
pub const DEFAULT_CAPACITY_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_CAPACITY_BYTES: usize = 16 * 1024 * 1024;
pub const DEFAULT_MAX_BATCH_BYTES: usize = 512 * 1024;
pub const DEFAULT_AGGREGATE_WINDOW_SECS: u64 = 60;
pub const MIN_SAMPLE_INTERVAL_SECS: u64 = 1;
/// 80% 触发一次高水位审计；降到 80% 以下产生 recovered。
pub const HIGH_WATERMARK_RATIO: f64 = 0.8;
/// 同一任务的高水位审计最多每 10 分钟一次（带滞后）。
pub const HIGH_WATERMARK_AUDIT_INTERVAL_SECS: i64 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverflowPolicy {
    #[default]
    DropOldest,
    Aggregate,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregateRule {
    Sum,
    Last,
    Max,
    Min,
}

impl AggregateRule {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "sum" => Some(Self::Sum),
            "last" => Some(Self::Last),
            "max" => Some(Self::Max),
            "min" => Some(Self::Min),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sum => "sum",
            Self::Last => "last",
            Self::Max => "max",
            Self::Min => "min",
        }
    }
}

/// 缓冲配置（契约 §4.2）。`aggregate` 规则从任务 `payload.aggregate` 读取，不在这里。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BufferConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_method: Option<String>,
    #[serde(default = "default_sample_interval")]
    pub sample_interval_secs: u64,
    #[serde(default = "default_capacity")]
    pub capacity_bytes: usize,
    #[serde(default)]
    pub overflow: OverflowPolicy,
    #[serde(default = "default_retain")]
    pub retain_across_ticks: bool,
    #[serde(default = "default_aggregate_window")]
    pub aggregate_window_secs: u64,
    #[serde(default = "default_max_batch")]
    pub max_batch_bytes: usize,
}

fn default_sample_interval() -> u64 {
    30
}

fn default_capacity() -> usize {
    DEFAULT_CAPACITY_BYTES
}

fn default_retain() -> bool {
    true
}

fn default_aggregate_window() -> u64 {
    DEFAULT_AGGREGATE_WINDOW_SECS
}

fn default_max_batch() -> usize {
    DEFAULT_MAX_BATCH_BYTES
}

impl Default for BufferConfig {
    fn default() -> Self {
        Self {
            sample_method: None,
            sample_interval_secs: default_sample_interval(),
            capacity_bytes: default_capacity(),
            overflow: OverflowPolicy::DropOldest,
            retain_across_ticks: true,
            aggregate_window_secs: default_aggregate_window(),
            max_batch_bytes: default_max_batch(),
        }
    }
}

impl BufferConfig {
    pub fn validate(&self) -> Result<(), TaskError> {
        if self.capacity_bytes == 0 || self.capacity_bytes > MAX_CAPACITY_BYTES {
            return Err(TaskError::InvalidBuffer(format!(
                "capacity_bytes 必须在 1–{MAX_CAPACITY_BYTES}：{}",
                self.capacity_bytes
            )));
        }
        if self.max_batch_bytes == 0 {
            return Err(TaskError::InvalidBuffer(
                "max_batch_bytes 必须大于 0".to_string(),
            ));
        }
        if self.sample_interval_secs < MIN_SAMPLE_INTERVAL_SECS {
            return Err(TaskError::InvalidBuffer(format!(
                "sample_interval_secs 最小 {MIN_SAMPLE_INTERVAL_SECS}s"
            )));
        }
        if self.aggregate_window_secs == 0 {
            return Err(TaskError::InvalidBuffer(
                "aggregate_window_secs 必须大于 0".to_string(),
            ));
        }
        if let Some(method) = &self.sample_method {
            let ok = !method.is_empty()
                && method.len() <= 64
                && method
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
            if !ok {
                return Err(TaskError::InvalidBuffer(format!(
                    "sample_method 非法：{method}"
                )));
            }
        }
        Ok(())
    }
}

/// 批次里交给执行体的一条采样点。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SamplePoint {
    pub seq: u64,
    pub at: i64,
    pub data: Value,
}

#[derive(Debug, Clone)]
struct StoredPoint {
    seq: u64,
    at: i64,
    bytes: usize,
    data: Value,
}

#[derive(Debug, Clone, Default)]
struct BufferTotals {
    dropped_points: u64,
    dropped_bytes: u64,
    rejected_points: u64,
    rejected_bytes: u64,
    aggregated_groups: u64,
    consumed_points: u64,
    consumed_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BufferSignal {
    HighWatermark,
    /// 发生过丢弃、聚合或拒收。
    Overflow,
    Recovered,
    PointTooLarge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    PointTooLarge,
    Full,
    GlobalDegraded,
    PluginQuota,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushOutcome {
    Accepted {
        seq: u64,
        bytes: usize,
        dropped_points: u64,
        dropped_bytes: u64,
        aggregated: bool,
    },
    Rejected {
        reason: RejectReason,
        bytes: usize,
    },
}

impl PushOutcome {
    pub fn is_accepted(&self) -> bool {
        matches!(self, PushOutcome::Accepted { .. })
    }
}

#[derive(Debug, Clone)]
pub struct PushReport {
    pub outcome: PushOutcome,
    pub signals: Vec<BufferSignal>,
    /// 缓冲净变化字节数（接受时为正/负，拒收时为 0），供全局计量使用。
    pub net_bytes: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotMeta {
    /// 快照 id：结算时必须带回来，防止同一批次被重复消费。
    pub id: u64,
    pub head_seq: Option<u64>,
    pub tail_seq: Option<u64>,
    /// 快照时刻缓冲区的最大序号；`retain_across_ticks=false` 用它清空未取走点。
    pub buffer_tail_seq: Option<u64>,
    pub truncated: bool,
}

impl SnapshotMeta {
    pub fn empty() -> Self {
        Self {
            id: 0,
            head_seq: None,
            tail_seq: None,
            buffer_tail_seq: None,
            truncated: false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.head_seq.is_none()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub meta: SnapshotMeta,
    pub points: Vec<SamplePoint>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitOutcome {
    pub removed_points: usize,
    pub removed_bytes: usize,
    pub remaining_points: usize,
    pub remaining_bytes: usize,
    pub signals: Vec<BufferSignal>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BufferError {
    #[error("快照已失效或已结算：{0}")]
    StaleSnapshot(u64),
    #[error("水位越界：{upto} 不在批次 [{head}, {tail}] 内，或不是批次中的真实序号")]
    WatermarkOutOfRange { upto: u64, head: u64, tail: u64 },
    #[error("批次为空，persist/partial 不能声明水位")]
    EmptyBatchWatermark,
}

/// 只读统计，给页面与指标用。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BufferStat {
    pub points: usize,
    pub bytes: usize,
    pub capacity_bytes: usize,
    /// 下一个采样点将使用的序号（即当前水位）。
    pub seq: u64,
    pub head_seq: Option<u64>,
    pub tail_seq: Option<u64>,
    pub dropped_points: u64,
    pub dropped_bytes: u64,
    pub rejected_points: u64,
    pub rejected_bytes: u64,
    pub aggregated_groups: u64,
    pub consumed_points: u64,
    pub consumed_bytes: u64,
    pub high_watermark: bool,
}

pub struct Buffer {
    cfg: BufferConfig,
    aggregate_rule: Option<AggregateRule>,
    points: VecDeque<StoredPoint>,
    bytes: usize,
    next_seq: u64,
    next_snapshot: u64,
    pending: Option<SnapshotMeta>,
    totals: BufferTotals,
    high_latched: bool,
    last_high_audit_at: i64,
}

impl Buffer {
    pub fn new(cfg: BufferConfig, aggregate_rule: Option<AggregateRule>) -> Self {
        Self {
            cfg,
            aggregate_rule,
            points: VecDeque::new(),
            bytes: 0,
            next_seq: 0,
            next_snapshot: 0,
            pending: None,
            totals: BufferTotals::default(),
            high_latched: false,
            last_high_audit_at: i64::MIN,
        }
    }

    /// 单点体积口径：序列化字节 + 32 B 管理开销。
    pub fn estimate_bytes(data: &Value) -> usize {
        serde_json::to_vec(data)
            .map(|v| v.len() + POINT_OVERHEAD_BYTES)
            .unwrap_or(POINT_MAX_BYTES + 1)
    }

    pub fn config(&self) -> &BufferConfig {
        &self.cfg
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn stat(&self) -> BufferStat {
        BufferStat {
            points: self.points.len(),
            bytes: self.bytes,
            capacity_bytes: self.cfg.capacity_bytes,
            seq: self.next_seq,
            head_seq: self.points.front().map(|p| p.seq),
            tail_seq: self.points.back().map(|p| p.seq),
            dropped_points: self.totals.dropped_points,
            dropped_bytes: self.totals.dropped_bytes,
            rejected_points: self.totals.rejected_points,
            rejected_bytes: self.totals.rejected_bytes,
            aggregated_groups: self.totals.aggregated_groups,
            consumed_points: self.totals.consumed_points,
            consumed_bytes: self.totals.consumed_bytes,
            high_watermark: self.high_latched,
        }
    }

    /// 写入一个采样点；溢出策略按配置执行。
    pub fn push(&mut self, at: i64, data: Value, now: i64) -> PushReport {
        let bytes = Self::estimate_bytes(&data);
        if bytes > POINT_MAX_BYTES {
            self.totals.rejected_points += 1;
            self.totals.rejected_bytes += bytes as u64;
            return PushReport {
                outcome: PushOutcome::Rejected {
                    reason: RejectReason::PointTooLarge,
                    bytes,
                },
                signals: vec![BufferSignal::PointTooLarge],
                net_bytes: 0,
            };
        }
        // 单点比整个任务容量还大：无论如何都放不下，按 reject 处理，避免空转。
        if bytes > self.cfg.capacity_bytes {
            self.totals.rejected_points += 1;
            self.totals.rejected_bytes += bytes as u64;
            return PushReport {
                outcome: PushOutcome::Rejected {
                    reason: RejectReason::Full,
                    bytes,
                },
                signals: vec![BufferSignal::Overflow],
                net_bytes: 0,
            };
        }

        let mut dropped_points = 0u64;
        let mut dropped_bytes = 0u64;
        let mut aggregated = false;

        while self.bytes + bytes > self.cfg.capacity_bytes {
            match self.cfg.overflow {
                OverflowPolicy::Reject => {
                    self.totals.rejected_points += 1;
                    self.totals.rejected_bytes += bytes as u64;
                    return PushReport {
                        outcome: PushOutcome::Rejected {
                            reason: RejectReason::Full,
                            bytes,
                        },
                        signals: vec![BufferSignal::Overflow],
                        net_bytes: 0,
                    };
                }
                OverflowPolicy::DropOldest => {
                    if let Some(removed) = self.drop_one() {
                        dropped_points += 1;
                        dropped_bytes += removed as u64;
                    } else {
                        break;
                    }
                }
                OverflowPolicy::Aggregate => {
                    if self.aggregate_rule.is_some() && self.aggregate_front() {
                        aggregated = true;
                    } else if let Some(removed) = self.drop_one() {
                        dropped_points += 1;
                        dropped_bytes += removed as u64;
                    } else {
                        break;
                    }
                }
            }
        }

        let seq = self.next_seq;
        self.next_seq += 1;
        self.bytes += bytes;
        self.points.push_back(StoredPoint {
            seq,
            at,
            bytes,
            data,
        });

        let mut signals = Vec::new();
        if dropped_points > 0 || aggregated {
            signals.push(BufferSignal::Overflow);
        }
        signals.extend(self.evaluate_watermarks(now));

        PushReport {
            outcome: PushOutcome::Accepted {
                seq,
                bytes,
                dropped_points,
                dropped_bytes,
                aggregated,
            },
            signals,
            net_bytes: bytes as i64 - dropped_bytes as i64,
        }
    }

    fn drop_one(&mut self) -> Option<usize> {
        let point = self.points.pop_front()?;
        self.bytes -= point.bytes;
        self.totals.dropped_points += 1;
        self.totals.dropped_bytes += point.bytes as u64;
        Some(point.bytes)
    }

    /// 把最旧窗口内的点合成一个点；窗口内不足两点时返回 false（调用方降级为丢最旧）。
    fn aggregate_front(&mut self) -> bool {
        let Some(rule) = self.aggregate_rule else {
            return false;
        };
        let window_secs = self.cfg.aggregate_window_secs.max(1) as i64;
        let Some(window) = self.points.front().map(|p| p.at.div_euclid(window_secs)) else {
            return false;
        };
        let mut group = Vec::new();
        while let Some(point) = self.points.front() {
            if point.at.div_euclid(window_secs) != window {
                break;
            }
            if let Some(point) = self.points.pop_front() {
                group.push(point);
            }
        }
        if group.len() < 2 {
            for point in group.into_iter().rev() {
                self.points.push_front(point);
            }
            return false;
        }
        let group_bytes: usize = group.iter().map(|p| p.bytes).sum();
        self.bytes -= group_bytes;
        let data = aggregate_data(&group, rule);
        let bytes = Self::estimate_bytes(&data);
        let last = group.last().expect("group.len() >= 2");
        let merged = StoredPoint {
            seq: last.seq,
            at: last.at,
            bytes,
            data,
        };
        self.bytes += bytes;
        self.points.push_front(merged);
        self.totals.aggregated_groups += 1;
        true
    }

    fn evaluate_watermarks(&mut self, now: i64) -> Vec<BufferSignal> {
        let mut signals = Vec::new();
        if self.cfg.capacity_bytes == 0 {
            return signals;
        }
        let threshold = (self.cfg.capacity_bytes as f64 * HIGH_WATERMARK_RATIO) as usize;
        if self.bytes >= threshold {
            let due_audit = !self.high_latched
                || now
                    >= self
                        .last_high_audit_at
                        .saturating_add(HIGH_WATERMARK_AUDIT_INTERVAL_SECS);
            if due_audit {
                self.high_latched = true;
                self.last_high_audit_at = now;
                signals.push(BufferSignal::HighWatermark);
            }
        } else if self.high_latched {
            self.high_latched = false;
            signals.push(BufferSignal::Recovered);
        }
        signals
    }

    /// 取最旧的一批点；超过 `max_batch_bytes` 时截断（至少交付一个点）。
    pub fn snapshot(&mut self, max_batch_bytes: usize) -> Snapshot {
        let mut points = Vec::new();
        let mut size = 0usize;
        let mut truncated = false;
        for point in &self.points {
            if !points.is_empty() && size + point.bytes > max_batch_bytes {
                truncated = true;
                break;
            }
            size += point.bytes;
            points.push(SamplePoint {
                seq: point.seq,
                at: point.at,
                data: point.data.clone(),
            });
        }
        let meta = SnapshotMeta {
            id: self.next_snapshot,
            head_seq: points.first().map(|p| p.seq),
            tail_seq: points.last().map(|p| p.seq),
            buffer_tail_seq: self.points.back().map(|p| p.seq),
            truncated,
        };
        self.next_snapshot += 1;
        // 同一时间只允许一个未结算快照；旧票据再来结算会得到 StaleSnapshot。
        self.pending = Some(meta);
        Snapshot { meta, points }
    }

    /// 结算回执。校验失败时不改动任何点。
    pub fn commit(
        &mut self,
        snapshot_id: u64,
        verdict: Option<Verdict>,
        now: i64,
    ) -> Result<CommitOutcome, BufferError> {
        let Some(meta) = self.pending else {
            return Err(BufferError::StaleSnapshot(snapshot_id));
        };
        if meta.id != snapshot_id {
            return Err(BufferError::StaleSnapshot(snapshot_id));
        }
        let verdict = verdict.unwrap_or(Verdict::Keep);
        let upto = match &verdict {
            Verdict::Keep => None,
            Verdict::Discard => meta.tail_seq,
            Verdict::Persist { persisted_upto } | Verdict::Partial { persisted_upto } => {
                let (Some(head), Some(tail)) = (meta.head_seq, meta.tail_seq) else {
                    return Err(BufferError::EmptyBatchWatermark);
                };
                if *persisted_upto < head || *persisted_upto > tail {
                    return Err(BufferError::WatermarkOutOfRange {
                        upto: *persisted_upto,
                        head,
                        tail,
                    });
                }
                let exists = self
                    .points
                    .iter()
                    .any(|p| p.seq == *persisted_upto && p.seq >= head && p.seq <= tail);
                if !exists {
                    return Err(BufferError::WatermarkOutOfRange {
                        upto: *persisted_upto,
                        head,
                        tail,
                    });
                }
                Some(*persisted_upto)
            }
        };

        self.pending = None;
        let mut removed_points = 0usize;
        let mut removed_bytes = 0usize;
        if let Some(upto) = upto {
            let (points, bytes) = self.remove_through(upto);
            removed_points += points;
            removed_bytes += bytes;
        }
        // 未保留跨 tick：快照时刻之前的点无论 verdict 都不留。
        if !self.cfg.retain_across_ticks {
            if let Some(tail) = meta.buffer_tail_seq {
                let (points, bytes) = self.remove_through(tail);
                removed_points += points;
                removed_bytes += bytes;
            }
        }
        let signals = self.evaluate_watermarks(now);
        Ok(CommitOutcome {
            removed_points,
            removed_bytes,
            remaining_points: self.points.len(),
            remaining_bytes: self.bytes,
            signals,
        })
    }

    fn remove_through(&mut self, upto: u64) -> (usize, usize) {
        let mut removed_points = 0usize;
        let mut removed_bytes = 0usize;
        let mut kept = VecDeque::with_capacity(self.points.len());
        while let Some(point) = self.points.pop_front() {
            if point.seq <= upto {
                self.bytes -= point.bytes;
                removed_points += 1;
                removed_bytes += point.bytes;
                self.totals.consumed_points += 1;
                self.totals.consumed_bytes += point.bytes as u64;
            } else {
                kept.push_back(point);
            }
        }
        self.points = kept;
        (removed_points, removed_bytes)
    }

    /// 全局超限时对 `drop_oldest` 任务腾空间：丢最旧直到 `target_bytes`。
    pub fn drop_oldest_until(&mut self, target_bytes: usize) -> (usize, usize) {
        let mut removed_points = 0usize;
        let mut removed_bytes = 0usize;
        while self.bytes > target_bytes {
            let Some(point) = self.points.pop_front() else {
                break;
            };
            self.bytes -= point.bytes;
            removed_points += 1;
            removed_bytes += point.bytes;
            self.totals.dropped_points += 1;
            self.totals.dropped_bytes += point.bytes as u64;
        }
        (removed_points, removed_bytes)
    }

    pub fn clear(&mut self) -> (usize, usize) {
        let points = self.points.len();
        let bytes = self.bytes;
        self.totals.dropped_points += points as u64;
        self.totals.dropped_bytes += bytes as u64;
        self.points.clear();
        self.bytes = 0;
        (points, bytes)
    }
}

fn aggregate_data(points: &[StoredPoint], rule: AggregateRule) -> Value {
    if rule == AggregateRule::Last || !points.iter().all(|p| p.data.is_object()) {
        return points.last().map(|p| p.data.clone()).unwrap_or(Value::Null);
    }
    let mut keys: Vec<String> = Vec::new();
    for point in points {
        if let Some(object) = point.data.as_object() {
            for key in object.keys() {
                if !keys.iter().any(|k| k == key) {
                    keys.push(key.clone());
                }
            }
        }
    }
    let mut out = Map::new();
    for key in keys {
        let values: Vec<&Value> = points.iter().filter_map(|p| p.data.get(&key)).collect();
        let combined = if !values.is_empty() && values.iter().all(|v| v.is_number()) {
            combine_numbers(&values, rule)
        } else {
            points
                .last()
                .and_then(|p| p.data.get(&key))
                .cloned()
                .unwrap_or(Value::Null)
        };
        out.insert(key, combined);
    }
    Value::Object(out)
}

fn combine_numbers(values: &[&Value], rule: AggregateRule) -> Value {
    let integers: Option<Vec<i64>> = values.iter().map(|v| v.as_i64()).collect();
    match rule {
        AggregateRule::Sum => integers
            .map(|nums| json!(nums.iter().sum::<i64>()))
            .unwrap_or_else(|| json!(values.iter().filter_map(|v| v.as_f64()).sum::<f64>())),
        AggregateRule::Max => integers
            .map(|nums| json!(nums.iter().copied().max().unwrap_or_default()))
            .unwrap_or_else(|| {
                json!(values
                    .iter()
                    .filter_map(|v| v.as_f64())
                    .fold(f64::NEG_INFINITY, f64::max))
            }),
        AggregateRule::Min => integers
            .map(|nums| json!(nums.iter().copied().min().unwrap_or_default()))
            .unwrap_or_else(|| {
                json!(values
                    .iter()
                    .filter_map(|v| v.as_f64())
                    .fold(f64::INFINITY, f64::min))
            }),
        AggregateRule::Last => values.last().map(|v| (*v).clone()).unwrap_or(Value::Null),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(capacity: usize, overflow: OverflowPolicy) -> BufferConfig {
        BufferConfig {
            capacity_bytes: capacity,
            overflow,
            ..BufferConfig::default()
        }
    }

    fn push_n(buffer: &mut Buffer, n: usize, value: i64) {
        for i in 0..n {
            let _ = buffer.push(i as i64, json!({ "v": value }), 0);
        }
    }

    #[test]
    fn drop_oldest_keeps_latest_and_counts_drops() {
        let mut buffer = Buffer::new(cfg(130, OverflowPolicy::DropOldest), None);
        push_n(&mut buffer, 3, 1);
        assert_eq!(buffer.len(), 3);
        let report = buffer.push(3, json!({ "v": 2 }), 0);
        assert!(report.outcome.is_accepted());
        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer.stat().dropped_points, 1);
        let tail = buffer.stat().tail_seq.unwrap();
        assert_eq!(tail, 3);
    }

    #[test]
    fn reject_refuses_new_point_and_keeps_old() {
        let mut buffer = Buffer::new(cfg(130, OverflowPolicy::Reject), None);
        push_n(&mut buffer, 3, 1);
        let report = buffer.push(3, json!({ "v": 2 }), 0);
        assert!(matches!(
            report.outcome,
            PushOutcome::Rejected {
                reason: RejectReason::Full,
                ..
            }
        ));
        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer.stat().rejected_points, 1);
        assert_eq!(buffer.stat().tail_seq, Some(2));
    }

    #[test]
    fn aggregate_merges_oldest_window() {
        let mut cfg = cfg(130, OverflowPolicy::Aggregate);
        cfg.aggregate_window_secs = 60;
        let mut buffer = Buffer::new(cfg, Some(AggregateRule::Sum));
        push_n(&mut buffer, 3, 1);
        let report = buffer.push(3, json!({ "v": 4 }), 0);
        assert!(report.outcome.is_accepted());
        assert_eq!(buffer.len(), 2);
        assert!(buffer.stat().aggregated_groups >= 1);
        let head = buffer.points.front().unwrap().data.clone();
        // 窗口内 1+1+1 合成 3，窗口外的新点仍是 4
        assert_eq!(head["v"], 3);
        assert_eq!(buffer.points.back().unwrap().data["v"], 4);
    }

    #[test]
    fn aggregate_without_rule_degrades_to_drop_oldest() {
        let mut buffer = Buffer::new(cfg(130, OverflowPolicy::Aggregate), None);
        push_n(&mut buffer, 3, 1);
        let report = buffer.push(3, json!({ "v": 2 }), 0);
        assert!(report.outcome.is_accepted());
        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer.stat().dropped_points, 1);
        assert_eq!(buffer.stat().aggregated_groups, 0);
    }

    #[test]
    fn oversized_point_is_rejected() {
        let mut buffer = Buffer::new(cfg(1024 * 1024, OverflowPolicy::DropOldest), None);
        let big = json!({ "blob": "x".repeat(POINT_MAX_BYTES) });
        let report = buffer.push(0, big, 0);
        assert!(matches!(
            report.outcome,
            PushOutcome::Rejected {
                reason: RejectReason::PointTooLarge,
                ..
            }
        ));
        assert!(report.signals.contains(&BufferSignal::PointTooLarge));
        assert!(buffer.is_empty());
    }

    #[test]
    fn snapshot_truncates_by_max_batch_bytes() {
        let mut buffer = Buffer::new(cfg(4096, OverflowPolicy::DropOldest), None);
        push_n(&mut buffer, 6, 1);
        let snapshot = buffer.snapshot(90);
        assert!(snapshot.meta.truncated);
        assert_eq!(snapshot.points.len(), 2);
    }

    #[test]
    fn persist_removes_only_up_to_watermark() {
        let mut buffer = Buffer::new(cfg(4096, OverflowPolicy::DropOldest), None);
        push_n(&mut buffer, 4, 1);
        let snapshot = buffer.snapshot(4096);
        let upto = snapshot.points[1].seq;
        let outcome = buffer
            .commit(
                snapshot.meta.id,
                Some(Verdict::Persist {
                    persisted_upto: upto,
                }),
                0,
            )
            .unwrap();
        assert_eq!(outcome.removed_points, 2);
        assert_eq!(buffer.len(), 2);
        assert_eq!(buffer.stat().head_seq, Some(upto + 1));
    }

    #[test]
    fn keep_retains_all_and_partial_keeps_remainder() {
        let mut buffer = Buffer::new(cfg(4096, OverflowPolicy::DropOldest), None);
        push_n(&mut buffer, 4, 1);
        let snapshot = buffer.snapshot(4096);
        let outcome = buffer.commit(snapshot.meta.id, None, 0).unwrap();
        assert_eq!(outcome.removed_points, 0);
        assert_eq!(buffer.len(), 4);

        let snapshot = buffer.snapshot(4096);
        let upto = snapshot.points[2].seq;
        let outcome = buffer
            .commit(
                snapshot.meta.id,
                Some(Verdict::Partial {
                    persisted_upto: upto,
                }),
                0,
            )
            .unwrap();
        assert_eq!(outcome.removed_points, 3);
        assert_eq!(buffer.len(), 1);
    }

    #[test]
    fn discard_drops_the_whole_batch() {
        let mut buffer = Buffer::new(cfg(4096, OverflowPolicy::DropOldest), None);
        push_n(&mut buffer, 5, 1);
        let snapshot = buffer.snapshot(90);
        assert_eq!(snapshot.points.len(), 2);
        let outcome = buffer
            .commit(snapshot.meta.id, Some(Verdict::Discard), 0)
            .unwrap();
        assert_eq!(outcome.removed_points, 2);
        assert_eq!(buffer.len(), 3);
    }

    #[test]
    fn watermark_out_of_range_leaves_buffer_untouched() {
        let mut buffer = Buffer::new(cfg(4096, OverflowPolicy::DropOldest), None);
        push_n(&mut buffer, 3, 1);
        let snapshot = buffer.snapshot(4096);
        let tail = snapshot.points.last().unwrap().seq;
        let before = buffer.stat();
        let err = buffer
            .commit(
                snapshot.meta.id,
                Some(Verdict::Persist {
                    persisted_upto: tail + 10,
                }),
                0,
            )
            .unwrap_err();
        assert!(matches!(err, BufferError::WatermarkOutOfRange { .. }));
        assert_eq!(buffer.stat(), before);
        assert_eq!(buffer.len(), 3);
        // 校验失败后快照仍可重试
        assert!(buffer
            .commit(snapshot.meta.id, Some(Verdict::Keep), 0)
            .is_ok());
    }

    #[test]
    fn empty_batch_cannot_declare_watermark() {
        let mut buffer = Buffer::new(cfg(4096, OverflowPolicy::DropOldest), None);
        let snapshot = buffer.snapshot(4096);
        let err = buffer
            .commit(
                snapshot.meta.id,
                Some(Verdict::Persist { persisted_upto: 0 }),
                0,
            )
            .unwrap_err();
        assert_eq!(err, BufferError::EmptyBatchWatermark);
    }

    #[test]
    fn snapshot_cannot_be_consumed_twice() {
        let mut buffer = Buffer::new(cfg(4096, OverflowPolicy::DropOldest), None);
        push_n(&mut buffer, 3, 1);
        let snapshot = buffer.snapshot(4096);
        let upto = snapshot.points[1].seq;
        assert!(buffer
            .commit(
                snapshot.meta.id,
                Some(Verdict::Persist {
                    persisted_upto: upto
                }),
                0
            )
            .is_ok());
        let err = buffer
            .commit(snapshot.meta.id, Some(Verdict::Keep), 0)
            .unwrap_err();
        assert_eq!(err, BufferError::StaleSnapshot(snapshot.meta.id));
        assert_eq!(buffer.len(), 1);
    }

    #[test]
    fn retain_across_ticks_false_clears_untaken_points() {
        let mut cfg = cfg(4096, OverflowPolicy::DropOldest);
        cfg.retain_across_ticks = false;
        let mut buffer = Buffer::new(cfg, None);
        push_n(&mut buffer, 5, 1);
        let snapshot = buffer.snapshot(90);
        assert_eq!(snapshot.points.len(), 2);
        let outcome = buffer
            .commit(snapshot.meta.id, Some(Verdict::Keep), 0)
            .unwrap();
        assert_eq!(outcome.removed_points, 5);
        assert!(buffer.is_empty());
    }

    #[test]
    fn high_watermark_and_recovery_signals() {
        // 4 点 * 39 B = 156 B，容量 160 B 的 80% 阈值是 128 B
        let mut buffer = Buffer::new(cfg(160, OverflowPolicy::DropOldest), None);
        push_n(&mut buffer, 4, 1);
        assert!(buffer.stat().high_watermark);
        let snapshot = buffer.snapshot(4096);
        // keep 不改变占用，仍高于阈值 → 不再重复告警
        let outcome = buffer
            .commit(snapshot.meta.id, Some(Verdict::Keep), 10)
            .unwrap();
        assert!(!outcome.signals.contains(&BufferSignal::HighWatermark));
        // 全部丢弃后回落到 0，产生一次 recovered
        let snapshot = buffer.snapshot(4096);
        let outcome = buffer
            .commit(snapshot.meta.id, Some(Verdict::Discard), 20)
            .unwrap();
        assert!(outcome.signals.contains(&BufferSignal::Recovered));
        assert!(!buffer.stat().high_watermark);
    }

    #[test]
    fn estimate_is_serialized_bytes_plus_overhead() {
        let value = json!({ "v": 1 });
        let raw = serde_json::to_vec(&value).unwrap().len();
        assert_eq!(Buffer::estimate_bytes(&value), raw + POINT_OVERHEAD_BYTES);
    }
}
