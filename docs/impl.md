# 实现说明与审计索引

> 契约写「应该怎样」，这里写「代码实际怎样、差在哪」。
> 正文在 [`impl/`](impl/)。

## 总则（共性）

- `*_IMPL_NOTES.md` 是常设差异台账：实现与契约不一致处逐条记录，修平后删条目；不复述契约正文。
- 审计稿带时效性：**只保留未处理的问题、搁置项与未验证项**；已落地的批次与执行记录随处理完即删（过程留痕在 `docs/changes/`）。行号易腐化，只作结论参考。
- 唯一例外：路径授权代数只在 `skiff-model::path`，仅 `skiff-fs` 的 I/O 感知解析器例外，见 [KERNEL_FS_IMPL_NOTES.md](impl/KERNEL_FS_IMPL_NOTES.md) §2.2.1。

## 子文档

| 子文档 | 管什么 |
| --- | --- |
| [KERNEL_FS_IMPL_NOTES.md](impl/KERNEL_FS_IMPL_NOTES.md) | `skiff-fs` 实现差异；路径解析唯一例外见 §2.2.1 |
| [KERNEL_SCHEDULER_IMPL_NOTES.md](impl/KERNEL_SCHEDULER_IMPL_NOTES.md) | `skiff-sched` tick 语义与回执边界 |
| [KERNEL_UI_BOUNDARY_AUDIT.md](impl/KERNEL_UI_BOUNDARY_AUDIT.md) | SSR / `/ui/v1` handler 边界：未闭环问题与待决策批次（审计稿） |
| [KERNEL_ARCH_AUDIT.md](impl/KERNEL_ARCH_AUDIT.md) | crate 划分与依赖方向七维结论、非问题清单、搁置项与未验证项（已收口） |
| [RUNTIME_PROVISION_SPEED.md](impl/RUNTIME_PROVISION_SPEED.md) | 运行环境安装速度：实测基线、用户场景矩阵、L0–L3 分层方案与验收口径 |
