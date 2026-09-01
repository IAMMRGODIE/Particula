# Particula — 实验性声音设计粒子效果器 · 架构文档

> 状态：v0 / v1 / v2·WSOLA 纹理层 已实现并通过 15 个测试；剩余 v2 项：BPM sync、stereo、CLAP 导出。
> 关联代码库：`i_am_dsp`（workspace 根：`i_am_dsp/Cargo.toml`）。particula 计划成为该 workspace 的第 6 个成员。

## 1. 定位与目标

- 共享 history 的粒子云：大量粒子从同一条共享延迟线（history）的任意位置读取音频，各自做音高/频率位移/增益包络处理后，把输出反馈写回同一条延迟线。
- 特色组合（每个粒子）：粒度重采样音高 + IIR Hilbert 频率位移 + 位置调制（固定 / LFO / 随机 / 峰值跟随）+ 反馈。
- 纯参数驱动出生：全部出生调度与随机发生在音频线程内，自包含、可复现；UI 只调全局速率与范围。
- WSOLA 作为**全局纹理层**（不是每粒子方案），负责整段 history 的时间拉伸质感（v2 加入）。

## 2. 引擎总览（数据流）

```
                  ┌──────────────────────────────────────────────┐
 input ──────────▶│ ParticulaEngine (Effect<CHANNELS>)            │
                  │                                              │
                  │  history: RingBuffer<f32>  (共享, 容量=最大延迟) │
                  │    ▲ push(input) —— 固定写"写入头"              │
                  │    ▲ 反馈写入  —— 可选的注入点（见 §3.1）        │
                  │                                              │
                  │  [WSOLA 纹理层 v2] 每 half_history 刷新一次     │
                  │    texture: stretched_buffer ──┐              │
                  │                                │ 可选读取源     │
                  │  [粒子池 slot-map, 64~256, 零分配] │            │
                  │   每粒子：                      ▼              │
                  │    pos_target → DoubleTimeConstant → pos(t)  │
                  │    s = source.sample(t)  ╴ history 或 texture │
                  │    s × playback_rate (粒度重采样=音高)          │
                  │    → IIRFreqShifter<4, 1> (Hilbert 频率位移)   │
                  │    × envelope (指数衰减)                       │
                  │    output += s ; history.add_at(注入点, s·fb)  │
                  │  [Spawner] 纯参数驱动 + BPM sync（可选）        │
                  └──────────────────────────────────────────────┘
                                  │
                                  ▼
                          dry + wet ───▶ output
```

## 3. 共享 history（RingBuffer）语义

### 3.1 写入

- **干声输入：永远写"写入头"（`current_pos`）**。滑窗语义是定义性约束，不可移动；任何"可选择写入位置"都不适用于输入。
- **反馈写入：注入点可选**。引入 `feedback_delay: f32`（相对写入头的延迟，`0..N`）：
  - `feedback_delay = 0`：即伪代码的 `history[-1]`，反馈同帧即被后续粒子读到（串行云，混沌、自生成感最强）。
  - `feedback_delay > 0`：反馈在更早的历史位置再注入，形成有节奏感的"再注入"效果（反馈延迟线式）。
  - 实现：给 `RingBuffer` 增加 `add_at(index, value)` 写接口（TODO：库扩展），或在粒子层用 `underlying_buffer_mut()` 手动取模累加，写回处加 soft-knee。
- **同帧可见性（串行语义，已定）**：粒子按池迭代顺序处理，编号靠后的粒子能读到编号靠前粒子本帧写回的反馈。此为特性，文档化，不"修复"。
- 环路稳定性：反馈增益 < 1 留裕量；环路加一阶低通/DC 阻塞；写回 soft-knee，防多粒子同 slot 累加爆炸。

### 3.2 容量

- `capacity` = 最大可达延迟上限；任何读取 clamp 到 `[0, capacity-1]`。
- `resize()` 会使内容失效（`current_pos` 归零，`ring_buffer.rs`），运行中改变容量 = 清空旧内容；粒子位置（t 空间）不受影响（见 §4）。

## 4. Position 语义（f32 / WaveTable 空间）

- **position 一律为 f32 的 `t ∈ [0, 1]`（WaveTable 语义）**：0 = 最老，1 = 最新。对应 `RingBuffer<f32>` 的 `WaveTable` 实现（`wavetable.rs:221`，`t * (capacity-1)` + 三次插值）。
- 理由：容量/长度变化不影响位置语义；与读取机制（`WaveTable::sample` 三次插值）之间零单位转换。粒子内部从不出现 usize 位置。
- 所有位置调制源（固定值 / LFO / 随机 / 峰值跟随）**直接产出 t**；链路为 `pos_target → DoubleTimeConstant 平滑 → 实际读取位置`，杜绝突变 click。
- 峰值跟随：在 t 空间按头相对索引扫描（`i → t = i/(capacity-1)`），与 WaveTable 隐藏的地址映射保持一致；短窗 RMS/峰 + 滞后 + 平滑。
- 边界：三次插值在 t 边缘需要 clamp；`t ≈ 1` 即最新样本（本帧刚写入的干声），允许读；
- **冷启动填充**：t=0 是最老样本、t=1 是最新，`t·(capacity-1)` 索引区域的音频要等对应样本写入后才存在。新引擎/大 history 的前 `t·capacity` 个样本中，读取点靠前的粒子会读到静音（v0 实测：capacity=4096、t=0.25 需约 1024 样本填充）。onset 靠近 1 可最快读到新鲜音频；行为符合滑窗语义，文档化即可。
- 出生初值（等差数列 + 随机偏移）也直接落在 t 空间。

## 5. 粒子

### 5.1 数据结构（AoS 起步，预留 SoA/SIMD 标记）

每粒子：

- `position_target: f32` 与 `position: f32`（t 空间，经平滑器）
- `playback_rate: f32`（粒度播放速率，1.0 = 原速；≠ 1 即音高变化）
- `freq_shift: f32`（Hz，频率位移）
- envelope 状态（阶段 + 当前增益，指数衰减）
- `feedback_gain: f32`（spawn 时快照；延迟点与阻尼系数由引擎每样本传入）和反馈阻尼低通状态
- `IIRHilbert<4> + Biquad` 滤波状态（频率位移用）
- LFO 相位（波形共享、相位私有）
- 寿命（采样计数）与死亡标志

### 5.2 生命周期

- 出生：Spawner 输出 `(初始 t, pitch, freq_shift, 初始增益, 寿命)`；写入 slot-map 空闲槽。
- 处理：见 §5.3 管线。
- 死亡：寿命耗尽 → 增益已趋零 → 入 free-list，槽位可复用。
- 音频线程零分配原则：spawn/despawn 均为槽位周转，不发生堆分配。

### 5.3 处理管线（采样级，每粒子）

1. `t = smoother(position)`
2. `s = source.sample(t)`（source ∈ { history, texture(v2) }，三次插值已在 WaveTable 内）
3. `s = s * playback_rate`（粒度重采样；速率变化即音高变化）
4. `s = iir_freq_shifter.process(s)`（`IIRFreqShifter<ORDER=4, CHANNELS=1>`）
5. `s = s * envelope`（线性 attack + 指数衰减至生命终点 -60 dB，采样级连续）
6. `output += s`；反馈写回：`fb = soft_clip(s * feedback_gain)` → 单极点低通阻尼 → `history.add_at(feedback_delay, fb)`（v1 已实现，串行语义）

注意：ORDER 是编译期常量（粒子池要求同型），选定 4（预算 64~256 粒子，见 §10）。

## 6. Spawner（出生规则）

- 纯参数驱动、音频线程内调度；规则 = **等差数列初值 + 随机偏移 + 每代 × decay_ratio（指数衰减）**。
- Spawner 输出一个 `Spawn`：`(初始 t, pitch, freq_shift, 初始增益, 寿命)`。
- 调度模式：`FreeRun`（按采样/秒密度）或 `Beats`（BPM 量化，见 §7）。
- 可复现性：引擎含固定随机种子开关，便于复现实验 patch。

## 7. BPM Sync

- 数据源：`ProcessContext::infos()` → `ProcessInfos { tempo: Option<f32>, trustable, playing, current_bar_number, … }`（`lib.rs:287`）。
- 工具：`tools/bpm_syncer.rs` —— `next_k(bpm, samples)` 按块累加分数拍相位（支持动态 BPM），`read()` 读当前拍位，`reset()` 归零。
- 用法：引擎每 block 用 `next_k(tempo, block_len)` 推进；Spawner 在 Beats 模式按拍相位跨过量化阈值触发（如每 1/16 拍）。
- 回退：`tempo` 为 None / 不 trustable / 未播放 → 回退到用户设定默认 BPM 或 FreeRun。
- 传输重启（`playing` false→true）：`reset()` 或从 `current_bar_number` 重建相位，避免偏移漂移。
- 扩展（后续）：LFO 周期按拍、envelope 时长按拍、节奏 gate 模式、整小节 pattern 门控（用 `current_bar_number`）。

## 8. 反馈路径与稳定性（汇总）

- 串行可见性（v1 已实现，§3.1）：粒子按池迭代顺序写回，本帧生效。
- 注入点可选：`feedback_delay`（0..2000 ms，v1 已实现；delay=0 即 `history[-1]`，delay=cap-1 即最老槽）。
- 稳定性三件套（v1 已实现）：增益 ≤0.99、`feedback_damping_hz` 单极点低通（0 = 直通）、写回 soft-clip（`x/(1+|x|)`）。
- 反馈延迟与插值窗口在 t 边缘的读写混叠行为：文档化即可（混沌特征的一部分）。

## 9. WSOLA 纹理层（v2）✅

- **已实现**：`texture.rs` 的 `Texture` —— 自带滑动窗口 tap（跟随 history 最新样本）→ 每 `texture_refresh_ms` 用 `tools/wsola.rs` 批量拉伸成冻结 wavetable（`texture_window_ms` 窗口，`texture_stretch` 0.25..4）→ 粒子经 `texture_blend`（0..1）混合读取。
- **防 zipper**：每次刷新都做旧→新交叉淡化（`texture_crossfade_ms`），同时覆盖"素材滑动"与"stretch 参数突变"两类边界跳变。
- 与粒子音高独立：粒子 pitch 仍是粒度重采样；纹理层只提供全局素材拉伸质感。
- 成本：批量 wsola 每刷新一次（~43ms @48k 默认），低频可接受；纹理 Vec 刷新有分配（零分配原则宽松项）。

## 10. 性能规则

- 音频线程零分配（粒子池 slot-map + free-list；WSOLA 纹理层低频刷新允许少量分配，后续可复用 buffer）。
- 粒子预算：64~256 → `IIRFreqShifter<ORDER=4, 1>`（代码中为 `FREQ_SHIFTER_ORDER = 4`）；AoS 起步，代码里预留 SoA/SIMD 迁移标记。
- 每粒子每样本成本估算：三次插值（~4 次表读）+ IIR Hilbert（ORDER=4 → 8 个二阶全通 ≈ 16~32 MAC）+ Biquad + 相位振荡（sin/cos 用递推或 sin_table）+ envelope。256 粒子 × 48kHz 桌面无压力。

## 11. 模块划分与集成

- 位置：`C:\projects\dsp\particula`，注册为 `i_am_dsp/Cargo.toml` workspace 第 6 个成员（共享 `[workspace.dependencies]` / `i_am_dsp_derive`）。
- 文件：
  - `engine.rs` — `Effect<CHANNELS>` 实现、每 block 编排
  - `history.rs` — 反馈注入点写接口 `add_at` + 峰值扫描 `recent_peak_position`
  - `particle.rs` — 粒子状态机（含 envelope/lifetime/滤波状态）
  - `spawner.rs` — 出生规则与调度（FreeRun / Beats）
  - `position_mod.rs` — 位置调制源（固定/LFO/随机/峰值跟随）+ 平滑
  - `texture.rs` — WSOLA 纹理层（v2 ✅）：滑动窗口 tap + 定时拉伸 + 交叉淡化
- 接口：实现 `Effect<CHANNELS>`；通过 `ProcessContext` 取 `sample_rate` / `tempo` / 传输信息。

## 12. 里程碑

- **v0 ✅**：mono 粒子云 —— 读点(三次插值) + 位置平滑 + envelope(-60dB) + 出生规则(等差+抖动+指数衰减) + 成本验证。
- **v1 ✅**：串行反馈（`feedback_delay` 注入点 + 阻尼 + soft-clip）+ 峰值跟随 position（`position_mode=3`，周期更新近窗峰值）。
- **v2**：WSOLA 纹理层 ✅ + BPM sync ⏳ + stereo ⏳ + UI/CLAP ⏳。

## 13. 待定项 / TODO

- `RingBuffer::resize()` 运行时容量变化的 UI 策略（清空 vs 双 buffer 过渡）。
- 峰值跟随目前取近窗全局峰值；如需"当前位置附近局部峰值吸附"（用户原话），加局部搜索模式。
- 反馈 t 边缘与插值窗口的读写混叠文档。
- BPM reset / transport 重启策略细节。
- ~~纹理层交叉淡化参数~~ ✅（`texture_crossfade_ms`）。
