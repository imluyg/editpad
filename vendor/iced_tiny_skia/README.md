# iced_tiny_skia（本地补丁副本）

本目录是 [`iced_tiny_skia`](https://github.com/iced-rs/iced) **0.14.0** 的就地维护副本，
经根 `Cargo.toml` 的 `[patch.crates-io]` 接管 Editpad 的软件渲染后端。

- **上游**：https://github.com/iced-rs/iced
- **许可**：MIT（见同目录 `LICENSE`，版权归 Héctor Ramón 与 Iced contributors）；
  Editpad 自身为 Apache-2.0，本目录**不适用**该许可，仍按上游 MIT 分发。

## 相对上游的改动

| 标记 | 位置 | 改动 |
|------|------|------|
| P117 | `src/window/compositor.rs` | 禁用本后端的「部分损伤呈现」，恒整帧重绘——自绘编辑器多层 clip + 选区带 quad 场景下，损伤计算与 softbuffer 复用缓冲失步会留下选区残线（实测代价 ≈ +0.13ms/帧） |
| P119 / P120 | `src/window/compositor.rs` | 层栈逐帧入队限深并改为探针期才入队：上游 `present` 的 `truncate` 随部分损伤呈现一起被禁用后，层栈只进不出，空闲与打字期都在持续堆积 |

改动点在源码中均以 `P117` / `P119` / `P120` 注释标注，便于与上游 diff 对照。
待上游定位损伤失步根因后，本副本可整体移除、改回 crates.io 版本。
