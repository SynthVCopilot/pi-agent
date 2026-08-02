# cvrs — Cross-Version Render Service（跨版本渲染搬运）

把 SV1/SV2 之间的**渲染结果**当作**静音参考音频轨**在 .svp 文件层搬运。
**只写不读**：绝不读取/翻译异版本工程的可编辑唱法语义（跨界直译必坏）。
与 synthv-agent-bridge 正交——桥管同版本实时控制，CVRS 管跨版本冷搬运。

## 为什么在文件层而非 Lua API 层

桥的 Lua 脚本 API **不能渲染音频、不能保存工程**（`bounce` 只是冻结状态标志，非渲染器），
且刻意不读写 .svp。跨版本又有语义断裂，所以 CVRS 独立走 .svp 文件级。

## SVP format 版本速查（本机 30+ 工程实测）

| format | 时代 | 标志 |
|---|---|---|
| 120–134 | **SV1** | 唱法在 `track.mainRef.voice`；v134 起有说唱(musicalType) |
| 153 | **SV2** 2.0-2.1 | 唱法上移到 `mainGroup.vocalModes`；+exportPitch |
| 187 | **SV2** 2.2.x | +pitchControls(Smart Pitch)、mouthOpening、startTimeSeconds |

断裂边界 v134→v153。CVRS 不跨这条线搬语义，只搬 wav+时间轴占位。

## 子命令

```bash
python cvrs.py probe target.svp          # 只读：版本/时代/轨列表/格式标记
python cvrs.py add-ref target.svp --audio render.wav --name "SV1渲染参考"
```

`add-ref` 把 wav 写成一条 **静音(mixer.mute) + 不渲染(renderEnabled=false) + isInstrumental**
的参考轨，**从目标自身克隆空轨壳**以保证 schema 与目标版本完全一致（这是"只写不读"下
唯一读目标结构的地方，且只搬骨架、清空全部音符/参数/唱法）。输出落
`~/.SynthVcopilot/output/<目标>_cvrs.svp`，**源文件不改动、禁 `..` 穿透**。

参数：`--begin-seconds`（音频起始）、`--out`（输出名）。wav 时长由 ffprobe 自动探测（可选）。

## 当前范围（用户定：先不做渲染）

产生 wav 的**渲染步不在本组件内**——wav 路径由调用方提供。渲染后续可接：
DLL 注入 bounce / UI 自动化导出 / 人工渲染+看门狗（见 agents_memory Pi_Agent/002）。

## 数据纪律
输出统一 `~/.SynthVcopilot/output/`，硬禁 `..` 穿透，绝对路径仅数据根内放行。
