# Particula Cloud

实验性声音设计用颗粒效果器 —— 一个 CLAP 插件。

共享历史缓冲 + 粒子池 + 串行反馈 + WSOLA 纹理层 + BPM 同步，
配一套 Homology 风格的活体控制面（中心旋转 sigil、出生即点亮的粒子、左右浮动参数面板）。

> 定位：实验性声音设计工具。它不一定"像"某个传统效果器 —— 它是一个可以放进 DAW 的颗粒云引擎。

## 特性

**声音引擎**

- 单根共享历史延迟线（1 << 16 样本，约 1.36 s / 48 kHz），干声先落盘、粒子再读
- 粒子池：上限 192，随机出生位置（算术序列 + jitter）、增益按 decay^n 衰减并带下限（gain floor）
- **串行反馈**：同一帧内后出生的粒子能读到前面粒子写回的反馈（软限幅 + 单极点阻尼），反馈注入距离可设 ms 或 **BPM 拍**
- **WSOLA 纹理层**：滑动窗定时批量时间拉伸 + 相邻刷新交叉淡化，粒子可选混合纹理与原始历史
- **BPM 同步**：出生网格、LFO 速率、反馈延迟三处都支持"拍"单位，随宿主 tempo 走（无 transport 时用 fallback BPM）
- 位置调制四模式：**Fixed / LFO / RandomWalk / PeakFollow**；LFO 有 Sine / Triangle / Saw / Square 四种波形
- 反向播放（按概率）、每粒子 IIR Hilbert 频移（±5000 Hz）
- 立体声：等功率 pan 分布；主旁路开关；PANIC 一键静置（清空历史 / 纹理 / 粒子）

**控制面（HOMOLOGY 风格）**

- 中心 sigil：三层同心圆差速旋转 + 神圣几何线稿 + **粒子出生点亮**（亮度随粒子寿命衰减，落点洗牌随机）—— 有面板时图案平滑让位
- 点击左 / 右半区分别淡入参数面板（互斥），面板顶部 I II III · TITLE 分页签
- 左面板：SPAWN / LAW / SHAPE；右面板：MOVEMENT / MATERIAL / OUTPUT（MOVEMENT 随模式条件显示专属参数，LFO 块常驻）
- header：Logo → About（整体渐入渐出）、WET / DRY 对数滑杆、ON/OFF
- footer：LIVE / SPAWNED / HZ 统计、**PANIC**、**RANDOMIZE**（SplitMix64 无偏采样 + 平滑过渡，保留 dry/wet/enabled）
- 交互：对数滑杆（时间/频率/拍/增益类）、**双击参数标题**恢复出厂默认（平滑）、**双击右侧数值**吸附到最近 2 的幂（beats / pitch / stretch）—— 均为平滑动画
- 内嵌 Crimson Text 衬线显示字体，不依赖系统字体

## 快速开始

前置：Rust stable；本项目离线构建（无网络），依赖通过拷贝的 Cargo.lock 固定，已被 gitignore。

### 构建 CLAP

```bash
cargo build --release -p particula_plugin
# 产出 .clap（dll 改成 CLAP 扩展名）
cp target/release/particula_plugin.dll target/release/Particula.clap
# 把 ParticulaCloud.clap 放进 DAW 的 CLAP 插件目录即可
```

### Standalone（不依赖 DAW）

```bash
cargo run --release -p particula_plugin --example standalone --offline
```

### 无头探针（验证 wet 通路）

```bash
cargo run --release -p particula_plugin --example clap_probe --offline -- target/release/ParticulaCloud.clap
```

> standalone 依赖 i_am_dsp / i_am_plugin 的本地提交；其中 i_am_plugin 的
> default_value 修正（参数元数据从原子值读取）与频移旋转递推需要先 apply 到
> i_am_dsp 仓库，否则宿主自动化或高频粒子负载会异常。

## 架构

详见 [Architecture.md](Architecture.md)。数据流：

```
dry in ──► mono mix ──► history ring (1<<16) ──► WSOLA texture（滑动窗 + 批量拉伸 + crossfade）
                          ▲        │                    │
              feedback 写回 │        └──► 粒子池（上限192）◄┘ 可选混合
                          │                │ 每粒子·每样本：
                          └──── soft clip + 阻尼 + 延迟注入   读出（线性插值）× 包络 × 频移 × pan
```

线程模型：GUI ↔ 音频只通过原子 ParamMap、spawn 事件通道、PANIC 闩锁通信；音频线程无锁、无分配。

## 参数总览（44 个，全部 host 可自动化）

| 组 | 参数 |
|---|---|
| 主 | dry, wet(0..4 补偿增益), enabled |
| Spawn | spawn_sync（BPM 网格）, spawn_interval_ms/beats, fallback_bpm, max_particles(≤192), reverse_chance |
| Law | base_position, position_step, position_jitter, gain_decay_ratio, min_gain_ratio(floor), initial_gain |
| Shape | attack_ms, lifetime_ms_min/max, pitch_min/max, freq_shift_min/max |
| Movement | position_mode, position_smooth_ms, lfo_wave, lfo_rate_hz/beats, lfo_depth, random_walk_step/interval, peak_window/update/threshold |
| Feedback | feedback_gain, feedback_delay_ms/beats, feedback_damping_hz |
| Texture | texture_blend, texture_window/refresh/crossfade_ms, texture_stretch |
| Output | pan_min/max |

## 开发

```bash
cargo test --offline                        # 全套行为测试（spawn 节律/反馈/PANIC/BPM…）
cargo test --release --test high_density --offline -- --nocapture
                                            # 高密度基准：128→0.39x / 256→0.50x / 384→0.51x
                                            # （相对实时负载，release、48 kHz、2 s 音频）
cargo clippy -p particula -p particula_plugin --all-targets --offline   # 保持零警告
```

性能要点（引擎热路径）：粒子侧线性插值读（位掩码回绕）、LFO 查找表、
IIR 频移载波旋转递推（替代逐样本 cos/sin）—— 384 满载约 0.5x 实时负载。

## 目录

- src/：引擎（engine / particle / spawner / texture / position_mod / history / rng）
- plugin/src/：CLAP 壳（lib.rs）与 UI（ui.rs）
- plugin/examples/：standalone 与 clap_probe
- tests/：行为测试 + 基准
- Architecture.md：设计文档（读模型、反馈稳定性、纹理、BPM）

## 许可证

MPL-2.0（见 Cargo.toml 的 license 字段；副本可在 https://mozilla.org/MPL/2.0/ 获取）。Crimson Text 衬线字体为 OFL 开源许可，随包分发。
