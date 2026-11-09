# binary-rust 设计草案

目标：与 `Mikodev.Binary`(C#) / `binary-cxx`(C++) 共享同一套 wire 格式，但用 Rust 的方式重新组织。
仓库从孤儿分支起步，没有需要兼容的旧实现。

---

## 0. 结论

1. **不要移植 `Generator` + 虚接口那一层**。Rust 没有反射，`TypeId` 注册表拿不到"任意类型的转换器"，
   这套架构在 Rust 里只能退化成一堆 `Box<dyn Any>` 下转型。改成 **静态 trait + derive 宏**。
2. **三个 trait**：`Layout`（类型级定长信息）、`Encode`、`Decode<'de>`。
3. **`Decode` 第一天就带生命周期** `'de`，哪怕 v1 只返回 owned 类型。事后补生命周期是破坏性改动。
4. **长度预测只在 converter 内部、且只用 O(1) 信息**。不暴露长度查询接口，也不递归遍历值树
   （§10 已实施）：定长类型用 `LEN` 常量，简单变长类型用 `len()`，其余一律 anchor 兜底。
   压缩（1 字节 vs 4 字节）只为了节省空间；**但压缩阈值必须与 C#/C++ 相同**：
   anchor 路径只在 `actual <= 16` 时压缩（§12 实测），不做通用 compact——这既是字节对齐，
   也避免 `FinishAnchor` 的 `memmove`。
5. **编码器是单一具体类型** `Encoder<'a>`（内部 Owned/Borrowed 两种后端），不用泛型 `Sink`——
   否则 `Encode` 失去 object safety，动态层没法做，而且单态化会把编译时间打爆。**缓冲容量由
   调用方在构造期给上界**（`with_capacity` / `on_slice`），库不做增长启发式：估算要连前缀一起
   算，少算 1 字节比不估更慢（§19.15.15）。
6. 错误用具体 `Error` enum，默认不装箱；core crate 做成 `no_std + alloc`。
   自定义类型自己的错误类型**不能穿透 trait**，只能在边界降级，且降级方式要显式选：
   `Error::message`（只留消息）/ `Error::message_static`（零分配）/ `Error::wrapped`
   （保留类型与 chain，付一次 `Arc` + 文本相等）（§18）。**整体擦除成 `dyn Error`** 的候选已
   评估：benchmark 证明代价可忽略（§19.3），结论仍按可观测性保留枚举（§18.8.7）。
   兼容性改由 **`#[non_exhaustive]`** 买单（§18.8.5）：加变体是 minor，改名与删除仍是 breaking。
7. **「对齐」的验收标准是 fixture 字节断言，不是「应该能互解」**：从 C#/C++ 导出真实 wire
   存成语料，Rust 侧逐字节断言（§12）。现已三边齐备：C# 33 条、C++ 27 条、F# 5 条 union
   （§12.5）；C++ 少的 6 条是 nullable，它没有该 converter。

---

## 1. wire 契约（必须先钉死，写进文档）

从三边实现里反推出来的格式：

|项|规则|
|:---|:---|
|长度前缀|`len <= 127` → 1 字节（高位 0）；否则 4 字节大端，最高位（bit 31）置 1 标记，剩余 31 位存长度；范围 `0 ~ i32::MAX`|
|前缀宽度（exact 路径）|宽度由**预测长度**决定（`<=127` → 1，否则 4），写入实际长度但**不改宽度**|
|前缀宽度（`write_prefixed` 路径）|先占 4 字节；`actual <= 16` 且尾部余量够 `(-actual) & 7` 字节时压缩为 1 字节，否则**保留 4 字节**（§2、§12）|
|前缀宽度（C# string，已知例外）|C# 用上界 `3 * chars + 3` 定宽度 → ≥42 字符的 ASCII 串是 4 字节头；Rust 与 C++ 用精确字节数（§12.3）|
|定长类型|原样写出，默认小端|
|变长类型|长度前缀 + payload|
|named object|每个成员写 `prefix(name) name prefix(value) value`；**顺序不属于契约**（Rust / C++ 声明序，C# 名字序只为输出稳定），解码按名字查表（§19.4）|
|named 解码|按名字查表；未知名字跳过（前向兼容）；重名报错；缺非 optional 成员报错|
|tuple object|成员拼接、无名字、**无自身前缀**；`encode` 除最后一个外 auto、最后一个 raw；`encode_auto` **所有**成员 auto；`LEN = Σ LEN(member)`（任一成员为 `0` 则 `0`），与同成员元组 wire 相同|
|enum（`#[binary(int)]`）|判别式按底层整数写出（小端）；默认 `i32` → 4 字节，`#[repr(u8)]` → 1 字节；`LEN = size_of(repr)`|
|union / variant|tag（紧凑整数，编码与长度前缀同构：C# `Converter.Encode(ref allocator, index)` → `NumberModule.EncodeLength`，C++ `Encode(Allocator&, size_t)` → `EncodeLengthPrefix`，≤127 一字节，否则 4 字节带 bit31）+ payload（同 tuple object 规则，auto 位置无自身前缀，§16）|
|`Option<T>`|1 字节 tag（0=None / 1=Some）+ value；**auto 位置无前缀**（§12.6），named 成员由框架加前缀|

### 不做 wire 版本号，也不做版本协商

前向兼容只靠两条**结构性质**，不靠数字：

1. named object 解码时**跳过未知名字**（上表）；
2. 新增成员**必须是可选的**（`Option<T>`，缺省即 `None`）。加必选成员等于让旧数据无法解码。

所以本库没有「数据版本太新」这一类失败——旧解码器不认识的成员跳过即可，它认得的成员语义
不变。代价是**不能改已存在成员的语义**：要改只能新增成员、在应用层同时接受新旧两个。这与
C#/C++ 的做法一致，也是三边能互解的前提。

这条原则也约束文档示例：`Error` 的示例错误一度叫 `TooNew`（"unsupported version 9"），那是
把「版本号协商」这个本库**没有**的机制写进了样板，已于 2026-11-07 全部换成与版本无关的领域
约束（`OutOfRange`：计数越界），并同步到 `tests/errors.rs`（`Dense { count, payload }`）。
示例是用户读到的第一份契约，措辞比内部命名更容易误导。

要分清两件事：`TooNew` / `Versioned` 作为**示例**不恰当（它们描述的是本库不做的机制），
但 `#[non_exhaustive]` 本身与版本无关、也不因此失效——它管的是错误 **enum 的变体集合**能否
在不 breaking 的前提下增长（§18.8.5），不是 wire 的向前兼容（那是上面两条结构性质）。

---

## 2. 长度前缀策略

### 原则

**压缩既是省空间，也是为了让 wire 与 C#/C++ 可比**。原则上不要求逐字节相等（互解即兼容），
但凡能对齐的路径都要对齐：anchor 阈值必须与两实现相同（16，§12.4），这样组合类型才能做
逐字节 fixture 断言。唯一无法复刻的是 C# 的字符串上界宽度（§12.3），Rust 跟随 C++。
解码侧必须同时接受 1 字节与 4 字节两种头，这一点永远不变。

长度预测**不是框架职责**。`Encode` trait 不暴露 `payload_len()`，也不递归遍历值树；
长度信息只来自 converter 自己能 O(1) 拿到的东西（`Layout::LEN`、`String::len()`、
`len() * LEN`）。给不出廉价估计的组合类型直接走 anchor 兜底。

为什么不做递归精确预测：最优头长度只在**一个窄窗口**里才拿得到。1 字节头只能表示 `<= 127`，
而 anchor 路径本身已在 `actual <= 16` 时压缩（§12.4）——两个条件一夹，差异区间只剩
`17..=127`，每个对象**固定省 3 字节**：`17` 字节 payload 省 14.3%，`32` 字节省 8.3%，
`127` 字节省 `3/131 ≈ 2.3%`。也就是说**绝对收益封顶 3 字节/对象，相对收益下限 2.3%**；
`<= 16` 与 `>= 128` 两端收益都是 0，N 个对象也只是 `3N` 字节，比例不随规模变好。
而递归遍历的开销随值树规模增长（重构前实测嵌套深度 d 的叶子被调用 d 次），
为 3 字节/对象遍历整棵值树不划算。完整表格与 C#/C++ 源码依据见 §10.2。

### 前缀编码规则

|`payload 长度`|前缀|说明|
|:---|:---|:---|
|`<= 127`|1 字节|高位 0，低 7 位存长度|
|`> 127`|4 字节|大端，最高位（bit 31）置 1 标记，剩余 31 位存长度|
|`> i32::MAX`|Err|不支持|

上表是**解码器必须接受的规范形式**，也是 exact 路径写出的形式。anchor 路径在
`17 <= actual <= 127` 时写出的仍是 4 字节形式（长度值本身仍 ≤127），解码侧靠 bit 31
区分，所以两种形式都合法——**宽度不是 wire 契约的一部分，字节相等才是可选目标**（§12）。

### 三条路径

`auto` 的本质是「只读写必要的数据」：定长类型直接 raw，变长类型需要边界。但**边界由谁提供**分两种：
普通类型由 `encode_with_prefix` 给整个值加一个前缀；**tuple 由成员各自提供边界**
（`encode_auto` 把每个成员都 auto，自身不加前缀）。后者是 §11 平铺语义的关键。

`encode_with_prefix` 是 named 与一般变长值的前缀写入点，默认实现按 `LEN` 分流：

|条件|路径|长度来源|对应 C#|
|:---|:---|:---|:---|
|`LEN = n > 0`|`write_prefixed_bounded(n)`|类型常量，O(1) 精确|`EncodeWithLengthPrefixConstant`|
|`LEN = 0` + override（`String` / `[u8]` / 定长元素集合）|`write_prefixed_bounded(len)`|`self.len()` 等，O(1) 精确|`StringConverter` override|
|`LEN = 0` + 默认（变长集合 / named object）|`write_prefixed`|兜底，不预测|`EncodeWithLengthPrefixVariable`|
|**tuple / `Option` 的 `encode_auto`（任意 `LEN`）**|**成员逐个 auto，无自身前缀**|无需预测|`TupleObjectConverter.EncodeAuto` / `NullableConverter.EncodeAuto`|

### 主路径：write_prefixed_bounded（O(1) 估计）

按 `max_len` 定头宽度（`prefix_header_len`）并**只预留头**，`f` 直接往同一个 encoder 里写
（可以少于估计值），写完用 `pos` 差算出实际字节数回填进头，头宽度不变 → 零 memmove
（即使 `actual <= 127`，头宽是 4 就仍写 4 字节）。

只预留头、不预留 `h + max_len`，是刻意的：预留上界会把上界成本压给两端——owned 后端要为零填充
多扩一截，借用后端则会把「其实写得下」的调用报成 `BufferFull`。代价是越界检测挪到写完之后
（`actual > max_len` 判成调用方 bug，报 `Message` 且视为终止，不回滚），也因此不需要 C# 那种
把写入限制在窗口里的子 encoder。

与旧的 `precise_prefix` 唯一差别：旧的**要求 `f` 精确写满 n**，所以必须先递归算出 n；
新的只要给个上界。Rust 的 `String::len()` 等本来就是精确值，wire 与重构前一致。

### 兜底路径：`write_prefixed` 回填

> **术语（2026-11-08 改名）**：Rust 侧两条前缀写入原语现为
> `Encoder::write_prefixed_bounded(max_len, f)`（调用方给长度上界、头宽事前定死）与
> `Encoder::write_prefixed(f)`（无上界、写完实测长度再决定压缩）。下文历史章节里的
> 「anchor 路径 / anchor 兜底 / 默认 anchor」一律指后者；`FinishAnchor`、
> `Allocator::anchor`、`AllocatorAnchorShrinkLimits` 是 C#/C++ 的公开成员，保留原名。
> 两条路径的 **wire 并不等价**：前者按 `max_len` 定头宽后不再改变，后者按
> `actual <= 16` 压缩，所以不可互换 —— 改名正是为了让这个区别落在名字上。

给不出 O(1) 估计时，先占位 4 字节（最大可能），写完 payload 后**只做一次判断**决定是否压缩，
判据与 C# `Allocator.FinishAnchor` / C++ `Allocator::FinishAnchor` 同形：`actual <= 16`
（`PREFIX_SHRINK_LIMIT`，与 C# `Limits`、C++ `AllocatorAnchorShrinkLimits` 一致）**且**尾部
余量够 `(-actual) & 7` 字节时压缩；其余情况一律保留 4 字节头、只回填长度——`17..=127`
与「借用后端给不出余量」共用同一条兜底分支，也共用同一个强制宽度写法（§12.4）。

|结果|头宽|payload 搬移|
|:---|:---|:---|
|压缩|1 字节，`actual` 直接写进字节 0|`(actual + 7) >> 3` 段（0 / 1 / 2）定长 `u64`，最多两段 16 字节|
|不压缩|4 字节，`encode_prefix_wide` 强制宽度|无|

本节只留判据与表格、不贴实现代码：搬移的 `unsafe` 前提与 `pad` 推导在源码 doc 里，两处各
写一遍必然漂移（DESIGN 里曾写 `self.buf`，实现是 `as_mut_slice()`；兜底分支也曾各写一遍）。

`move_u64` 是一次非对齐 `u64` 读写（C# `Unsafe.ReadUnaligned<long>` /
`WriteUnaligned<long>`、C++ `LoadUnaligned<uint64_t>` / `SaveUnaligned`）：先整字读进
值再写出，所以两段重叠也安全；数值从不被解释，因此与大端无关。

**搬移形状的选择只记结论**：定长 `u64` 与 C#/C++ `Allocator::FinishAnchor` 同形，
最终抉择与实测见 §19.15.7；本节其余内容只描述 wire，不承载性能论证。

代价是尾部安全余量：两次 8 字节块会读 / 写越过 payload 末尾最多
7 字节，所以 C#/C++ 有 `bounds - offset >= ((-length) & 7)` 这条判断，余量不足就保留
4 字节头（两种宽度都是合法 wire）。Rust 同款：`Owned` 后端把 `Vec` 补零到
`offset + pad`（在 capacity 内不 realloc，且这些字节在 `pos` 之外、不进结果），
`Borrowed` 后端由调用方的 buffer 余量决定。**实际序列化应提前分配足够长的 buffer、
尽量避免 realloc**，这样常态走压缩快路径，补零也不会触发扩容。

> **状态（2026-11-08 已实施）**：`Encoder::write_prefixed` 已按 16 阈值 + 定长 `u64` 搬移
> 改写。历史版本用运行时长度 `copy_within`，会退化成通用 `memmove` 调用（§19.15.7）；
> §12.4 记录阈值决策与 `encode_prefix_wide` 的隐藏 bug。

### 设计决策

- **不需要 `PrefixPolicy` 枚举**。Rust 统一一套规则（`<=127` → 1 字节，否则 4 字节），
  `write_prefixed_bounded` / `write_prefixed` 共用；差别只在宽度由谁决定（预测值 vs 16 阈值）。
- **不暴露长度查询接口**。`payload_len()` / `auto_len()` / `named_len()` 已删除（§10），
  派生宏与自定义类型只需实现 `encode`。
- **anchor 阈值对齐 C#/C++（16），不做通用 compact**。理由是双重的：
  ① 与两实现字节一致，fixture 可以逐字节断言；② 避免 `FinishAnchor` 的 `memmove`，不重新
  引入对所有长度 compact 的通用方案。
  代价是 `17..=127` 区间多 3 字节（占该对象总长的 2.3% ~ 14.3%，即 `3/131` ~ `3/21`），
  换掉最多 127 字节的搬移。
- **唯一保留的字节差异是 C# 字符串上界**（§12.3）。C# 用 `3 * chars + 3` 定头宽，
  Rust 的 `str` 没有 O(1) 字符数，要复刻就得扫一遍——不值得，且 C++ 也走精确长度，
  所以 Rust 与 C++ 一致、与 C# 在「42..127 字符 ASCII」一段不同。
- **decode → encode 幂等**在 Rust 侧是硬约束；对其他实现写出的 payload，Rust 解码后再编码
  **允许改变前缀宽度**（这正是 §12 的差异来源），但必须仍是合法 wire。

---

## 3. 核心 trait

```rust
pub trait Layout {
    /// 定长：每个值恒为 n 字节（n > 0）；`0` = 变长（需要长度前缀）
    const LEN: usize = 0;
}

pub trait Encode: Layout {
    /// 只写 raw payload，绝不写前缀
    fn encode(&self, e: &mut Encoder<'_>) -> Result<()>;

    /// 无条件写 prefix + payload；默认按 LEN 分流，可被 O(1) 长度的类型覆盖
    fn encode_with_prefix(&self, e: &mut Encoder<'_>) -> Result<()> {
        match Self::LEN {
            0 => e.write_prefixed(|e| self.encode(e)),
            n => e.write_prefixed_bounded(n, |e| self.encode(e)),
        }
    }

    /// auto 位置：定长 raw、变长带前缀；tuple 覆盖为成员级平铺
    fn encode_auto(&self, e: &mut Encoder<'_>) -> Result<()> {
        match Self::LEN {
            0 => self.encode_with_prefix(e),
            _ => self.encode(e),
        }
    }

    /// 内部钩子（`#[doc(hidden)]`）：容器把整段 payload 交给元素；默认逐元素循环。
    /// 只有本 crate 的定宽标量覆盖它做整块写，外部类型因此永远不在整块路径上（§19.16）
    fn encode_slice(items: &[Self], e: &mut Encoder<'_>) -> Result<()>
    where
        Self: Sized,
    { /* 默认实现：逐元素 encode_auto */ }
}

pub trait Decode<'de>: Layout + Sized {
    fn decode(d: &mut Decoder<'de>) -> Result<Self>;

    /// auto 位置：tuple 覆盖为「所有成员 decode_auto」
    fn decode_auto(d: &mut Decoder<'de>) -> Result<Self> {
        match Self::LEN {
            0 => d.with_prefix(|d| Self::decode(d)),
            _ => Self::decode(d),
        }
    }

    /// 内部钩子（`#[doc(hidden)]`）：与 `encode_slice` 对称，默认逐元素 `decode`（§19.16）
    fn decode_slice(d: &mut Decoder<'de>, count: usize) -> Result<Vec<Self>>
    where
        Self: Sized,
    { /* 默认实现：逐元素 decode */ }
}
```

对照关系：

|C# / C++|Rust|
|:---|:---|
|`Length()`（0 = 变长）|`Layout::LEN`（0 = 变长），编译期常量|
|`Encode`|`Encode::encode`|
|`EncodeAuto`|`Encode::encode_auto`（有默认实现，**tuple 必须 override**）|
|`EncodeWithLengthPrefix`|`Encode::encode_with_prefix`（可 override）|
|`Decode` / `DecodeAuto` / `DecodeWithLengthPrefix`|`Decode::decode` / `Decode::decode_auto` / `Decoder::with_prefix`|

`EncodeAuto` 在绝大多数类型上不需要单独实现（默认按 `LEN` 分流），
但 **tuple 必须 override**：C# `ContextMethodsOfTupleObject::GetEncodeDelegateAsTupleObject`
与 C++ `TupleConverter::EncodeAuto`（`EncodeInternal<1>`）都让 `EncodeAuto` 把所有成员
（含最后一个）走 auto，
且不调用基类的加前缀逻辑。少了这一层，嵌套 tuple 就会多一个外层前缀，
破坏平铺等价。详见 §11。

### 分类语义：`Layout::LEN` ↔ `Converter.Length`

`Converter.Length` 的本意是描述「这个 converter 处理的是定长还是变长类型」，
Rust 用 trait 关联常量表达，并且**照抄同一个哨兵**：

||C# `Converter<T>.Length` / C++ `IConverter::Length()`|`Layout::LEN`|
|:---|:---|:---|
|粒度|类型级（构造时传入的 `readonly int`）|类型级 `const`|
|变长|`0`|`0`|
|定长|`n > 0`|`n > 0`|
|单位|raw payload 字节数，**不含前缀**|同左|

2026-11-07 之前这里写的是 `Option<usize>`（`None` = 变长），理由是把判别式显式化。
改成裸 `usize` 是因为那条理由被两条更硬的理由推翻了：

1. **`Some(0)` 不是理论上的空洞，是能跑出来的。** `[T; 0]` 走 `array_len::<T, 0>()`
   必然得到 `Some(0)`——恰好是 trait 文档里标成 invalid 的那个值。`encode_auto` 的
   `debug_assert!(n > 0)` 在 debug 构建直接 panic（已实测）；release 下更糟：0 字节不带前缀，
   `Vec<[i32; 0]>` 的元素个数彻底丢失。换成 `usize` 后这个状态不存在，`[T; 0]` 自然退化为
   `0`，`debug_assert` 也一并删掉——不是更好防，而是不用防。
2. **三边共用同一个哨兵，少一层翻译。** 读 C#/C++ 的 `if (length != 0)` 和读 Rust 的
   `match LEN { 0 => …, n => … }` 是同一件事；`None` 只是多做一次映射，没有多表达信息。

代价是 `LEN = 0` 不像 `FIXED = None` 那样自解释，所以常量名从 `FIXED` 改成 `LEN`
（对齐 C# `Length` / C++ `Length()`），文档注释写明 `0` 的含义。分类只由 `LEN` 决定，
`encode_with_prefix` 走精确路径还是 anchor 是另一件事，两者不要混。

`LEN` 是关联 `const` 而不是运行时字段：编译期可组合
（`array_len::<T, N>()`、`match (A::LEN, B::LEN)`），
C++ 的 `GetConverterLength` 是运行时循环。分类抽到独立的 `Layout` trait，
让 `Encode` 与 `Decode<'de>` 共享同一份判定，不会两边不一致。

### 零字节类型归为变长

不存在「定长 0 字节」这种分类：若 `()` 真按定长 0 字节处理，`Vec<()>` 的 payload
就是 0 字节，`remaining / 0` 无法推断元素个数，解码会静默丢掉元素个数
（payload 非空时还会死循环）。

处理方式：**`LEN = 0`，即变长**，让默认 `encode_with_prefix` 走 anchor，
写出一个 `0x00` 零长度前缀；容器因此能按前缀推进、恢复元素个数。
回归测试：`test_vec_of_unit_preserves_count`、`test_zero_length_array_is_variable`、
`test_tuple_with_zst_member_stays_variable`。

`0` 同时是「变长」和「`()` 的真实长度」，两者不需要区分——这正是裸 `usize` 比 `Option`
顺手的地方：`array_len` 里 `N == 0` 与 `T::LEN == 0` 归一到同一个返回值，没有第二种
「零」要处理。

位置派发不是游标操作：走 raw 还是带前缀由**值的类型**决定（定长 raw、变长
`prefix + raw`、tuple 平铺），所以入口是 trait 方法，接收者是被编码的值：`value.encode_auto(&mut e)`。
游标只认字节。C# / C++ 同形：`Converter<T>.EncodeAuto(ref Allocator, item)` /
`Converter<T>::EncodeAuto(Allocator&, item)` 都是 converter 的方法、allocator 作参数，
没有 `Allocator::EncodeAuto`。

### 前缀归属（编码时必须钉死的边界）

**前缀由 `encode_auto` / `encode_with_prefix` 决定，`encode` 和 `decode` 都只处理 raw payload。**

|层|写入 / 读取内容|
|:---|:---|
|`Encode::encode_auto`|定长 raw；一般变长 `prefix + raw`；**tuple 平铺、无自身前缀**|
|`Encode::encode_with_prefix`|`prefix + encode(raw)`，**无条件前缀**|
|`Encode::encode`|**仅 raw payload**，绝不写前缀|
|`Decode::decode_auto`|`encode_auto` 的解码对称面；tuple 覆盖为逐成员 `decode_auto`|
|`Decode::decode`|**仅 raw payload**，绝不读前缀|

`write_named_member(e, name, value)` 直接调 `value.encode_with_prefix(e)` —— named object 的
成员 value 无条件带前缀（对应 C# `EncodeWithLengthPrefix`），所以**不会**误用 tuple 的
平铺 override。因为 `encode_auto` 是统一入口，tuple 的 override 在嵌套每一层自动生效，
容器无需配合。

`Decoder::with_prefix` 读前缀后会构造一个 `buf` 恰好等于 payload 长度的子 decoder，
所以 `decode` 里读 "全部剩余字节" 就是精确边界：

```rust
impl<'de> Decode<'de> for String {
    fn decode(d: &mut Decoder<'de>) -> Result<Self> {
        let bytes = d.take(d.remaining())?;   // 不是 read_prefix()
        String::from_utf8(bytes.to_vec()).map_err(|_| Error::message_static("invalid UTF-8"))
    }
}
```

首版实现里 `String` / `Vec<T>` / `&str` / `&[u8]` 的 `decode` 误调了 `read_prefix()`，
等于读了两次前缀：5 字节 payload `[1,2,3,4,5]` 被解成 `[2]`，1000 字节字符串被截断。
11 个测试同时失败且症状一致，定位后统一改为 `take(remaining())`。**这条边界不要再用错。**

`read_prefix()` 是公开 API（对应 C# `DecodeWithLengthPrefix`），供手动构造 wire 之类的场景使用，标准类型的 `decode` 实现不应该调用它。

---

## 4. Encoder / Decoder

两边的结构是一一对应的，Rust 侧没有引入新概念：

|角色|C#|C++|Rust|
|:---|:---|:---|:---|
|写缓冲 + 锚点回填|`Allocator`（默认自增长；传入 `Span<byte>` 则先写调用方内存）|`Allocator`（默认自增长；传入 `std::span<std::byte>` 同上）|`Encoder<'a>`（`Backing::Owned` / `Borrowed`）|
|可推进的读切片|`ref ReadOnlySpan<byte>`|`std::span<const std::byte>&`|`Decoder<'de>`|
|取视图|`Allocator.AsSpan()`|`Allocator::AsSpan()`|`Encoder::as_bytes()`|
|物化结果（自有）|`Allocator.ToArray()`（拷一份 `byte[]`）|`Allocator::Invoke<std::vector<std::byte>>(action)`（静态模板，内部建 `Allocator`）|`Encoder::finish() -> Result<Vec<u8>>`|

「物化」两边形状不同，这一行最容易记错：C# 与 Rust 是对**已有** allocator 取值（`ToArray()` 复制出
`byte[]`、`finish()` 交出 `Vec`），而 C++ 的 `Allocator` 没有 `Take()` / `ToArray()` 这类方法 ——
只有 `AsSpan()` 取视图（README 的编码示例就是 `const auto bytes = allocator.AsSpan();`，视图有效期
跟着 allocator），要自有容器只能走静态模板 `Allocator::Invoke<T>(action)`：内部构造 `Allocator`、
跑完 action 后按 `allocator.Length()` `resize` 容器再返回，`T` 默认 `std::vector<std::byte>`
（要求元素 1 字节、可平凡复制，容器有 `data()` / `resize()`）。C# 侧的一次性版本是
`Allocator.Invoke<T>(data, action)`（内部用池化 allocator，末尾 `ToArray()`）；C++ 的
`binary::experimental::Encode(item)` 则封装了上面那条 `Invoke` 路径。

### Encoder

```rust
pub struct Encoder<'a> { inner: Inner<'a>, pos: usize }

enum Inner<'a> {
    Owned(Vec<u8>),          // 自增长，对应 C# Allocator 默认构造
    Borrowed(&'a mut [u8]),  // 写调用方内存，对应 C# Allocator(span)
}
```

单类型 + 内部枚举，而不是 `Encoder<S: Sink>` 泛型：

- `Encoder<'a>` 出现在每个 `Encode` / `Decode` 签名里，加 `S: Sink` 参数会把类型参数传染给
  所有 impl 并成倍放大单态化；而且「`Encode` 要能 `dyn`」这条路本来就不通（`Layout::LEN`
  让 `Encode` 不是 dyn-compatible，见 §7），没有为它保留泛型的理由；
- 泛型会把整条编码路径单态化 N 份，Rust 编译时间代价很实在；
- 增长是冷路径，一次分支可以接受（C# 的 `Ensure` 也是每次判断）。

关键 API：

```rust
impl<'a> Encoder<'a> {
    pub fn new() -> Self;                       // Owned，容量种子 256
    pub fn with_capacity(n: usize) -> Self;     // 容量预算 = 调用方给的上界（超出仍增长）
    pub fn on_slice(buf: &'a mut [u8]) -> Self; // 借用，越界即 Err

    pub fn write(&mut self, src: &[u8]) -> Result<()>;
    pub fn write_byte(&mut self, b: u8) -> Result<()>;
    pub fn as_bytes(&self) -> &[u8];
    pub fn len(&self) -> usize;                 // 已写字节数；借用后端只有它和 as_bytes
    pub fn finish(self) -> Result<Vec<u8>>;

    /// 无条件 prefix + payload；按 O(1) 估计预留头，回填实际长度，零 memmove
    pub fn write_prefixed_bounded(
        &mut self,
        max_len: usize,
        f: impl FnOnce(&mut Encoder<'_>) -> Result<()>,
    ) -> Result<()>;

    /// 兜底：占位 4 字节 → 写 payload → 仅在 actual <= 16 且尾部余量够时压缩
    /// （两次定长 u64 搬移），否则保留 4 字节头（与 C# / C++ Allocator::FinishAnchor
    /// 一致，见 §12）
    pub fn write_prefixed(
        &mut self,
        f: impl FnOnce(&mut Encoder<'_>) -> Result<()>,
    ) -> Result<()>;

}
```

编码侧**不再有 `with_prefix`**（旧的 `with_prefix(Option<usize>, f)` 精确路径已删除）；
解码侧 `Decoder::with_prefix` 保留。`write_prefixed_bounded` 用 `Encoder::on_slice` 把写入限制在
恰好 `max_len` 字节内，越界返回 `Error::BufferFull`，对应 C# `ThrowInvalidReturnValue`。

`Encoder` / `Decoder` 上没有任何协议层方法：位置派发在 `Encode` / `Decode` 上
（`value.encode_auto(&mut e)`），成员读写是 `named.rs` 的两个自由函数，游标只认字节。
`named` 移出与 `auto` 删除的经过见 §5。

**命名约定：改游标的一律动词开头。** 写侧 `write` / `write_byte` / `write_compact_int`，
读侧 `take` / `take_byte` / `read_prefix` / `read_compact_int` / `peek_byte`；名词只剩
`len` / `remaining` / `as_bytes` 三个**不动游标**的访问器。2026-11-09 把 `compact_int` 改成
`write_compact_int` / `read_compact_int`，就是因为裸名词分不清「读一个属性」还是「推进游标」——
`d.compact_int()` 与 `d.remaining()` 在调用点上长得一模一样，而前者会消费字节。

#### 公开接口逐条审计（2026-11-07）

判定标准只有两条：**有没有调用者**（整仓 grep，含测试与 bench）、**该不该由 `Encoder` 承担**。
`pub` 的成本是永久承诺，为测试便利而导出不算理由（§12.4 的 `PREFIX_SHRINK_LIMIT` 同理）。

|接口|调用者|结论|
|:---|:---|:---|
|`new` / `with_capacity`|全部调用点 / 仅 `new()`|留：预分配是真实需求|
|`on_slice`|`write_prefixed_bounded`、bench、测试|留：借用后端唯一入口|
|`write` / `write_byte` / `write_compact_int`|容器、派生代码、`option`|留|
|`auto` / `named`|派生代码、容器|**改**：`named` 移成自由函数（11-08）、`auto` 删除（11-09）——两者都不是游标操作，见 §5|
|`write_prefixed_bounded` / `write_prefixed`|`codec.rs` 默认实现、容器|留：两条带前缀路径|
|`finish` / `as_bytes`|全部调用点 / 2 处测试|留：`on_slice` 没有 `finish`，`as_bytes` 是它的唯一出口|
|`len`|`errors.rs`、bench（借用后端）|留|
|`position`|**0**|**删**：与 `len` 是同一个 `self.pos`|
|`is_empty`|**0**|**删**：`len() == 0`，只为满足 clippy|
|`impl Default`|**0**|**删**：唯一用途是 `mem::take` 复用缓冲，而「不复用」已定（见下），故 `new_without_default` 用 `#[allow]` 静音|

`len` 上加了 `#[allow(clippy::len_without_is_empty)]`：`Encoder` 不是容器，「还没写过东西」
不值得一个谓词，为此再养一个访问器才是被 lint 牵着走。

`Decoder` 侧做过同样的清点，结论更狠——**`len` / `is_empty` / `position` 三个全删**，只留
`remaining()`。`len`/`is_empty` 报的是**输入缓冲**大小（借用模型的细节）；`position()` 是同一
细节加个偏移，而且它**本身不自洽**：交给 `Decode` impl 的 decoder 是 scoped 的、从 0 开始，
所以同一个名字在顶层是绝对值、在嵌套里是作用域相对值。能解释绝对值的人必然持有原始 buffer
（`Decoder::new(&buf)`），一行 `buf.len() - d.remaining()` 就还原；不能解释的人只想要相对距离
（`start - d.remaining()`）。C# 和 C++ 根本没有 decoder 对象，把 cursor 放在 span 里
（`ref ReadOnlySpan<byte>` / `std::span`），"我在哪" 天生就是 `total - remaining`。
`Decode` 的形状三边不同，差别不在「有没有 span」，而在**消费量是否可观测**：

||子解码入口|边界从哪来|解码器读了多少|
|:---|:---|:---|:---|
|C#|`Converter<T>::Decode(in ReadOnlySpan<byte>)`|参数自带的 `span.Length`|**不可观测**（`in`，不能再赋值推进）|
|C++|`Converter<T>::Decode(std::span<const std::byte>)`|参数自带的 `span.size()`|**不可观测**（按值，推进只影响副本）|
|Rust|`Decode::decode(&mut Decoder<'de>)`|`Decoder.buf`，由 `with_prefix` 收窄|`pos`，**可观测**|

C# 与 C++ 的 `Decode` 是纯函数：进去一段字节、出来一个值，「读了几字节」对调用方不可见，
所以报错只能说到「这段内容不对」。`DecodeWithLengthPrefix(ref span)` 两侧同名、同形——在
调用方一侧完成「读前缀 → 推进父 span → 切出 payload → 交给 `Decode`」，作用域就是那个切片，
不需要额外类型。

Rust 侧 `&'de [u8]` 同样是 span（上一段已说明 `&mut &'de [u8]` 也写得出来），所以这不是
语言能力差异。差别是选型：边界被编码进 `Decoder.buf`、游标跟着一起走，于是 `with_prefix`
失败时能把子坐标折回父坐标（`self.pos = start + sub.pos`）——**这一行 C# / C++ 写不出来，
它们没有可回写的游标**。可观测的代价就是那十几行作用域管理代码。

反过来，Rust 的定长路径**不切边界**：C++ `DecodeAuto` 要 `span = span.subspan(length)` 再交给
`Decode`，Rust 直接 `Self::decode(d)`，靠 `LEN` 契约自觉。`with_prefix` 只服务变长值——
只有那里边界无从推导。


**「不复用」是决策，不是遗漏**（2026-11-07）：allocator 与 C# `Allocator`、C++
`Allocator(span)` 同形——一次性的轻量栈上对象，只追加不删除。`finish(self)` 吃掉 `Vec` 是
刻意的；要复用就自己拿 `on_slice` 写进已分配的缓冲。因此不加 `clear()` / `reset()` /
`take_bytes()`，`impl Default` 也随之失去最后一条理由。

`write_compact_int` 的 `debug_assert!` 换成真检查：`v > i32::MAX` 时 `v | 0x8000_0000` 把 bit 31 当
数据写出去，解码端又把这个 bit 掩掉，值会**静默改变**。现在返回 `Error::LengthTooLarge` 且
不写任何字节（`test_compact_int_beyond_i32_max_is_rejected`）。

`Encoder` / `Decoder` 都加了手写 `Debug`（`src/hex.rs` 提供 32 字节上限的十六进制预览）：
`Encoder` 打 backend / capacity / len / bytes，`Decoder` 打 `buf_len` / `pos` / `remaining` /
`next`。派生 `Debug` 会把 `Vec` 的保留尾巴一起倒出来，既是噪声又是错的（未初始化区不属于结果）。

另加两个一次性辅助函数（`src/oneshot.rs`）：`to_vec(&T)` = `new + Encode::encode + finish`，
`from_slice::<T>(&[u8])` = `Decode::decode` + **拒绝尾随字节**。**两者都是 raw 位置**：
C# `GeneratorExtensions::Encode<T>` → `Converter<T>::Encode(item)`（`EncodeConstant` /
`EncodeVariable` 都在新 allocator 上调 raw `Encode(ref allocator, item)`），
`GeneratorExtensions::Decode<T>` → `Converter<T>::Decode(byte[])` → `Decode(in ReadOnlySpan<byte>)`，
把整段交给 raw 解码；
C++ `Converter<T>::Encode(Allocator&, item)` / `Converter<T>::Decode(span)` 同样是 raw，
`EncodeAuto` / `DecodeAuto`（以及 Rust 的 `encode_auto` / `decode_auto`）只用于成员位置。
顶层值独占整个 buffer，再加前缀既冗余、字节也与另外两边不同 —— 初版误用 `auto`，
2026-11-08 按三边源码订正。变长类型的 raw 解码按定义读满整段，所以「尾随字节」只对
定长类型可见；`from_slice` 要求整段消耗完，想从大缓冲里读一个值就用 `Decoder`
（`remaining()` 在那里是可见的）。`tests/derive.rs:test_csharp_record_wire` 并列钉住了这
条区别：C# `Generator.Encode` 是 17 字节 raw，`Converter<Person>::EncodeAuto` 才多出
`80 00 00 11` 宽头。


### Decoder

```rust
pub struct Decoder<'a> { buf: &'a [u8], pos: usize }

impl<'a> Decoder<'a> {
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]>;
    pub fn read_prefix(&mut self) -> Result<&'a [u8]>; // 读前缀并切出 payload
    pub fn with_prefix<T>(&mut self, f: impl FnOnce(&mut Decoder<'a>) -> Result<T>) -> Result<T>;
    pub fn remaining(&self) -> usize;
}
```

`take` 返回 `&'a [u8]` 而不是绑定到 `&mut self` 的借用，这样零拷贝解码才能成立
（`&'a str`、`&'a [u8]` 成员）。`remaining()` 是**唯一**的长度/位置查询：失败后它仍然有意义
（`take` 失败不动游标，`with_prefix` 把子游标折回父坐标），拿 `buf.len() - remaining()` 就是
失败字节的绝对偏移；而 `len`/`is_empty`/`position` 都不提供，理由见 §4 `Encoder` 段末尾。

### `Decoder` 是必须的吗

严格说不是：`fn decode(r: &mut &'de [u8]) -> Result<Self>` 一样能写，零拷贝也照样成立
（`r.split_at(n)` 拿到的两段都还是 `&'de [u8]`）。它值得存在，是因为三件事：

1. **它是唯一的扩展点**。以后要加长度预算、`&'de [T]` 的按元素宽度校验、解码值的来源校验，
   只需要往 `Decoder` 上加字段和方法；换成 `&mut &'de [u8]`，加一个参数就是破坏所有 impl
   的签名变更。这和 serde 要一个 `Deserializer` 对象、而不是裸 `&[u8]`，是同一个道理。
2. **调用点更好读**。`d.take(4)` / `d.read_prefix()` / `d.read_compact_int()` 是方法调用，
   `*r = &r[n..]` 是手工切片，在二十多个 impl 里会累积成噪声。
3. **`with_prefix` / `read_prefix` 需要显式游标**，报错时才有位置可指。

代价也是真的：`decoder.rs` 141 行，`Decoder<'de>` 出现在 6 个源文件与派生代码里，共 39 处
引用。真要砍掉它是机械改动而非设计重构——但会失去第 1 条，所以当前保留。

---

## 5. derive 宏

```rust
#[derive(Encode, Decode)]
struct Person {
    id: i32,
    #[binary(rename = "Name")]
    name: String,
    #[binary(optional)]
    age: u8,          // 缺失时取 Default；写成 Option<T> 会在值上再套一层 tag
    #[binary(skip)]
    cache: Cell<u32>,
}
```

|属性|作用|对应 C#/C++|
|:---|:---|:---|
|默认|named object|`NamedObjectConverter`|
|`#[binary(tuple)]`|tuple object（平铺：`encode` 最后 raw，`encode_auto` 全成员 auto，见 §11）|`TupleObjectConverter`|
|`#[binary(rename = "...")]`|改 wire 名字|`NamedMember{"Name", ...}`|
|`#[binary(optional)]`|允许缺失，缺失取 `Default`|`IsOptional = true`|
|`#[binary(skip)]`|不参与编码|—|
|`#[binary(with = "path")]`|成员转换器：模块声明 `Repr` + `to` / `from`，wire 形状取 `Repr`（§19.17）|`FixedConverterPolicy`|
|借用成员|`&'de str` / `&'de [u8]` 零拷贝；由成员类型表达，**没有**属性（§19.17）|`SpanLike` 一族|
|`#[binary(int)]`|枚举按底层整数；枚举必须二选一（`int` 或 `union`），否则编译报错|`LittleEndianEnumConverter`|
|`#[binary(union)]`|枚举按 tag + auto value|`UnionConverter` / `VariantConverter`|

### C# `record` 怎么落到 Rust

C# `record Person(int Id, string Name)` 在 wire 上是 **named object**——「位置」只影响构造
函数，不影响布局。所以对应物是具名结构体，不是元组结构体：

```rust
#[derive(Debug, PartialEq, Eq, Encode, Decode)]
struct Person {
    #[binary(rename = "Id")]
    id: i32,
    #[binary(rename = "Name")]
    name: String,
}
```

- **不能**写 `struct Person(i32, String)`：derive 明确拒绝 tuple struct（phase 1），而
  `#[binary(tuple)]` 是另一种布局（无名、无成员前缀）。C# 的 `[TupleObject]` 才对应后者。
- record 自带的相等 / `ToString` / `with`：Rust 侧是 `derive(PartialEq, Eq, Debug)` 与结构体
  更新语法 `Person { id: 7, ..old }`，都不参与 wire。
- 逐字节已验证（`tests/derive.rs::test_csharp_record_wire`）：`(42, "Bob")` →
  raw `024964 042a000000 044e616d65 03426f62`（17 字节）/ auto = `80000011` + 上述。
  该例的声明序恰好等于 C# 的名字序（`"Id" < "Name"`），所以两边 hex 相同；顺序本身不是
  契约，只是 C# 为了输出稳定才排序（§19.4）。
- 代价是**命名**：C# 属性 PascalCase、Rust 字段 snake_case，每个字段一行 `rename`；漏一个不会
  在编译期报错，只会在解码侧变成 `MissingMember`。容器级 `#[binary(rename_all = "PascalCase")]`
  （serde 同名属性）能把 n 行压成 1 行，**尚未实现**，见下面的实现状态表。

### 实现状态（2026-11-07）

`crates/binary-derive` 已实现第二版：**零依赖**（只用编译器给的 `proc_macro`，不引 `syn`/`quote`），
自带 token 展平 + 递归下降解析，生成 `Layout`/`Encode`（`#[derive(Encode)]`）与
`Decode`（`#[derive(Decode)]`）。范围：**无泛型的具名结构体**（named / tuple）+ **C 类枚举** + **标记联合**。

|属性|状态|说明|
|:---|:---|:---|
|默认（named object）|✅|成员名默认取字段名，`write_named_member` 写无条件前缀|
|`rename`|✅|支持任意 UTF-8（含转义、raw string）；不再产出非法字节串|
|`optional`|✅|编码侧 `!= Default::default()`，解码侧缺失取 `Default`|
|`skip`|✅|不参与编码，解码取 `Default`|
|`tuple`（结构体级）|✅|无名字、无自身前缀，与同成员元组 wire 一致（§14.1）|
|`int`（枚举级）|✅|判别式按 `repr` 宽度写出，默认 `i32`（§14.2）|
|`union`（枚举级）|✅|tag（紧凑整数）+ payload 按 tuple object 规则；auto 位置无自身前缀（§16）|
|`with`|✅|模块声明 `Repr` + `to` / `from`；`Repr` 的 `Layout` 决定 wire，字段类型无需实现 `Layout`（§19.17）|
|`borrow`|✅|单个生命周期参数绑到 `Decode<'de>`，借用由成员类型表达；`&'de [T]` 不做（§19.17）|
|`rename_all`（容器级）|❌|未实现；C# record 的 PascalCase 属性名要求 n 个字段写 n 行 `rename`|

已验证：`crates/binary-tests/tests/derive.rs` 的 25 个用例 + `fixtures/csharp.tsv` 的
6 条 derive 相关 fixture（named、tuple object ×2、enum ×2、`int?` 成员）。
**2026-11-07 之前前 5 个用例没有接线**——测试入口文件缺 `mod derive;`，等于宏从未被编译过。

已修（原「已知缺陷」1–3、5，详见 §14.3 / §15）：

1. ~~结构体级属性被静默丢弃~~ → 现在解析并校验，未知选项直接 `compile_error`。
2. ~~重复 wire name 无检查~~ → 解析期报
   ``duplicate wire name "id" on field `b`; `a` already uses it``，caret 落在重复的字段上。
   报错文案不再泄漏内部表示（以前是 `b"x"`）。
3. ~~非 ASCII `rename` 生成非法字节串~~ → 反转义成字节后 `Literal::byte_string`。
5. ~~错误 span 全部指向 derive 名~~ → token 现在携带 `Span`，`Error` 带定位：
   字段级错误指向字段名、`#[binary(nope)]` 指向选项本身、`union` 指向关键字、
   tuple struct 指向括号、空结构体指向花括号。两个 derive 各产出一条但 span 相同，
   rustc 会去重，所以用户只看到一条。

仍然存在：

4. **`optional` 隐含要求 `PartialEq`**（编码侧要和 `Default` 比较）。不是 bug，
   但属于未声明的 bound；字段类型没有 `PartialEq` 时得到指向 derive 的 E0369。
6. **`expected ,` 分支几乎不可达**：真少一个逗号时 rustc 的 parser 先报语法错，
   宏拿到的 token 流已经被嚼掉一段类型，只会生成缺字段的初始化式并被 E0063 兜住。
   这条路径不值得写用例（会钉住 rustc 的措辞）。

负向测试：`crates/binary-tests/tests/ui/` 的 **27 个 compile-fail fixture + `.stderr` 快照**，
由 `trybuild` 驱动（**仅 dev-dependency**，`cargo tree -e normal` 仍然只有
`binary → binary-derive`）。快照同时钉住三件事：文案、caret 位置、以及「每个错误只报一次」。

校验能力差距：C# 的 source generator 有 **31 条专用诊断**（Key 重复、TupleKey 不连续、
required 成员缺 Key、多重属性冲突…），Rust 现有 **27 条已钉住的错误路径**：
重复 wire name / 未知结构体级选项 / 冲突的布局模式 / 未知字段选项 / `rename` 非字面量 /
tuple 带成员属性 / 枚举缺 `int` / `int` 用在结构体 / `tuple` 用在枚举 / Rust `union` 类型 /
tuple struct / unit struct / 泛型结构体 / 泛型枚举 / 空结构体 / 空枚举 / 带字段的变体 /
非整数 `repr` / `union` 用在结构体 / `union` 变体上的 `repr` / `union` 显式判别式 /
`union` 成员属性 / 枚举变体上的 `#[binary(...)]`。

### named object 的 wire 与算法

每个成员写 **两段带前缀的字节**，顺序即声明顺序：

```text
[prefix(name)] [name bytes] [prefix(value)] [value payload]   ← 成员 0
[prefix(name)] [name bytes] [prefix(value)] [value payload]   ← 成员 1
...
```

关键：**name 和 value 都无条件带前缀**，用 `encode_with_prefix` 而不是 `auto`。
C# `ContextMethodsOfNamedObject` 调的是 `EncodeWithLengthPrefix`，
C++ `NamedObjectConverter::EncodeInternal` 也是 `EncodeWithLengthPrefix`；
即使 value 是 `i32` 这种定长类型也照样带前缀（与 tuple 的 `encode_auto` 平铺行为不同）。
`prefix(name) + name` 与成员名一样是编译期常量，可以预计算成 `&'static [u8]`，
编码时整块 `write`（C# 的 `headers[i]` 预计算就是这个思路）。

解码（对应 C++ `NamedObjectLayout::Parse` / C# `NamedObjectDecoder.Invoke`）：

1. 循环：读前缀 → 得到 name slice；再读前缀 → 得到 value slice。
2. `find_member(name)` 查索引，未知名字直接 `continue`（前向兼容，跳过整个成员）。
3. 已知但该槽位已填 → 重名报错。
4. 记录 value 的 `(offset, length)` 到 slots 数组（C# 用 `long` 打包 `offset << 32 | length`）。
5. 循环结束后遍历 slots：非 optional 且未出现 → `MissingMember` 报错。
6. 最后按索引对每个 slot 调 `T::decode` —— **raw，不带前缀**，因为 value 前缀已在第 1 步消费。
   这与 §3「前缀归属」的边界一致，不要写成 `decode_auto`。

`optional` 成员在编码侧是「值不为 `Default` 才写」（C# `ObjectModule.NotDefaultValue`），
解码侧缺失时取 `Default`；`skip` 成员完全不参与。

### `Layout::LEN` 的归属

- **named object 恒为 `0`（变长）**。C++ `NamedObjectConverter` 没有向 `Converter<T>` 基类传
  length（默认 0 = 变长）；而且即使所有成员定长，总字节也包含每成员的 name 与两个前缀，
  不等于 `sum(LEN)`。
- 「所有成员定长则取 `sum`」只适用于 **tuple object**（成员拼接、除最后一个外 auto，
  最后一个 raw，无 name 无额外前缀），对应 C++ `TupleObjectMember::Length()` 的累加。

### core 侧已落地的 API（2026-11-07）

派生代码要写的东西现在由 core 直接提供，derive 宏只需生成调用：

|API|作用|
|:---|:---|
|`Encode::encode_auto` / `Decode::decode_auto`|位置派发的唯一入口：定长 raw、变长 `prefix + raw`、tuple 平铺；派生代码用限定路径调用|
|`write_named_member(e, name, value)`|写一个成员，直接调 `value.encode_with_prefix(e)`，**无条件前缀**；成员写入的**唯一**公开入口|
|`read_named_member(d)`|读一个成员，返回 `(name, raw payload)`；值前缀在这里消费掉，`write_named_member` 的读侧对偶|
|`NamedMembers::<N>::parse(d, find)`|循环 `read_named_member(d)`；未知跳过、重名报错、按索引记录 value 的 raw slice|
|`NamedMembers::value(i, name)` / `optional(i)`|按索引 raw decode；缺失分别报 `MissingMember(name)` / 取 `Default`|

成员读写是 `named.rs` 的两个自由函数，不在 `Encoder` / `Decoder` 上：成员不是游标操作，
挂上去像是在给 `named object` 开特权（`Vec`、tuple 都没有对应方法）。名字也直说 `member`，
不会被读成「读写整个对象」；半操作随之消失 —— 名字与值在同一个函数里连写，
「有名字没值」表达不出来。

对象层只有读侧有类型（`NamedMembers`），**这是格式决定的，不是写侧漏了**：写是流式的，
一个 `Encoder` 连续写几个成员就是整个对象；读必须先缓冲全部成员才能按名取字段
（成员顺序不是 wire 契约），这份缓冲就是 `NamedMembers`。非对称因此只留在对象层，
成员层由 `write_named_member` / `read_named_member` 这一对抹平。

`NamedMembers` 是表中唯一**必须**留在 crate 根 `pub` 的类型，不是为测试便利导出：
`#[derive(Decode)]` 展开在调用方 crate，生成代码直接引用
`::binary::NamedMembers::<N>::parse(__d, ...)`；把重导出降成 `pub(crate)` 会让所有
named struct 的派生解码报 E0603（实测另有三处手写调用同样失败）。同时它也是
**不用 derive、手写 `Decode`** 时唯一的 named 解码入口 —— `read_named_member` 只把一对前缀
读成 `(name, payload)`，跳过未知名、查重、按索引保存 raw payload 这套语义仍只在这里。
`parse` / `get` / `require` / `value` / `optional` 五个方法各有真实调用者，没有可删的冗余。
若只想收窄文档可见面，可给重导出加
`#[doc(hidden)]`，但 semver 承诺不变。

半操作不存在了：`write_named_member` 里 `write_prefixed_bytes(name)` 与 `encode_with_prefix`
连着写，没有「只写名字」的公开或私有入口。生成代码目前不预计算名字头；若将来实测值得，
再加 `write_named_member_precomputed(&'static [u8], &T)`，而不是拆出半操作。

`named_len` 已随 §10 重构删除，派生宏**不需要生成任何长度计算**，只生成 `encode`。
`find` 就是生成代码里的 `match name { b"id" => Some(0), ... }`。`Error` 新增
`DuplicateMember`；`MissingMember(&'static str)` 与 `InvalidMemberIndex` 现在都真正被用到。
`crates/binary-tests/tests/named.rs` 用手写的 `Person` 模拟派生结果，14 个用例覆盖
精确 wire 字节、乱序、未知成员跳过、重名、必填缺失、optional 默认值、4 字节前缀、
截断错误、decode→encode 幂等与 anchor 兜底。

### 为什么成员读写是自由函数

第一版把成员写入做成 `impl Encoder { fn named(..) }`，论证的是「**可行**」：模块边界不是层
边界，固有 `impl` 写在 `named.rs` 与写在 `encoder.rs` 调用点完全一致（std 就把 `impl File`
放在平台模块里）。但「可以」不等于「应该」—— 名字 `named` 读起来像「读写整个 named 对象」，
实际只写**一个成员**；把它挂在 `Encoder` 上，等于让字节游标认识一种对象形状，而 `Vec`、
tuple 都没有这种待遇。

所以归属和名字一起改：`write_named_member` / `read_named_member` 是 `named.rs` 里的自由
函数，`Encoder` / `Decoder` 退回纯字节游标。

|方案|买到|付掉|
|:---|:---|:---|
|`NamedObject<'_>` 状态 writer（serde `SerializeStruct` 风格）|强制「每个 name 跟一个 value」|对象边界是**外层长度前缀**，writer 本身没有状态、`finish()` 是空的；每个派生 `encode` 多一个 binding 加一个生命周期|
|扩展 trait `NamedExt`|`Encoder` 的表面只剩字节操作|手写 impl 多一个 `use`，生成代码走 UFCS —— 为观感付接口税|
|固有方法 `Encoder::named`（第一版）|调用点最短|名字误导成「整个对象」；`Encoder` 与 `Encode` 耦合；`write_name` 这个半操作被迫活在一个 `impl` 里|
|自由函数（现状）|协议与游标分离、名字直说 `member`、半操作消失、与 C++ `::binary::EncodeWithLengthPrefix` 同形|两个 crate 根名字；调用点从 `e.named(..)` 变成 `write_named_member(&mut e, ..)`|

**半操作随固有 `impl` 一起消失了**：自由函数里两行连着写，不再需要一个「只写名字」的私有
方法，「有名字没值」这种非法状态连半条路径都没有。

### 为什么 `auto` 也没有留在游标上

`Encoder::auto` / `Decoder::auto` 曾是游标上的两个转发方法，各一行（`v.encode_auto(self)` /
`T::decode_auto(self)`）。删除的理由与 `named` 同源，只是更彻底：**位置由值决定，不由游标决定**。
一个类型同时「认识字节」和「认识怎么给值加前缀」，是把两种生命周期不同的职责绑在一起；
游标退成纯字节之后，`Encode` / `Decode` 才是解码策略的唯一持有者。

|方案|买到|付掉|
|:---|:---|:---|
|游标上的转发方法（旧）|调用点短一个 `&mut`；不必 `use Encode`|同一件事两种写法；游标与 codec 耦合；逐元素热路径多一跳|
|直接调 trait 方法（现状）|只有一种写法；与 C# / C++ 的 converter 方法同形；派生代码走限定路径|调用点要 `use Encode` / `use Decode`（手写 `impl` 本来就要）|

**代价不是理论上的。**§19.16 记过一次实测：`vec<bool>` 的逐元素循环写 `e.auto(item)` 时
2150 ns，改成 `item.encode_auto(e)` 后 958 ns —— 转发层没被内联，每个元素多一次调用加一次
`Result`。删掉这层包装后不存在「不小心用了慢写法」的路径；`collections.rs` 的
`encode_items` / `encode_pairs` 正是当时漏掉的同类循环，这次一并改直。

读侧同理：`d.auto::<T>()` 把「按哪种策略解码」藏在游标上，`T::decode_auto(d)` 直接说明
这是类型的解码策略；带推断的类型写 `Decode::decode_auto(&mut d)`，与 `T::decode_auto(d)` 等价
（前者由期望类型定 `Self`）。

### 为什么读侧是 `read_named_member` + `NamedMembers`

写侧一个 `write_named_member` 就够，是因为写是流式的：成员边界就是外层长度前缀，writer 不
需要状态（上面那张表已否掉 `NamedObject`）。读侧不能这么办：成员顺序不是 wire 契约，字段
只能等全部成员读完再按名取，所以必须有 `NamedMembers` 这份缓冲。

原先只有 `NamedMembers::parse` 读「两个前缀」，游标类型完全不认识成员 —— 于是写侧知道协议
的一半、读侧一点都不知道，看上去像漏了一半。现在读侧分两层：`read_named_member` 只做
「读一对前缀，返回 `(name, raw payload)`」，是 `write_named_member` 的逐字节逆操作；
`NamedMembers::parse` 退成策略层（跳过未知名、查重、按索引存 raw）。结果是**成员层成对、
对象层仍非对称**，而对象层的非对称来自格式本身（写流式、读必须缓冲），不是为了对称造一个
空转的 writer。

### 名字查找

直接生成 `match`：

```rust
let __m = NamedMembers::<2>::parse(d, |name| match name {
    b"id" => Some(0usize),
    b"Name" => Some(1usize),
    _ => None,
})?;
```

`&[u8]` 上的 `match` 不是「长度 + `memcmp` 链」：LLVM 把它编成**长度 switch → 首字节 switch →
剩余字节逐个比较**，短名字 2~3 条 `cmp`，既没有 `bcmp` 调用也没有跳转表（反汇编见 §19.18）。

曾经担心的对手是 C++ 的 packed 2×u64（`longdata` 方案）。**2026-11-09 已量（§19.18）：`match` 在
现实 schema 上比它快 1.3~1.8×，而且表内存是 0 字节**（表方案要 64~2048 B），所以 derive 继续只生成
`match`，不预计算任何查找表、也不引 `const fn` 建表。

分派树的深度只跟名字形状有关，跟成员数关系很小：16 个 6 字节等长名字（长度 switch 失效、
`match` 的最坏形状）是 3.49 ns，仍比第二名的 6.24 ns 快；16 → 32 成员是 3.79 → 3.99，没有拐点。
换表只在「压根没有 `match` 可用」时才有意义，而这个库的 derive 永远能给 `match`。

---

## 6. 性能要点（按优先级排序）

**先基准后修改**：性能改动一律先跑出基线，再改，再用同一套 harness、同一口径复跑一遍；没有数据
不改代码，也不用「应该会更快」的推断代替读数。三边对比只比各语言的最优构建，比值与绝对值
分开看（§19.15.5 / §19.15.9）。下面按优先级排列。

1. **O(1) 长度估计优先**。`LEN` 常量或 `len()` 能给出估计时走 `write_prefixed_bounded`，
   一次预留、回填实际长度，不 `memmove`；给不出就 anchor 兜底。
   不为省 3 字节去递归遍历值树（§10）。
2. **容器 memcpy 快路径（2026-11-08 已实现，encode 侧）**。C++ 有两道判断
   （`converters/ContainerConverter.hpp`）：`Copyable`（编译期：`sizeof(E) == 1`，或小端 + trivially
   copyable）与 `copy`（运行时 `dynamic_cast<LittleEndianConverter<E>*>`，确认元素 converter 就是
   规范 LE 那个，而不是用户替换过的自定义实现）。

   原先照抄成 `pub unsafe trait Pod` + `impl<T: Pod> Encode for Vec<T>`，但第二个 impl 与已有的
   `impl<T: Encode> Encode for Vec<T>` **重叠**（E0119，stable 无 specialization）。后来一度退成
   `Layout::POD: bool` 关联常量 + 容器内分支 —— 能工作，但那个常量是**公开可写、编译期不强制**
   的 unsafe 契约：外部 crate 一句安全代码 `const POD: bool = true`，解码侧就会把不可信字节
   memcpy 成值（§19.10 代价 2）。2026-11-08 把它收回内部，改为**元素钩子**：

   ```rust
   pub trait Layout { const LEN: usize = 0; }              // 不再有 POD

   // 内部钩子（`#[doc(hidden)]`），默认实现就是逐元素循环
   fn Encode::encode_slice(items: &[Self], e: &mut Encoder<'_>) -> Result<()>;
   fn Decode::decode_slice(d: &mut Decoder<'de>, count: usize) -> Result<Vec<Self>>;
   ```

   `Vec<T>` / `[T; N]` / `VecDeque<T>` 只写 `T::encode_slice(...)` / `T::decode_slice(...)`，
   不再判断任何标志；整块语义只存在于 `primitives.rs` 的定宽标量（`size_of == 1` 或小端目标），
   `unsafe` 全部收进私有 `src/pod.rs` 的两个函数。**外部类型没有安全途径**打开整块路径：要打开
   只能自己写 `unsafe`，UB 就落在它自己的块里，不再是本 crate 的 unsoundness。详见 §19.16。

   不覆盖钩子的类型（`bool` / `char` / derive 出的 `#[binary(tuple)]` / `Box` 等指针）一律逐元素。
   `bool` 写出去本来没问题（内存里只可能是 `0` / `1`），但整块读要求「任意字节都是合法值」，
   `02` 不是合法 `bool`，`char` 还有 Unicode scalar 值域校验；两者解码仍宽容
   （`b != 0` / `char::from_u32`），与 C# / C++ 的读行为一致。

   **C++ 的解码侧用的是同一个 `Copyable` 标志**：`ContainerConverter::Decode` 里
   `std::span<const E>(reinterpret_cast<const E*>(span.data()), capacity)` 再构造容器。
   因为 `sizeof(bool) == 1` 也满足 `Copyable`，C++ 连 `bool` 一起整块拷 —— 读进 `02` 在 C++
   里同样是 UB，只是主流编译器表现得像 `true`。Rust 不能照抄，这正是 `bool` 留在逐元素路径的
   原因。C++ 的第二道判断（运行时 `dynamic_cast`）在 Rust 里不存在：单态化下每个类型只有一个
   `Encode` impl，没有「用户换掉了元素 converter」这回事，所以只留编译期那一半。

   实测（`binary-bench wire`，同一会话三次；`LoopI32` 是不覆盖钩子的 4 字节同构类型）：

   |用例|bulk|逐元素|
   |:---|---:|---:|
   |encode 1024 × i32（4100 B）|34.3 – 36.3 ns|141.8 – 146.1 ns|
   |decode 1024 × i32（改前 1215 – 1440 ns）|**44.8 – 47.8 ns**|—|

   解码 **26–31×**，是本项目目前最大的单项收益。**已定型为方案 A**（2026-11-08，§19.10）：
   不为 `bool` 单独保留编码侧快路径。大量 `bool` 集合在实际应用中很难出现，真有密集布尔
   需求应当用 **bitset**；为了让一个罕见场景拆出第二个钩子、维护两份白名单、并给公开面再
   加一条契约，不划算。`Vec<bool>` 逐元素是明确接受的代价。完整实测与备选方案的否决理由见
   §19.10，开关收回内部的经过与代价见 §19.16。

3. **解码预分配**。定长元素：`Vec::with_capacity(n / LEN)` + `set_len` + `copy_nonoverlapping`，
   不要 `push` 循环。对应 C++ 的 `GetCapacity(byteLength, itemLength, type)`；§6 第 2 条
   已把它落地在标量的 `decode_slice` 钩子，默认钩子（外部类型）仍是 `with_capacity` + `push`。
   **§19.15.4 补测**：变长元素（`T::LEN == 0`）连 `with_capacity` 都没有——`Vec<String>` 走
   `Vec::new()` + `push`，64 个元素实测 **69 次分配 / 1.6 µs**；换 `Vec<&str>` 零拷贝是 351 ns。
   **§19.15.14 已量**：扫描计数在 64 元素上要 88.7 ns，正好抵掉省下的 4 次 realloc
   （净慢 1.2%）；免扫描的上界预留快 4.5% 但多占 3.5× 内存。结论是**不做预分配** —— 变长
   元素的解码成本在 64 次 `String` 分配上，`Vec<&str>` 零拷贝快 3.6×，杠杆在借用解码。
   编码侧相反：payload 已在内存里、长度可预知，容量预算由调用方在**构造期**给，见 §19.15.15。
4. **错误默认不装箱**。`enum Error` 是 11 个变体的闭合枚举：静态消息（`message_static`）走
   `Cow::Borrowed` 零分配，只有 `custom(impl Display)` 才分配一次字符串。默认路径不碰
   `Box<dyn Error>`——那会拿走 `PartialEq` / `Eq` / `Clone`，而这三样是「能断言到具体变体」
   的前提（§18.2）。需要保留原始错误类型时才显式选 `wrapped`（`Arc` + 文本相等，§18.7），
   装箱因此是**可选**代价而不是默认定价。`Error` 仍是 24 字节（`Wrapped` 载荷 16 < `Cow<str>`
   的 24）。
5. 不要接 `std::io::Read/Write`。C#/C++ 都没接，接进来只会让泛型和单态化变复杂。
6. **owned 后端不给未写区域清零**（§19.15.17）：`src.len() >= 256` 的批量写入走
   `Vec::extend_from_slice`（追加到未初始化余量），不再用 `Vec::resize` 先清零再覆盖。
   4 KiB 写回 69.2 → 40.1 ns、1 MiB 28.8 → 18.6 µs，端到端 64 KiB `String` 1375 → 927 ns；
   改完 owned 只剩一次 malloc + free 的固定成本。C++ / C# 本来就是 `bounds` / `offset` 分离，
   只有 Rust 之前是两遍写。
7. **不再有递归长度预测**（§10 已实施）。`test_no_recursive_length_prediction` 用计数器验证：
   `Vec<Counting>` 与 `Vec<Vec<Counting>>` 中每个叶子只 `encode` 一次（各 2 次调用），
   且 wire 与重构前逐字节相同（`[6,2,1,2,2,1,2]` / `[7,6,2,1,2,2,1,2]`）。
   代价是组合类型走 `write_prefixed` 兜底、带一次 ≤16 字节定长搬移；收益是去掉
   O(d × N) 的重复遍历和公开接口。
   按 **先基准后修改** 的规矩：当时尚无 harness，前后性能对比不做无数据推断；
   **订正**：当晚已建 `crates/binary-bench`（§19），但本次重构前后专门对比仍未补测。

---

## 7. 明确放弃的 C# 机制

|C# 机制|处理|
|:---|:---|
|`Token` / `DynamicObject` 反射树|放弃。Rust 没有反射，也没有等价需求|
|`IConverterCreator` / `GeneratorBuilder` / fallback 反射工厂|放弃。trait impl + derive 覆盖|
|`Generator`（`TypeId` → `IConverter` 注册表）|v1 不做。真需要动态时用 `Value` 枚举 + `TypeId` 注册表（注意 `dyn Encode` 不存在，见本节末）；实在要注册表再加薄薄一层 `TypeId -> Arc<dyn AnyConverter>`|
|`IsolatedConverter` / `IVariableConverterFunctions`|换成包装类型 `With<C, T>`|
|`SpanLike` adapter（ArraySegment / Memory / ...）|放弃，`&[u8]` / `Vec<u8>` / `Cow` 够了|
|`Converter.Stream` 系列|v1 不做，需要时加 `Read`/`Write` 适配层|
|.NET 专有类型：`decimal` / `Guid` / `DateTime` / `DateTimeOffset` / `TimeSpan` / `DateOnly` / `TimeOnly` / `BigInteger` / `BitArray` / `IPAddress` / `IPEndPoint` / `Version` / `Uri` / `Half` / `System.Numerics` 全家桶|**不追求 wire 对齐**。C++ 侧也没有这些类型，跨语言本来用不上；Rust 的等价类型（`SystemTime`/`Duration`/`IpAddr`/`Uuid` 等）语义也不同。需要时由使用方 crate 自己 impl `Encode`/`Decode`（`Binary.Experimental` 也不算参考实现）|

动态层 v1 不做。这里要记清楚**为什么「只加一个 object-safe 的编码 trait」不够**——早期草稿
里真加过 `EncodeDyn`（`encode_dyn` → `e.auto(self)`，blanket impl），结果是死代码，0.1 前删掉：

1. `Encode` 本身**不是** dyn-compatible：`Layout::LEN` 是关联常量，`&dyn Encode` 直接
   E0038（错误指向 `codec.rs` 的 `const LEN`）。去掉 `Layout` 的 `EncodeDyn` 能绕开这一条，
   但绕开之后它就只是一个「能写字节的 `Any`」；
2. 没有 glue：`Vec<Box<dyn EncodeDyn>>` 不满足 `Vec<T: Encode>`，`Box<dyn EncodeDyn>` 也没有
   `Layout`/`Encode` impl，所以 DESIGN 早期那句「这样 `Vec<Box<dyn EncodeDyn>>` 都能直接编」
   从来没成立过（实测 `E0277: the trait bound Box<dyn EncodeDyn>: Encode is not satisfied`）；
3. 就算补上 glue，写出去的字节**没有类型标签**，读端无法切分：这是个只能写、不能读的死胡同。

所以真要做动态层，至少得同时给三样东西：类型标识（`TypeId` 注册表或 `Value` 枚举）、
`Box<dyn _>` 的 `Layout`/`Encode` glue、以及解码侧的注册表查找。缺一样都不该发布一个
看起来能用、实际用不了的 trait。

---

## 8. crate 结构

crate 划分与逐文件职责只写一处：见 §9.1–§9.4 的模块与测试表（同样的清单留两份，必然漂移）。

**命令只跑一次、数字只记一处**：下文各章不再复述 `cargo test` / `clippy` / `fmt` 这类命令，
凡写「已通过」即当时按 §20.1 的矩阵整轮跑过，计数只在 §20.1 维护。本节只定义「什么算对齐」。

benchmark 变体从 4 个扩到 5 个；第 6 个（Rust dyn）做不出来，理由见下表最后一行：

|变体|说明|
|:---|:---|
|C# JIT / C# AOT|现有（`Benchmarks.IntegrationTests`，含 source-generated AOT）|
|C++ virtual / C++ template|现有|
|**Rust static**|全单态化，对应 C++ template|
|**Rust dyn**|❌ 未做：`Encode` 不是 dyn-compatible（`Layout::LEN`），没有 vtbl 版本可测，C++ virtual 那一列在 Rust 侧暂无对应|

对齐判据（缺一不可）：

1. **wire fixture 通过（分桶断言，见 §12.5）**：C# 语料在 `crates/binary-tests/fixtures/csharp.tsv`，
   - `same` 桶：**断言 Rust encode 的字节完全相同**（当前 31 条）；
   - `csharp-only` 桶（string `42..=127` 字符）：断言 payload 逐字节相同、
     `reencode(csharp_wire) == rust_wire`（当前 2 条）；
   - 反向：Rust encode 的结果 C# 必须能正确 decode（尚未自动化，靠 `same` 桶的字节相等间接保证）。
   C++ 语料在 `crates/binary-tests/fixtures/cxx.tsv`（27 条，**全部逐字节相等**，没有桶列）：
   `binary-cxx` 修掉容器头宽后与 Rust 同宽（§12.5）；缺的 6 条 nullable 由
   `test_cxx_covers_every_expressible_case` 显式声明，不静默跳过。
   F# 语料在 `crates/binary-tests/fixtures/fsharp.tsv`（5 条 union，**全部逐字节相等**）。
2. **decode → encode 幂等**（Rust 自身）：Rust decode 出的值再 encode，字节稳定。这是精确长度
   预测路径的保证。C#/C++ 因容量依赖做不到，不影响 Rust 侧这一条。
3. benchmark 的互解列与 C# 一致，性能列同表比较。anchor 阈值对齐后（§12.4）wire hash 列
   除 string 用例外应当相同；差异必须能归因到已知例外，否则视为 bug。

---

## 9. 实现状态（2026-11-07 起，更新至 2026-11-09；当前总表见 §20）

`crates/` 的划分与逐文件职责只写在这一节。**验证数字、质量门与剩余项只在 §20 维护**，本节不重复；
§9.1–§9.4 是「已经有什么」，§9.5 起是当时确认的语义、wire 边界与已关闭的待办。

### 9.1 `binary`：core（`no_std + alloc` 已验证）

|模块|职责|
|:---|:---|
|`lib.rs`|`no_std` 入口（`extern crate alloc`）、模块声明与 re-export；feature 只有 `std` 与 `derive`（§17.5）|
|`codec.rs`|三个 trait：`Layout` / `Encode`（`encode` + `encode_with_prefix` + `encode_auto` + `encode_slice` 内部钩子）/ `Decode<'de>`（`decode` + `decode_auto` + `decode_slice`）|
|`length.rs`|前缀编解码 `encode_prefix` / `encode_prefix_wide` / `decode_prefix` / `prefix_header_len`；全部 `Result` 化，无静默截断（§18）|
|`encoder.rs`|`Encoder<'a>`、`write_prefixed_bounded`、`write_prefixed`（16 阈值）、`PREFIX_SHRINK_LIMIT`（`pub(crate)`，不导出，§12.4）、`write_compact_int`（union tag 原语，§16）|
|`decoder.rs`|`Decoder<'a>`、零拷贝 `take`、`with_prefix`、`read_compact_int`；`remaining()` 是唯一长度查询|
|`primitives.rs`|`u8`–`u128` / `i8`–`i128` / `f32` / `f64` / `bool`（读宽容 `b != 0`）/ `char`（4 字节）/ `usize` / `isize`|
|`container.rs`|`String` / `str` / `Vec<T>` / `&[u8]` / `[T; N]`；整段 payload 委托 `T::encode_slice` / `T::decode_slice`（§6 第 2 条、§19.16）；O(1) `encode_with_prefix`；`Tuple` 1–12 由 `macro_rules!` 平铺（§11）|
|`option.rs`|`Option<T>` = tag + value，auto 位置**无前缀**（§12.6）|
|`smart_ptr.rs`|`Box` / `Rc` / `Arc`：wire 透明，解码不保留共享身份（§19.13）|
|`collections.rs`|`VecDeque` / `LinkedList` / `BinaryHeap` / `BTreeMap` / `BTreeSet` / `Cow`（`alloc`，无条件）+ `HashMap` / `HashSet`（`std`，hasher 泛型）；raw payload = `Vec<T>`，委托 `Vec::decode`（§19.14）|
|`named.rs`|`write_named_member` / `read_named_member` / `NamedMembers<N>`；成员无条件带前缀（§5）|
|`pod.rs`|私有 `write_bulk` / `read_bulk`：全 crate 唯一两处把 `&[T]` 当 `&[u8]` 重解释（§19.16）|
|`error.rs`|闭合 `Error`（11 变体，含 `DuplicateMember` / `MissingMember` / `InvalidMemberIndex` / `InvalidTag`）；`Message` 持 `Cow<'static, str>`、`Wrapped` 持 `ErasedError`（`Arc<dyn core::error::Error + Send + Sync>`），静态消息零分配；两条降级路径见 §18.4 / §18.7|
|`hex.rs`|`Debug` 用的 32 字节 hex 预览（私有）|
|`oneshot.rs`|`to_vec` / `from_slice`：raw 位置的入口，每次分配、不复用缓冲、不接受尾随字节（§4）|

### 9.2 `binary-derive`

|能力|写法|实现要点|
|:---|:---|:---|
|named object|默认（无属性）|`rename` / `optional` / `skip`；成员一律走 `encode_with_prefix`，定长 value 也带前缀。容器级 `rename_all` 未实现（§5）|
|tuple object|`#[binary(tuple)]`|字段按声明序平铺，auto 位置无自身前缀（§11）|
|C 类枚举|`#[binary(int)]`|判别值按 C# 的整数语义写（§14）；`#[repr(...)]` 是编译错误，tag 永远紧凑整数（§16）|
|标记联合|`#[binary(union)]`|tag 为紧凑整数，payload 按 tuple object 规则（§16）|
|成员级转换器|`#[binary(with = "m")]`|只做类型替换（`m::Repr` + `to` / `from`），wire 语义从 `Repr` 继承（§19.17）|
|借用成员|成员类型 `&'de str` / `&'de [u8]`|derive 只放开「恰好一个生命周期参数」，不设属性（§19.17）|

诊断与负向用例见 §15（27 条 compile-fail fixture + `.stderr` 快照）；剩余缺口见 §5「实现状态」。

### 9.3 `binary-tests`：测试与语料

|测试文件|条数|覆盖|
|:---|---:|:---|
|`tests/wire.rs`|50|基础 wire / anchor / `compact_int` / 整块与逐元素对照 / 端序契约 / tuple / oneshot / `Debug` / `write_prefixed*`|
|`tests/derive.rs`|25|派生宏：union 8 + with 5 + C# record 1 等分组|
|`tests/errors.rs`|15|自定义错误降级、`Cow` 形态、外层包装、位置折算、`Wrapped` 保留类型、变体语义（§18）|
|`tests/named.rs`|14|手写 named 实现交叉验证|
|`tests/smart_ptr.rs`|12|`Box` / `Rc` / `Arc` wire 透明、`?Sized`、递归 struct / union / tuple（§19.13）|
|`tests/collections.rs`|11|容器与 `Vec<T>` / `Vec<(K, V)>` 逐字节相等、自定义 hasher、`Cow`、错位 payload（§19.14）|
|`tests/borrow.rs`|8|借用成员零拷贝、解码指针落在输入缓冲内（§19.17）|
|`tests/cxx_fixtures.rs`|2|27 条 C++ fixture 逐字节 + 覆盖集断言（§12.5）|
|`tests/fixtures.rs`|1|33 条 C# fixture 逐字节（§12.5）|
|`tests/fsharp_fixtures.rs`|1|5 条 F# union fixture 逐字节（§12.5）|
|`tests/buffer_contents.rs`|1|owned 后端水位线：33 个 fixture case 编码后整段比对（§19.15.18）|
|`tests/ui.rs`|1|驱动 27 个 compile-fail（trybuild，§15；`#[cfg(not(miri))]` 跳过）|

合计 141 条集成用例。共享件在 `tests/common/mod.rs`（`round_trip` / `with_header` / `split_prefixed` /
`from_hex` …）与 `tests/common/cases.rs`（三张表共用的 name → 类型 → 值）。`binary` 自己另有
`tests/length.rs` 11 条（原 `src/length.rs` 的内联 `mod tests`，已外移），`src/` 里不再有
`#[cfg(test)] mod tests`。

|语料|条数|来源与状态|
|:---|---:|:---|
|`fixtures/csharp.tsv`|33|真实 C# 输出，全部逐字节相等|
|`fixtures/cxx.tsv`|27|真实 C++ 输出，比 C# 少 6 条 nullable（C++ 没有 nullable converter，由覆盖集断言显式声明）|
|`fixtures/fsharp.tsv`|5|F# union 输出，每条四边同 tag|

### 9.4 benchmark 与脚本

|项|内容|
|:---|:---|
|`crates/binary-bench`|手写 harness、零依赖、`publish = false`；六组：`wire`（三边同 payload，只留 C# / C++ 也测的四行，打 hex + ns/op）、`objects`、`collections`、`errors`、`profile`、`lookup`，见 §19|
|`scripts/run-benchmark.sh`|构建并运行上面六组之一，默认 `wire`；`lookup` 另接受 `full` 出完整矩阵|
|`scripts/test-multiarch.sh`|七个 target 的四象限交叉，每个 target 自己的 qemu-user runner + sysroot，见 §20.1|
|`scripts/check-miri.sh`|nightly + Miri 的 UB 检查（host + s390x 抽象机），见 §19.12|
|`scripts/check-shell.sh`|`bash -n` + `shfmt -d` + `shellcheck` 三个门（只扫 `scripts/`），见 §20.1|

### 9.5 实现中确认的语义（对照 C# / C++ 源码）

- 集合元素用 `auto` 而非 raw `encode`：`EnumerableEncoder` 与 C++ `ContainerConverter`
  都是 `EncodeAuto` / `DecodeAuto`。定长元素 `auto` 折叠成 raw，无额外前缀。
- `Option<T>` = `tag + value`，**`encode_auto` / `decode_auto` 不加前缀**，与
  `NullableConverter` 的 override 一致（§12.6）。前缀只在框架要求时出现
  （named 成员），由默认 `encode_with_prefix` 提供。
- 同一个理由适用于 union：C# `UnionConverter` override 了 `EncodeAuto`，
  否则会落到基类 `Converter<T>::EncodeWithLengthPrefixVariable` 的 anchor 路径而多一个前缀。
  派生代码因此 override `encode_auto` / `decode_auto`（§16）。
- **named object 的成员是例外：用 `encode_with_prefix`（无条件前缀），不是 `auto`**。
  C# `ContextMethodsOfNamedObject` 与 C++ `NamedObjectConverter` 都用
  `EncodeWithLengthPrefix`，定长 value 也带前缀。详见 §5。
- **派生宏生成的 wire 与手写实现一致**（`test_derive_wire_matches_handwritten_layout`），
  外层前缀宽度已按 §12.4 改成 16 阈值，与 C#/C++ 逐字节相等。
- **长度预测不暴露、不递归**（§10 已实施）：`String` / `[u8]` / 定长元素集合 override
  `encode_with_prefix` 用 O(1) 精确长度；组合类型走默认 anchor，`auto_len` / `named_len` 已删除。
- **tuple 平铺**（§11 已实施）：`encode` 最后成员 raw，`encode_auto` / `decode_auto` 全成员 auto
  且不加自身前缀；`((a,b),c)` 与 `(a,b,c)` wire 相同，可跨类型互解。

### 9.6 wire 边界（当时的结论）

质量门与用例数见 §20.1，这里只留三条与字节有关的结论：

- `decode → encode` 幂等（`test_decode_encode_idempotent` / `test_named_decode_encode_idempotent`）、
  anchor 兜底路径（`Unpredictable` / `Opaque` 无 O(1) 估计）、每个值只编码一次
  （`test_no_recursive_length_prediction`）都已验证；`test_tuple_flatten_equals_nested` 用 C#/C++
  README 的 25 字节例子验证嵌套与平铺等价、且两种类型能互解对方的 payload。
- §11 的 tuple 平铺**改变了**嵌套 tuple 的 wire（去掉多余外层前缀），使其与 C# / C++ 一致；
  非 tuple 的 wire 不受影响。§12.4 的 anchor 阈值同理：`payload <= 16` 的用例不变，
  `17..=127` 的 anchor 用例变成 4 字节头（`test_named_wire_exact`、
  `test_derive_wire_matches_handwritten_layout`），与 C# 一致。其余 wire 断言逐字节保持。
- named object 的逐字节布局被 `test_named_wire_exact` 钉死（与真实 C# 输出一致）：

```text
80 00 00 11 | 02 "id" | 04 2A 00 00 00 | 04 "name" | 03 "Bob"
 ^外层前缀(17>16 → 4 字节)   ^name  ^value(定长也带前缀)  ^name   ^value
```

唯一的第三方依赖是 dev-dependency `trybuild 1.x`（冷启动约 2 MB 源码 / ~9 s 构建，离线机需先
`cargo fetch`）；`cargo build` / `no_std` / 发布都不受影响。

### 9.7 历史待办（全部结案）

以下是 2026-11-07～11-09 开的待办，现全部关闭或转成口径问题；§20.3 当前为空。

|曾列待办|结论|依据|
|:---|:---|:---|
|`union` 派生|✅ 已完成：tag 紧凑整数，payload 按 tuple object，auto 位置无自身前缀|§16|
|八个标准容器|✅ `Box` / `Rc` / `Arc` + `VecDeque` / `LinkedList` / `BinaryHeap` / `BTreeMap` / `BTreeSet` / `Cow` / `HashMap` / `HashSet` 全部落地，零新增依赖；`KeyValuePair` 复用 `(K, V)` tuple|§19.13 / §19.14|
|`with` / `borrow`|✅ 成员级转换器用 `Repr` 作契约，借用成员由成员类型表达|§19.17|
|整块读写快路径|✅ `Vec<i32>` ×1024：encode 38 vs 逐元素 82 ns，decode 44.8–47.8 vs 1215–1440 ns；能力从 `Layout::POD` 收紧成元素内部钩子，`bool` / `char` 仍逐元素|§6 第 2 条 / §19.10 / §19.16|
|benchmark harness|✅ Rust 手写 + C# JIT/AOT + C++ integration 三边同表；C# AOT 未并入同口径矩阵、C++ union 场景仍缺，属口径与覆盖问题（记 §19.6）|§19|
|C++ 侧语料|✅ 27 条实测字节；顺带量出并修掉 C++ 在 17..=127 字节 payload 上写宽头的 bug（`binary-cxx` c6c11cb），重生成后全部逐字节相等|§12.5|
|错误模型是否整体擦除|❌ 否决：失败路径只多约 5.5 ns + 一次分配，但失去可 `match` 的可观测性|§18.8.7 / §19.3|
|API 审计|✅ 8 条已修、1 条保留（named 成员错误位置不回折），明细见下|—|

审计项明细（前八条已修，最后一条是有意保留）：

|审计项|结论|
|:---|:---|
|`Decoder::take(n)` 溢出|✅ 判据改为 `n > buf.len() - pos`；`take(usize::MAX)` 得 `NotEnoughBytes` 而非 panic（`test_take_past_the_end_is_not_enough_bytes`）|
|前缀编码签名|✅ `encode_prefix` / `encode_prefix_wide` 改 `Result<usize>`：`LengthTooLarge`（`len > i32::MAX`）与 `BufferFull`（1 / 4 字节都放不下）；`debug_assert!` 全删，release 不再有静默截断|
|`bool::decode` 的宽容|✅ 照抄不是疏忽：真实 C# 探针（`00 → False`，`01` / `02` / `FF → True`）与 C++ 一字节 memcpy 同表现，保留 `b != 0`，文档写明「读宽容、写规范」|
|`Encoder` / `Decoder` 的 `Debug`|✅ 手写实现 + `hex.rs` 的 32 字节预览；不用 derive（会把 `Vec` 的保留尾巴倒出来）|
|缓冲复用|✅ 不做：`finish(self)` 吃掉 `Vec` 是刻意的，`on_slice` 是唯一复用出口；`impl Default` 随之删除（§4）|
|一次性辅助函数|✅ `binary::to_vec(&T)` / `binary::from_slice::<T>(&[u8])`，raw 位置（§4）|
|`Decoder::len()` / `is_empty()` / `position()`|✅ 全删：前两个报的是输入总长，`position()` 在 scoped decoder 里是作用域相对值却名字像绝对值。`remaining()` 唯一，绝对偏移 = `buf.len() - remaining()`|
|`Pod` 空壳|✅ 删掉 `pub unsafe trait Pod`（写不出配套 `impl`，E0119），改为元素内部钩子|
|named 成员的错误位置|❗ 不回折：`NamedMembers::value` 在 detached payload 上建 decoder，成员内偏移是成员自身坐标、外层 `remaining()` 不随之前进；要绝对位置只能在成员 impl 里自己拼（`Error::message`）|

---

## 10. 已实施：移除 `payload_len()`，改为 converter 内部预测（2026-11-07）

> **状态**：**已实施并验证**。本节保留决策依据与 C#/C++ 源码考证；
> §2 / §3 / §4 / §6 / §9 已同步为新实现。
> `cargo test` 当时 45 passed / 0 failed（§11 后为 46 passed），wire 逐字节不变。

### 10.1 决策

从 `Encode` trait **移除公开的 `payload_len()` 接口**，连带删除 `auto_len()` / `named_len()`。
长度预测不再是框架职责，只保留在具体 converter 内部，且只用 O(1) 可得的信息。

### 10.2 为什么：开销与回报不成比例

递归精确预测唯一的作用是让 length prefix 用最优头长度（1 字节而不是 4 字节）。但 1 字节头
只在 `<= 127` 装得下，而 anchor 路径已在 `actual <= 16` 时压缩（§12.4），两个条件一夹，
差异区间只剩 `17..=127`，每个对象**固定省 3 字节**：

|payload|anchor 写出|精确预测写出|节省|
|:---|---:|---:|---:|
|`0..=16`|`1 + L`（已压缩）|`1 + L`|**0**|
|17|4 + 17 = 21|1 + 17 = 18|14.3%（3/21）|
|32|36|33|8.3%（3/36）|
|127|131|128|2.3%（3/131）|
|`>= 128`|`4 + L`|`4 + L`|**0**|

窗口两侧的收益都是零：`<= 16` 时 anchor 已经用 1 字节头，`>= 128` 时 1 字节头根本表示不了。
窗口内的绝对收益封顶 3 字节、相对收益下限 `3 / (4 + 127) ≈ 2.3%`；有 N 个这样的对象时总收益
`3N`，比例不会因为对象变多而改善（分母同比例变大）。而递归 `payload_len()` 的开销随值树
规模增长（重构前实测：嵌套深度 d 的叶子被调用 d 次，总代价 O(d × N)）。
为这 3 字节遍历整棵值树不划算。用户结论：**照抄 C# / C++，只在转换器内部做必要的长度预测**。

### 10.3 C# / C++ 的「内部预测」怎么做（源码依据）

C# `Allocator::AppendWithLengthPrefix`（`Allocator.Module.cs` 的 `maxLength` 重载）：

```csharp
var numberLength = NumberModule.EncodeLength((uint)maxLength);          // 按【上界】算头长度
ref var target = ref Create(ref allocator, maxLength + numberLength);   // 一次预留
var actual = writer.Invoke(span, data);                                 // 一次写入，返回实际字节数
NumberModule.Encode(ref target, (uint)actual, numberLength);            // 回填 actual，头长度不变
FinishCreate(ref allocator, actual + numberLength);
```

- **头长度按 `maxLength` 上界算，不按 `actual`**；写完只回填头内容、不改头长度 → 零 memmove。
- `maxLength` 来自 `U.Limits(item)`（`VariableConverter::EncodeAuto` / `EncodeWithLengthPrefix`）或 `span.Length`，都是 O(1)，
  **不递归遍历值树**。
- `StringConverter::EncodeAuto` / `EncodeWithLengthPrefix` override 走这条路径；
  `Allocator::AppendWithLengthPrefix`（`Allocator.String.cs` 的 string 重载）缓冲够时用
  `GetMaxByteCount`（上界），不够才 `GetByteCount`（精确），
  这正是已知的 wire 余量差异来源。
- C++ `LittleEndianStringConverter::EncodeWithLengthPrefix` 更直接：
  `Encode(allocator, size * item.size())` 用 O(1) 精确长度写前缀再写 payload；
  `Allocator::AppendWithLengthPrefix(maxLength, action)` 与 C# 同构。
- **给不出上界的复杂 converter 才走 `Anchor` + `FinishAnchor` 兜底**
  （C# `Allocator::AppendWithLengthPrefix`（`AllocatorAction<T>` 重载）、
  C++ `Converter<T>::EncodeWithLengthPrefix`）。

这也说明「length prefix 优先让 converter 自己预测精确长度，避免 `FinishAnchor` 的
memmove；`FinishAnchor` 只作兜底」这条约定指的是 **converter 自己**，
不是框架递归遍历整棵值树。重构前 §2 的设计把预测职责错放到了公开 trait 上，偏离了这条约定。

Rust 比 C# 更有条件做对：`String::len()` 就是 UTF-8 wire 字节数、`Vec<u8>::len()` 是精确长度、
定长元素集合是 `len * LEN`，全是 O(1) 精确值，连上界都不需要。

### 10.4 采用的 API 形态（可选钩子）

三个候选（可选钩子 / 方法族照抄 C# / 只留 `encode` 加一律 anchor）中，用户确认按
**可选钩子**实施。它把前缀写入点从 `Encoder` 挪到 trait 层，同时 `encode` 仍只写
raw payload，`auto` 默认由 `Layout::LEN` 统一分派。
**后续修正（§11）**：tuple 必须能 override `auto`，因此补了 `encode_auto` /
`decode_auto` 两个带默认实现的钩子；「不可 override」只对非 tuple 类型成立。
下面的代码块是**当时的形状**，现行 API 见 §3 与 §11。

```rust
pub trait Encode: Layout {
    /// 只写 raw payload，不写前缀
    fn encode(&self, e: &mut Encoder<'_>) -> Result<()>;

    /// 带前缀编码。默认走 anchor 兜底；具体 converter 可用 O(1) 长度覆盖，
    /// 走「预留头 → 一次写入 → 回填实际长度」的无 memmove 路径。
    fn encode_with_prefix(&self, e: &mut Encoder<'_>) -> Result<()> {
        e.write_prefixed(|e| self.encode(e))
    }
}

impl Encoder<'_> {
    pub fn auto<T: Encode + ?Sized>(&mut self, v: &T) -> Result<()> {
        match T::LEN {
            0 => v.encode_with_prefix(self), // 变长：由 converter 决定
            _ => v.encode(self),             // 定长：无前缀
        }
    }
}
```

派生宏与自定义类型**只需实现 `encode`**；`encode_with_prefix` 是可选的，
**没有任何长度查询接口被暴露**。`Encoder` 新增原语 `write_prefixed_bounded(max_len, f)`：
按 `max_len` 算头长度并一次预留，`f` 写入后返回实际字节数，再把实际长度回填进预留的头
（头长度不变），对应 C# 的 `AppendWithLengthPrefix(maxLength, ...)`。

分层：

|类型|路径|长度来源|
|:---|:---|:---|
|`String` / `str` / `[u8]` / `Vec<u8>`|override `encode_with_prefix`|`self.len()`，O(1) 精确|
|`Vec<T>` / `[T; N]`，`T::LEN = n`|override|`len() * n`，O(1) 精确|
|变长元素集合 / tuple / named object / `Option<T>`|默认 anchor|兜底，**不递归预测**|
|自定义 / 派生结构体|默认 anchor|兜底|

### 10.5 wire 影响（已验证）

**逐字节不变**：`test_named_wire_exact`、`test_long_prefix_boundary`、
`test_named_long_value_4byte_prefix`、零字节类型系列和
`test_no_recursive_length_prediction` 的 wire 断言全部原样通过。
anchor 兜底压缩后仍写 1 字节头，与精确路径的字节一致，只是内部从「先算后写」
变成「先写后回填」。

> ⚠️ **2026-11-07 起本条已被 §12.4 修订**：实测 C#/C++ 的 anchor 只在 `actual <= 16` 时压缩，
> 17..=127 保留 4 字节头；本节的「anchor 压缩后与精确路径字节一致」只对 `<= 16` 成立。
> 按上界算头（`EncodeLength(maxLength)`）的方案**不做**，见 §12.3。

### 10.6 执行结果

- 已删除：`Encode::payload_len()`、`auto_len()`、`named_len()`、
  `Encoder::with_prefix(Option<usize>, f)` / `precise_prefix`、各 impl 里的 `payload_len` 覆盖。
- 已新增：`Encode::encode_with_prefix`（默认按 `LEN` 分流）、
  `Encoder::write_prefixed_bounded(max_len, f)`、`write_prefixed` 改为公开。
- 已 override：`str` / `String` / `&str` / `[u8]` / `&[u8]` 用 `len()`；
  `Vec<T>` / `[T; N]` 在 `T::LEN = n` 时用 `len() * n`。
- 已改：`codec.rs`、`encoder.rs`、`container.rs`、`option.rs`、`named.rs`、`lib.rs`；
  `decoder.rs` 零改动。
- 测试：`Unpredictable` / `Opaque` 改为只实现 `encode`；`test_payload_len_recursion_count`
  改为 `test_no_recursive_length_prediction`（每叶子只 encode 一次）；
  新增 `test_write_prefixed_bounded_writes_actual_length`；编码侧 `with_prefix` 用例改用
  `write_prefixed_bounded` / `write_prefixed`。
- 文档：§0 / §2 / §3 / §4 / §5 / §6 / §9 已全部同步。
- **后续修正（§11）**：补上 tuple 的 `encode_auto` / `decode_auto` override，
  修复嵌套 tuple 多一个外层前缀、破坏平铺等价的问题。
- **未做（当时，2026-11-07 白天）**：按「先基准后修改」，彼时尚无 benchmark
  harness，不做无数据推断。**订正（同日晚 → §19）**：`crates/binary-bench` 手写 harness
  已建立，三边对照与错误模型 A/B 均已跑出数据；但「本次重构前后」的专门对比仍未补测，
  不做事后推断。

---

## 11. 已实施：tuple 平铺语义（2026-11-07）

### 11.1 约束

C# / C++ 两个仓库 README 的 tuple 语义小节都明确：

> Elements are concatenated in order: a variable length element is prefixed unless it is the
> last element, whose length is implied by the remaining bytes. Nested tuples are flattened...

即 **任意层级嵌套的 tuple 与其直接平铺的 wire 必然完全相等**，且两种类型能互解对方写出的
payload；嵌套不产生任何 header 或额外字节（C++ README 甚至指出
`std::tuple<std::tuple<std::int32_t>>` 就是 4 字节）。

### 11.2 源码依据：tuple 的 `EncodeAuto` 被 override

|方法|C# `ContextMethodsOfTupleObject::GetEncodeDelegateAsTupleObject`|C++ `TupleConverter::EncodeInternal<IsAuto>` / `TupleObjectConverter::EncodeAuto`|
|:---|:---|:---|
|`Encode`（raw）|除最后成员外 `EncodeAuto`，最后 `Encode`|`IsAuto == 0` 且是最后成员 → `Encode`，否则 `EncodeAuto`|
|`EncodeAuto`|**所有成员 `EncodeAuto`**（`auto` 为真或非最后成员）|`EncodeInternal<1>`：**所有成员 `EncodeAuto`**|

关键：`EncodeAuto` override **直接展开成员，不调用基类的「加前缀 + Encode」**。
基类 `Converter<T>::EncodeAuto` 对变长类型走
`EncodeWithLengthPrefixVariable`（anchor + 前缀），tuple 走的不是这条路。
Decode 侧 `Decode` / `DecodeAuto` 完全对称。

三个方法对 tuple 各有不同语义，不能合并：

|方法|tuple 行为|
|:---|:---|
|`encode`（C# `Encode`）|成员展开；除最后外 auto，最后 raw；**无外层前缀**|
|`encode_auto`（C# `EncodeAuto`）|成员展开；**所有成员 auto**；**无外层前缀**|
|`encode_with_prefix`（C# `EncodeWithLengthPrefix`）|**外层前缀** + `encode`（最后 raw）|

### 11.3 Rust 侧修复

§10 重构后 `Encode` 只有 `encode` + `encode_with_prefix`，`auto` 入口（当时的
`Encoder::auto`，2026-11-09 已删、改为 `value.encode_auto(e)`，见 §5）对变长类型
直接调 `encode_with_prefix` —— 对一般类型正确，对 tuple 则错：变长子 tuple 会多一个
整体前缀。修复前实测：

```text
nested (26 bytes): b0 06 ... f4 01 00 00 | 08 | 03 61 6d 79 07 00 00 00 | 6c 6f 67 69 6e
flat   (25 bytes): b0 06 ... f4 01 00 00 |     03 61 6d 79 07 00 00 00 | 6c 6f 67 69 6e
                                          ^ 多余的 0x08 外层前缀
```

修复：给 `Encode` 加 `encode_auto`、给 `Decode` 加 `decode_auto`（都带默认实现），
`auto` 入口改为调用它们；1–12 元 tuple（`macro_rules!` 生成，见 §11.5）
override 这两个方法为「所有成员 auto、不加自身前缀」，而 `encode` / `decode` 的 raw 语义保持不变。
`write_named_member` 仍用 `encode_with_prefix`，所以 named object 成员不会误用平铺 override。

这也说明 §10 里「`EncodeAuto` 不需要 trait 方法」的判断**只对非 tuple 类型成立**：
`auto` 必须由 converter 决定边界由谁提供，tuple 是必须 override 的例外。

### 11.4 wire 断言（25 字节例子）

`test_tuple_flatten_equals_nested` 用两份 README 共同的 25 字节例子：

```text
b0 06 00 00 00 00 00 00  f4 01 00 00  03 61 6d 79  07 00 00 00  6c 6f 67 69 6e
1712i64                   500i32        "amy"        7i32         "login"（最后，raw）
```

断言：嵌套 `((1712L, 500), ("amy", 7), "login")` 与平铺 `(1712L, 500, "amy", 7, "login")`
都产出这 25 字节，且两种类型都能解出对方写出的 payload。

### 11.5 元数上限与 `macro_rules!` 重写（2026-11-07）

原来 1..=5 是手写的 20 个 `impl`（约 190 行），6 元 tuple 直接编译失败：

```text
error[E0277]: the trait bound `(i32, i32, i32, i32, i32, i32): Encode` is not satisfied
```

三边对比时这成了**只有我们被卡住**的地方：C# `ValueTuple` 8 元（第 8 个塞 `TRest` 嵌套）、
`[TupleObject]` 靠反射不限；C++ `TupleConverter` 用 `TupleSize` + `index_sequence` 变参展开，
不限。Rust 既没有反射也没有变参，元数只能在源码里写死——`std` 自己也是在 12 元停下的
（13 元 tuple 的 `Debug` 是编译错误，rustc 1.99.1 实测）。注意这里说的是**元数上限**，不是
测试覆盖：C# 侧对 10 元早就有用例，见 §11.5.1。

现在由 `container.rs` 的一个 `macro_rules!` 生成 1..=12，`()` 仍是特例（`LEN = 0`，见 §5）。
三个实现细节值得记：

- **`LEN` 用 `const fn tuple_len(&[usize])` 折叠**，不再写 `match (A::LEN, B::LEN, ...)`：
  「任一成员为 0」在 5 元要 5 个 arm、12 元要 12 个，手写必然漏一个。
- **`encode` 的「除最后一个」用 muncher 展开**：`tuple_encode_raw!` 递归到最后一个成员为止，
  前面的走 `auto`、最后的走 `encode`。转录块里的内层 `{{ }}` 是必需的——`=>` 后的花括号只是
  transcriber 定界符，不会出现在展开结果里；少了它，递归臂展开成 `stmt; expr`，在表达式
  位置报 "trailing semicolon in macro used in expression position"。
- **`decode` 不能照抄**：`macro_rules!` 无法把表达式「前置」到递归结果上，所以用一个 `$val`
  累加器收集前 n-1 个 `T::decode_auto(d)?`，最后一次吐出元组字面量（元组元素从左到右求值，
  顺序仍有保证）。base arm 末尾的逗号也是必需的，否则 1 元 tuple 会退化成括号表达式。

断言：`test_wide_tuples`（6 元 `LEN = 18`；12 元 `LEN = 12`、auto 无头、逐字节等于 1..=12）
与 `test_wide_tuple_last_member_is_raw`（6 元、`String` 收尾：raw 为 `01 02 03 04 05 78`，
auto 为 `01 02 03 04 05 01 78`），wire 字节与重写前完全一致（既有 35 个 wire 用例全绿）。
`container.rs` 从 484 行降到 396 行，其中 tuple 段从 194 行降到 106 行（含注释）。
bench 复跑与重写前同量级：encode tuple raw 28.1 ns、auto 32.9 ns、decode tuple 102.6 ns。

#### 11.5.1 C# 侧本来就有 10 元覆盖

上一节说「三边对比里只有我们被卡住」，容易被读成「C#/C++ 只有短 tuple 的测试」。实际上 C#
侧早就有三条针对性用例（`Binary.SourceGeneration.TupleObjectTests/SystemTupleTypes/`）：

|用例|覆盖|
|:---|:---|
|`LargeValueTupleTests.cs`|10 元 `ValueTuple`，断言 `converter.Length == 62` + raw/auto 双向往返|
|`LargeTupleTests.cs`|10 元 `System.Tuple`（引用元组），同样 `Length == 62`，嵌套 `Tuple<long, UInt128, Int128>` 单独注册 converter|
|`EmbeddedValueTupleTests.cs`|`(int, (long, int), double, short, (ulong, float, byte), int, int)` 与等价的 10 元扁平 tuple **字节相等**，且 AOT 与 JIT 一致|

第三条正是 §11 那次修复的核心性质（嵌套 tuple 在 wire 上压平），C# 侧一直有断言；Rust 侧
对应的是 `test_tuple_flatten_equals_nested`。所以**不需要新增 C# fixture**：元数覆盖已经在
C# 自己的测试里，缺的只是「同一组值两边字节一致」这一步，本轮补上了。

#### 11.5.2 10 元 tuple 的三边字节一致（实测）

拿同一组值分别跑真 C# 库（`Generator.CreateDefault()`，`Encode` / `Converter.EncodeAuto`）
与 Rust（`encode` / `encode_auto`），`(1u8, -2i8, 3u16, -4i16, 5u32, -6i32, 7u64, -8i64,
9u128, -10i128)` 在两边都是 62 字节且逐字节相同：

```text
01 fe | 03 00 | fc ff | 05 00 00 00 | fa ff ff ff
07 00 00 00 00 00 00 00 | f8 ff ff ff ff ff ff ff
09 00 ... (16 字节)      | f6 ff ... (16 字节)
```

C# 的 `Length == 62` 与这条 hex 同时说明：`ValueTuple` 第 8 个位置的 `TRest` 嵌套
（`ValueTuple<..., ValueTuple<long, UInt128, Int128>>`）**在 wire 上没有额外前缀**，与 Rust
的 10 元 tuple 同形。`test_ten_element_tuple_matches_csharp` 把这条 hex 钉在 Rust 侧，并顺带
断言 C# 写出的字节能被 Rust 解码回同一个值。

---

## 12. 跨语言 wire 实测（2026-11-07）

> **状态（2026-11-08 更新）**：本节是 2026-11-07 的实测记录，**§12.4 的 anchor 阈值修改
> 当天即已实施**（见 §12.4 标题与 §2 的状态注）；表中「改后」指实施后的字节，不是待办。
> 此前的文档只做了源码考证，没有跑过真实 C# 输出；本节把三边的字节摆在一起，推翻了 §2 里
> 「C# `FinishAnchor` 与本实现同构」的判断。

### 12.1 方法

- C#：`Mikodev.Binary`（`net11.0`）→ `Generator.CreateDefault()` →
  `GeneratorExtensions.GetConverter<T>()` → `Converter<T>.EncodeAuto(ref Allocator)` → hex。
  用真实的库，不是复刻逻辑。
- C++：读 `Allocator.cpp` / `Converter.hpp` / `Length.hpp` 的实现常量与分支（未跑 harness）。
- Rust：同一批值走 `encode_auto`。
- 全部是 `auto` 位置（带边界），不是 raw `encode`。

### 12.2 实测表

|用例|C# `EncodeAuto`|Rust `encode_auto`（改后）|一致|
|:---|:---|:---|:---|
|`i32 42`|`2A000000`|同|✅|
|`Option<i32>` = None / Some(5)|`00` / `0105000000`|同|✅|
|`Vec<Option<i32>>` = `[None, Some(5)]`|`06 00 01 05000000`|同|✅|
|named 成员 `int?` = None / Some(5)|`01 00` / `05 01 05000000`|同|✅|
|`Vec<i32> [1,2,3]`|`0C 01000000 02000000 03000000`|同|✅|
|`Vec<i32>` 5 个（payload 20，exact 路径）|`14 …`|同|✅|
|`Vec<Vec<i32>>`（payload 10）|`0A 04 01000000 04 02000000`|同|✅|
|`Vec<String>` 4×4（payload 20）|`80 00 00 14 04 …`|同|✅|
|`String` 16 / 17 / 41×`a`|`10` / `11` / `29` + payload|同|✅|
|**`String` 42×`a`**（42 字节）|`80 00 00 2A` + 42 字节|`2A` + 42 字节|❌ 已知例外|
|**`String` 127×`a`**|`80 00 00 7F` + 127 字节|`7F` + 127 字节|❌ 已知例外|
|`String` 128×`a`|`80 00 00 80` + payload|同|✅|
|`String` 20×`中`（60 字节）|`3C` + 60 字节|同|✅|
|tuple `(1, "a")`|`01000000 0161`|同|✅|
|named `Tiny{ x: 7 }`（payload 4）|`04 0178 0107`|同|✅|
|named `Person`（payload 17）|`80 00 00 11 026964 04 2A000000 046E616D65 03426F62`|同|✅|
|named `Mid`（3×i32，payload 21）|`80 00 00 15 …`|同|✅|

上表全部由 `test_csharp_fixtures` 自动化断言（33 条，见 §12.5），不再依赖手工比对。
除最后两条 string 的**已知例外**外，anchor 路径的字节也与 C#/C++ 相同。
`Vec<i32>` 5 个走 exact 路径（`5 * 4 = 20` 可 O(1) 预测），所以宽度是 1 字节——
说明「exact 用 ≤127 规则、anchor 用 ≤16 规则」这条区分是必要的，不能统一。

解码方向全部互操作：`decode_prefix` 靠 bit 31 判断 1/4 字节，两个方向都能读。
不一致的只有**编码时选的宽度**，即字节数与 wire 大小。

### 12.3 根因：三边各有一条宽度策略

|路径|C#|C++|Rust|
|:---|:---|:---|:---|
|exact（能 O(1) 预测）|按预测长度定宽度，回填实际值|同（容器只对连续 + blittable，2026-11-08 补齐）|同 ✅|
|anchor（无法预测）|占 4 字节；`actual <= 16` 才压缩（`Allocator::FinishAnchor` 的 `Limits` 常量）|占 4 字节；`<= 16` 才压缩（`Allocator::FinishAnchor` 的 `AllocatorAnchorShrinkLimits` 常量）|同 ✅（§12.4 已改）|
|string|上界 `3 * chars + 3` 定宽度（`Allocator::AppendWithLengthPrefix` 的 `GetMaxByteCount`）|精确字节数 `size * item.size()`|精确字节数（`str::len()`）|
|`Option`（auto）|无前缀（`NullableConverter` override）|无前缀|同 ✅（§12.6 已改）|

于是出现两组关系：

- Rust ↔ C++：**完全一致** —— anchor 阈值、字符串宽度（都用精确字节数）、容器头宽都相同，
  且与主机端序无关。大端曾经是例外（C++ 的容器退回 anchor 宽头，写 24 字节），2026-11-08
  已修（§12.5 末条）。
  容器曾经不一致：C++ 的容器路径不做长度预测，定长元素 payload 落在 17..=127 字节时保留
  4 字节头，Rust / C# 写 1 字节（`vec_i32_5` 是唯一撞上的用例）。2026-11-08 修掉，机制与
  修法见 §12.5。
- Rust ↔ C#：只在 string 的 `42..=127` 字符区间不同——**C# ↔ C++ 在同一区间也不同**，
  说明参考实现之间本来就不逐字节相等。

所以「对齐」定义成两件事：**解码互操作**（已满足）+ **在可对齐路径上字节一致**
（anchor 已做到，string 路径无法全做到）。

C# 的 string 上界差一个 `O(n)` 的字符扫描才能复刻（`str` 没有 O(1) 字符数），
为 3 字节不值得；C++ 也没这么做。**Rust 跟随 C++**。

### 12.4 决策：anchor 阈值改成 16（已实施）

```rust
// encoder.rs
pub(crate) const PREFIX_SHRINK_LIMIT: usize = 16;   // 与 C# Limits / C++ AllocatorAnchorShrinkLimits 一致
if actual <= PREFIX_SHRINK_LIMIT && 尾部余量够 { /* 压缩，搬移 <= 16 字节 */ }
else { /* 保留 4 字节，只回填长度 */ }
```

**常量不导出**（`pub(crate)`，2026-11-07 决定）。两种宽度都是合法 wire——读端遇到任何长度
都能接受 4 字节头——所以这个数字是**编码侧的规范化策略**，不是解码规则，只有生产端需要；
调用方要的是行为，从 `write_prefixed` 拿到即可。反过来，`pub` 会把一次调优决定升级成 semver
承诺：这条线本来就从 127 改到 16（本节），若已发布，改动就是 breaking；而且公开的常量会
诱导「调一下没关系」的改动，实际会破坏与 C#/C++ 的逐字节相等。测试侧改为在 `wire.rs`
自带一份 `const PREFIX_SHRINK_LIMIT = 16`，由 16 / 17 两个字节级断言钉住——比断言「库里的
常量等于 16」更强，因为钉的是 wire 而不是符号。同理，`prefix_header_len` /
`encode_prefix` 这类仍然公开：它们是**格式规则**的函数（127 边界），不是内部策略数字。

三条理由：

1. **字节对齐**：named object、`Vec<可变长元素>`、嵌套组合在 `17..=127` 区间与 C#/C++ 一致，
   fixture 可以逐字节断言，而不是只能断言「能互解」。
2. **去掉 memmove**：改前 `actual <= 127` 都会 `copy_within`，搬移量等于 payload 长度、最坏 127 字节；
   改后搬移上限 16 字节；2026-11-08 起这 16 字节进一步改成两次定长 `u64` 搬移，连运行时长度
   的 `memmove` 调用也去掉（§19.15.7）。
3. **代价可控**：区间内多 3 字节，占该对象总长 2.3% ~ 14.3%（`3/131` ~ `3/21`），换来近乎零搬移；
   C#/C++ 已经用数据选过这条路。

**实施时发现的隐藏 bug**：保留 4 字节头这一支原来调 `encode_prefix`，而它在
`len <= 127` 时只写 1 字节 —— 于是 `actual = 17` 写出 `11 00 00 00`（三个残留零字节），
wire 直接损坏。宽头必须强制写 4 字节，为此新增 `length::encode_prefix_wide`
（`encode_prefix` 的宽支也复用它）。这个 bug 只有把阈值下调到 16 才会被触发：
原来那条分支只可能遇到 `actual > 127`。

影响面（已逐个核对）：`test_named_wire_exact`、`test_derive_wire_matches_handwritten_layout`
从 `0x11` 变成 `80 00 00 11`；新增 `test_write_prefixed_shrink_boundary`（16 / 17 边界）与
`test_write_prefixed_wire_matches_csharp`。`payload <= 16` 的用例（`Tiny`、`Vec<Vec<i32>>`）不变。

### 12.5 fixture 管道（已建成）

三张表、三个断言文件，共用同一份「名字 → 类型 → 值」表：

```text
crates/binary-tests/fixtures/csharp.tsv      # name <TAB> bucket <TAB> hex，33 条（真实 C# 输出）
crates/binary-tests/fixtures/cxx.tsv         # name <TAB> hex，27 条（真实 C++ 输出；无桶列，全 same）
crates/binary-tests/fixtures/fsharp.tsv      # name <TAB> hex，5 条（真实 F# union 输出）
crates/binary-tests/tests/fixtures.rs        # 断言 csharp.tsv
crates/binary-tests/tests/cxx_fixtures.rs    # 断言 cxx.tsv + 「除缺口外全覆盖」集合断言
crates/binary-tests/tests/fsharp_fixtures.rs # 断言 fsharp.tsv（union 行）
crates/binary-tests/tests/common/cases.rs    # 三张表共用的 name → 类型 → 值 表
```

`fsharp.tsv` 只放 union 行，不是为了省事：`Binary.FSharp` 的 `UnionConverterCreator`
是**另一份实现**（反射 `FSharpType` / `UnionCaseInfo`，与 C# 的 `IUnion` 路径无关），
而 F# 侧没有与「named object / tuple object / 容器」同形的类型 —— 那些行由
`csharp.tsv` 用同一个库的反射 converter 覆盖。union 是这条路独有的一类语料，
也正好是此前唯一没和真实实现比过的一类（§16.4）。

- **生成端：可复现方法**。三张表都不是在这里重写一遍规则，而是驱动**真实的**参考实现
  （C# `Mikodev.Binary` 的 `Converter.EncodeAuto`、C++ `binary-cxx` 的 `ConverterEncodeAuto`、
  F# `Binary.FSharp` 的 `AddFSharpConverterCreators` + `EncodeAuto`）打出字节。探针是一次性
  程序、**不入库**：C++ 那个要 include 外部 checkout 的头文件并链接其 `libbinary.a`，F# 那个
  要引用 `Binary.FSharp.fsproj`，干净 clone 构建不出来；入库的是三张表本身。复现按三步走，
  三边同形：
  1. **取用例集**：`crates/binary-tests/tests/common/cases.rs` 是权威清单 —— 每行的 `name`、
     Rust 类型、构造值都在那里，探针照着它声明同形的参考类型、赋同一组值。表的条数由它
     决定（33 / 27 / 5）；加用例时先改这一处，再重跑探针重生成整张表。
  2. **调真实实现的 `EncodeAuto`**：新建控制台程序引用对应库，逐个值取字节。C# 的两条
     `csharp-only` 行（`string_42a` / `string_127a`）走的是**带前缀**路径，要用
     `EncodeWithLengthPrefix` 取。零成员类型（`union_dot`）库里没有现成 converter，探针里
     手写一个「`Length == 0`、`EncodeAuto` 也写 0 字节」的 converter 并注册，写法见 §16.4
     —— C# 用库自带的 `[Converter]` 特性，不必改 generator 装配，`Generator.CreateDefault()`
     原样可用；C++ 要 `generator.AddConverter(...)`，且必须排在 `VariantConverter<T>` 构造
     之前（后者构造时就要取各 alternative 的 converter）。
  3. **打印表体**：每行 `name <TAB> hex`，hex 小写、无分隔，表体打 stdout 重定向进
     `fixtures/*.tsv`。C# 表多一列 `bucket`：`same` / `csharp-only`，判据是实际字节长度与
     Rust 的预测宽度比（§12.3 的 `42..=127` 区间），例外人工对着字节确认，不由测试代码猜；
     C++ / F# 表全部逐字节相同，没有这一列。
- **断言端**：`include_str!` 在编译期读表，`cases.rs` 的 `macro_rules!` 把用例名映射到具体
  类型的值；同一张表既做 encode 断言也做 decode 回环。**用例名找不到会 panic，不会静默跳过**。
- **分桶**（只有 C# 表带第三列，显式声明，不靠测试代码猜）：
  - `same`：两边逐字节相同，断言字节相等（31 条）。
  - `csharp-only`：C# 用了宽头（string 的 `42..=127` 字符区间，2 条），断言「payload 逐字节
    相等」+「Rust 能解 C# 的字节并还原成自己的 wire」。
  C++ 表没有桶列：27 条全部 `same`，容器头宽修掉之后（见下）不存在第三种情形。
  F# 表也没有桶列，5 条全部 `same`。
- **C++ 容器的头宽：曾经错，已修（`binary-cxx` c6c11cb，2026-11-08）**。修之前
  `ContainerConverter` 不 override `EncodeWithLengthPrefix`，落到
  `Converter<T>::EncodeWithLengthPrefix` 的 anchor 路径 —— `Anchor()` 先占 4 字节，写完由
  `FinishAnchor` 决定压不压，阈值是 `AllocatorAnchorShrinkLimits = 16`，**不是 127**；
  于是宽度成了「写完之后的字节数」的函数，定长元素 payload 落在 17..=127 字节时保留宽头。
  实测（一次性探针）：payload 16 → wire 17 字节（`10` 开头），payload 17 → wire 21 字节
  （`80000011` 开头）。边界仍由两条既有用例夹住：`vec_i32_3`（12 B）与
  `vec_string_4x4`（20 B，元素变长）。
  这是**移植缺口，不是 wire 口径差异**：C# 的同一条 anchor 路径阈值也是 16
  （`Allocator.Invoke.cs` 的 `FinishAnchor` 里 `const int Limits = 16`），差别在于 C# 对
  **定长元素**的 `List<T>` / `T[]` 另有一条 native-endian 快路径
  （`SpanLikeNativeEndianMethods.EncodeWithLengthPrefix` → `Converter.EncodeWithLengthPrefix`）：
  长度 O(1) 已知，直接按真实长度选头宽。C++ 的 `copy` 标志只搬了这条快路径的 memcpy 一半
  （payload 整块拷），没搬「换个方式写头」那一半 —— 而
  `::binary::EncodeWithLengthPrefix(Allocator&, std::span<const std::byte>)` 这个「先算前缀
  宽度、一次写成」的自由函数库里本来就有（`ConverterExtensions.cpp`），此前只有测试在调。
  修法就是把它接上：两份 `ContainerConverter`（virtual 版与 experimental 静态版）都
  override `EncodeWithLengthPrefix`，`copy` / `Copyable` 单分支改调那个自由函数，其余落回
  基类；`Encode`（无前缀）不能动，tuple 末位还靠它。experimental 版原来**比 C# 还宽**：
  它对所有定长元素都走 `ContainerLengthFunction<T>` 预测，`std::map<i32,i64>` 五个元素
  它写 1 字节头、C# 写 4 字节头；收窄成 `Copyable` 单分支后三者同一条规则。两边各补 5 条
  用例（`…EncodeAutoBlittableShortLengthPrefixTest` / `…BlittableLengthPrefixBoundaryTest` /
  `…NonBlittableKeepsAnchorTest` / `…NonContiguousKeepsAnchorTest` / `…EmptyContainerTest`），
  钉住「预测只给连续 + blittable」。修完 `vec_i32_5` 变 `same`，桶列删除；§12.3 那句
  「Rust ↔ C++ 在容器上不一致」随之改回**完全一致**。
- **大端主机没有这条快路径 → 已修（2026-11-08，四架构矩阵首跑发现，同日修掉）**。
  `Copyable` 原来把 `std::endian::native == std::endian::little` 也写进同一个谓词
  （`sizeof(E) == 1` 除外），于是 s390x 上的 `vector<int32_t>` 只能走「逐元素
  `LittleEndianConverter`」+ anchor：20 字节 payload 大于 16，4 字节头原样保留，
  `80 00 00 14 | payload`，wire 24 字节；LE 上同一值是 21 字节。两种都是合法编码
  （`DecodeLengthPrefix` 只看首位），问题是**产出的字节数跟主机端序挂钩**，Rust 不受影响
  （前缀来自 `len() * LEN` 的预测，与 memcpy 无关，s390x 上 `vec_i32_5` 仍是 21 字节）。
  **根因是把两个不相干的判断合成了一个**：「元素区能否整块 memcpy」需要主机序等于 wire 序，
  「容器字节数是否 O(1) 已知」只需要定长 + 连续。修法是把谓词拆开 ——
  `Contiguous`（有 `data()` 且 `sizeof(E) == 1 || trivially_copyable`，**与端序无关**）与
  `Copyable = Contiguous && (sizeof(E) == 1 || LE 主机)`（能否 memcpy）；`copy` 标志改由
  `Contiguous` 决定，`EncodeWithLengthPrefix` 多一条 `Contiguous && !Copyable` 分支：
  `::binary::Encode(allocator, sizeof(E) * item.size())` 先按预测宽度写头，再逐元素
  `EncodeAuto`（大端上就是字节交换）。`Encode`（无前缀）和 `Decode` 的 reinterpret 分支仍只在
  `Copyable` 下启用，大端继续走循环。experimental 静态版同样拆成 `Contiguous` / `Copyable`，
  `EncodeWithLengthPrefix` 变三分支（memcpy 预测 / 预测 + 逐元素 / anchor）。
  **边界没动**：非连续（`std::map`）与非 blittable（`std::string`、tuple）元素仍走 anchor，
  阈值 `AllocatorAnchorShrinkLimits = 16` 与 C# 的 `Limits = 16` 对齐，`…KeepsAnchorTest`
  继续钉住这条；大端与 LE 在这条路径上本来就一致，所以 C# 语料的逐字节断言不受影响。
  修完 s390x 的 `vec_i32_5` 回到 21 字节，两条用例的 `if constexpr (std::endian::native == …)`
  分支删掉、改回单一断言，`<bit>` 引用一并删除。
- **缺口**：`cxx.tsv` 比 `csharp.tsv` 少 6 条 —— `i32opt_none` / `i32opt_some5` /
  `vec_i32opt` / `vec_i32opt_none` / `named_with_opt_none` / `named_with_opt_some`。
  原因是 `binary-cxx` **没有 nullable converter**（`code/` 下既没有 `optional` 也没有
  `Nullable*`；`std::variant` 是「索引 + 活跃分支」，与 `Option<T>` 的「tag + `auto(value)`、
  不套前缀」不是同一形状）。在这里手写一个 `std::monostate` 适配器只能证明「这个文件能写出
  这些字节」，证不了「C++ 库会这么写」，所以宁缺不造。
  `tests/cxx_fixtures.rs::test_cxx_covers_every_expressible_case` 把这层意思钉成断言：
  C++ 的名字集合必须等于 C# 的集合减去 `CXX_GAP` ——往 `csharp.tsv` 加一条却不更新
  `cxx.tsv`（或缺口表）会直接失败，不会悄悄少测。
- **覆盖**：payload 16 / 17、字符串 0 / 16 / 17 / 41 / 42 / 127 / 128 字符、多字节字符、
  `Vec<定长>`（12 / 20 字节两档）/ `Vec<变长>` / `Vec<Option<i32>>` / `Vec<enum>`、tuple、
  tuple object ×2、named（含 `int?` 成员）、`Option`、enum。
- **named 的顺序不是契约**（§19.4）：C# 按名字序、Rust / C++ 按声明序。现有 3 条 named 用例
  的名字恰好已经是字典序（`id`/`name`、`a`/`b`/`c`、`x`），所以逐字节断言成立；真要加一条
  顺序不同的，只能断言「各自解出同一组值」，不能再比字节 —— 目前还没有这样的行。
- **union / `std::variant` 已补**（2026-11-09，含 unit 用例，§16.4）：五条 union 行在
  三张表里，四边 tag 与 payload 逐字节相同。

### 12.6 修正：`Option<T>` 原来多了一个前缀

建 fixture 时发现 `i32opt_none` 对不上：C# 是 `00`，Rust 是 `01 00`。

- **C#**：`NullableConverter` 直接 override `EncodeAuto` / `DecodeAuto`，写
  `tag + auto(value)`，**不套前缀**（`NullableConverter::EncodeAuto`）。所以
  `List<int?>{null, 5}` = `06 | 00 | 01 05000000`。
- **Rust 改前**：`Option` 只实现了 `encode` / `decode`，`auto` 走默认路径 → 因为
  `LEN = 0`（变长）而套上 anchor 前缀 → `05 0105000000`。这个 wire **C# 解不了**
  （把 `05` 当 tag，直接 invalid tag），属于真不兼容，不只是宽度差异。

改法：像 tuple 一样 override `encode_auto` / `decode_auto`（tag + `auto`，无前缀），
`encode` / `decode` 用 raw value。named 成员仍带前缀（`05 | 01 05000000`），
因为那由默认 `encode_with_prefix` 提供，正是 C# `EncodeWithLengthPrefix` 的行为。

回归：`vec_i32opt`、`vec_i32opt_none`、`named_with_opt_none`、`named_with_opt_some`
四条 fixture 钉死这四种场景。

---

## 13. 与 C# / C++ 的功能覆盖对照（更新至 2026-11-08）

图例：✅ 有且对齐 · ⚠️ 部分 · ❌ 无。

|能力|C#|C++|Rust|备注|
|:---|:---|:---|:---|:---|
|整数 / 浮点 / bool|✅|✅|✅|`i128/u128` 对齐；C# 另有 `Half`（2B），Rust `f16` 未稳定|
|`char`|✅ 2B（UTF-16 码元）|✅ `char16_t` 等|✅ 4B|**语义不同**：Rust `char` = C# `Rune` = 4B；C# `char` 在 Rust 无对应|
|字符串|✅ UTF-8（内存是 UTF-16，编码时转码）|✅ 字节序列（库不保证是 UTF-8，也不校验）|✅ UTF-8（类型不变量保证合法，解码校验）|三边 wire 都是 UTF-8 字节，**内存表示各不相同**；所以跨语言 benchmark 不用 `string`，用字节类型（§19.15.1）|
|`Option` / `Nullable`|✅|✅ `optional`|✅|tag + value，auto 无前缀，字节一致（§12.6）|
|序列容器|✅ List / LinkedList / Stack / Queue / Immutable* / Frozen* / `ReadOnlySequence`|✅ 任意 range|✅ `Vec` / `[T;N]` / `VecDeque` / `LinkedList` / `BinaryHeap` / `BTreeSet` / `Cow`|与 `Vec<T>` 同 wire，不写元素个数（§19.14）；`Cow` 解码恒 Owned|
|映射 / 集合|✅ Dictionary / HashSet / Immutable* / Frozen*|✅ `map` / `set`（range）|✅ `BTreeMap` / `BTreeSet` / `HashMap` / `HashSet`|条目 = `(K,V)` flatten；哈希容器顺序不作跨语言字节保证（§17.3 / §19.14）|
|键值对 / pair|✅ `KeyValuePair`|✅ `std::pair`|✅ `(K, V)` tuple|不新增类型；`encode_auto` 两成员都 auto、无 pair 头（§19.14）|
|enum（底层整数）|✅ `LittleEndianEnumConverter`|✅ 无专用 converter，直接用 `LittleEndianConverter<T>`|✅ `#[binary(int)]`|C++ 的 `LittleEndianConverter` 只约束 `same_as<T, remove_cv_t<T>>`，enum 按 `sizeof(T)`（= 底层类型宽度）走整数路径，实验 API 的测试类型表里就包含 `enum class ObjectKind`；Rust 只支持 C 类枚举，默认 `i32`，`#[repr]` 控制宽度|
|union / variant|✅ `UnionConverter`（tag 走长度前缀）|✅ `VariantConverter`|✅ `#[binary(union)]`|tag 与两实现逐字节一致（§16）；支持 unit / tuple / struct 变体|
|named object|✅ `[NamedObject]`+`[NamedKey]`|✅ `NamedObjectConverter` + schema|✅ derive|wire 逐字节对齐，fixture 覆盖（§12.5）|
|tuple object|✅ `[TupleObject]`+`[TupleKey]`（键必须连续）|✅ `TupleObjectConverter`|✅ `#[binary(tuple)]`|无泛型具名结构体；成员顺序 = 声明顺序；wire 已由 2 条 fixture 钉死|
|成员级自定义转换器|✅ `[Converter]` / `[ConverterCreator]`|✅ `FixedConverterPolicy`|✅ `#[binary(with = "m")]`|模块给 `Repr` + `to` / `from`，wire 取 `Repr` 的（§19.17）|
|零拷贝借用解码|✅ `SpanLike` 一族|✅ `std::span`|✅ `&'de str` / `&'de [u8]`|由成员类型表达，不需要属性；`&'de [T]` 不做——payload 对齐无法保证（§19.17）|
|拥有型指针|✅ 引用类型天然|✅ 指针天然|✅ `Box` / `Rc` / `Arc`|wire 透明，无 tag / 长度 / 身份（§19.13）|
|递归类型|✅ 运行期递归 + 兜底栈检查|✅|✅|`Box<T>` 已可用；深度不受限制，栈溢出按资源耗尽处理（§19.13）|
|泛型目标类型|✅ 运行期构造|✅ 模板天然支持|⚠️ 只允许一个生命周期参数|借用成员需要它；type / const 参数仍拒绝（§19.17）|
|运行期 converter 注册 / 反射|✅ `IGenerator` / `GeneratorBuilder` / `Token`|✅ `Generator` + `AddConverter`|❌ 有意不做|§7：Rust 无反射，静态 trait 替代|
|编译期代码生成|✅ SourceGenerator（31 条诊断）|—|⚠️ proc-macro（27 条负向快照，§15）|诊断条数仍落后，但每条都有回归保护|
|流式 / Brotli|✅|❌|❌|有意不做（§6）|
|no_std|—|—|✅|Rust 侧唯一超出两实现的能力|

### 13.1 优先级（为什么是这个顺序）

1. ~~**fixture + anchor 阈值**（§12）~~ ✅ 已完成：不先钉住字节，后面每加一个类型都要重新怀疑 wire。
2. ~~**enum + tuple object**~~ ✅ 已完成（§14）：wire 最简单、两实现都有、derive 已有骨架。
3. ~~**`union` / `variant`**~~ ✅ 已完成（§16）：derive 的最后一个形态，wire 直接复用
   tuple object 规则 + 一个紧凑整数 tag。
4. ~~**指针与容器**~~ ✅ 已完成（§19.13 / §19.14）：`Box` / `Rc` / `Arc` 解除递归阻塞，
   八个标准容器全部落地，零新增依赖。`with` / `borrow` 现在是第一优先级。
5. ~~**`with` / `borrow`**~~ ✅ 已完成（§19.17）：功能补齐，不阻塞别人；wire 不变，无新增公开 API。
6. **benchmark + 四项性能优化**：硬规矩是先基准，没有数据不动代码。

### 13.2 规模对照（判断「还差多少」的参照）

|项|C#|C++|Rust|
|:---|:---|:---|:---|
|核心源文件|143 `.cs`|52 头（含 experimental）+ 10 源|15 `.rs` + 1 proc-macro|
|测试规模|188 个测试文件 / 28 个测试项目|44 个测试源文件|153 个用例（含 33 条 C#、27 条 C++ 与 5 条 F# union fixture）+ 27 条 compile-fail 快照 + 4 个 doc test，全部在 `tests/`|
|benchmark|17 个 `Benchmarks.*` 项目（含 `Benchmarks.IntegrationTests`，JIT / AOT 双 generator）|`binary-benchmarks` 四变体（C# JIT / C# AOT / C++ virtual / C++ template）|手写 harness `crates/binary-bench`（wire / objects / collections / errors / profile / lookup 六组，§19）；C# AOT 同口径对照 / C++ union-enum 场景仍缺|
|编译期诊断|31 条|—|27 条错误路径（全部有快照，§15）|

---

## 14. 已实施：tuple object / enum(int) / derive 静默错误（2026-11-07）

### 14.1 tuple object 不是「每个成员都带前缀」

先查源码再动手，结论和直觉相反：C# `ContextMethodsOfTupleObject.GetEncodeDelegateAsTupleObject`
对成员用 `EncodeAuto`，**只有最后一个成员**在 raw 模式下用 `Encode`；
`TupleObjectDelegateConverter` 直接 override `EncodeAuto` / `DecodeAuto` 调委托，
**不加自身前缀**。逐字节验证：

|用例|C#|Rust `#[binary(tuple)]`|
|:---|:---|:---|
|`{ int, int }`|`01000000 02000000`|同|
|`{ int, string }`|`01000000 0161`|同|

所以 tuple object ≡ 同成员元组。`LEN` 按 `TupleObject.GetConverterLength`
= `Σ LEN(member)`，任一成员为 `0` 则 `0`——这个值决定外层容器的头宽：
`Vec<{int,int}>` 3 个元素 = `18 | 8 字节 * 3`（exact 路径），与 C# 一致。

### 14.2 enum：判别式按底层整数写

C# `LittleEndianEnumConverter` 用底层类型宽度（`enum : int` → 4 字节，实测
`enum_green` = `02000000`；`List<Color>` 2 个元素 = `08 01000000 02000000`）。
Rust 侧：

- 默认 `i32`（与 C# 默认一致），`#[repr(u8)]` 等改宽度，`LEN = size_of(repr)`。
- 编码用 `match *self { Self::A => 0 as i32, … }`，不依赖 `Copy`，也不用 `as` 强转 `*self`。
- 隐式判别式沿上一个值递增（`Low, High = 7, Bumped` → 0 / 7 / 8）。
- 解码遇到未声明的值返回 `Error::message_static("invalid enum discriminant")`，不会误映射到某个变体。
- 只支持**无字段变体**；tuple / struct 变体、非字面量判别式都报编译错误。

### 14.3 三个静默错误都改成编译错误

|原来|现在|
|:---|:---|
|结构体级 `#[binary(...)]` 被静默忽略|解析并校验；未知选项报「phase 1 supports `tuple`/`int`」，caret 落在选项上|
|重复 `rename` 编译零警告、解码必失败|``duplicate wire name "id" on field `b`; `a` already uses it``，caret 落在字段上|
|非 ASCII `rename` 产出 `b"名字"` → 非法字节串|反转义成字节后 `Literal::byte_string`，`rename = "名字"` → `b"\xe5\x90\x8d\xe5\xad\x97"`|

顺带：`enum` / `union` / tuple struct / 泛型都换成了具体消息，不再是「expected struct」。
这三条错误信息当时靠手工验证（临时 crate + `cargo build`）；现已全部纳入 §15 的快照。

这一轮新增的 4 条 fixture（`tupleobj_two_int`、`tupleobj_int_str`、`enum_green`、`vec_enum`）
全部落在 `same` 桶——即与真实 C# 逐字节相等。

---

## 15. 已实施：派生宏诊断定位 + 负向测试（2026-11-07）

### 15.1 两个缺陷的根因

**文案泄漏内部表示**。`Field.wire` 存的是 `b"..."` 字面量**源码文本**，直接 `{}` 拼进错误消息，
于是 `rename = "x"` 冲突时用户看到 `duplicate wire name b"x"`。改成存原始字节
（`Vec<u8>`）：生成代码时才用 `Literal::byte_string` 还原成字面量，诊断时用
`str::from_utf8` 后 `{:?}` 渲染成 `"x"`（非 UTF-8 才退回 `b"\xff"`）。

**所有错误都指向 `#[derive(...)]`**。tokenizer 把 `TokenTree` 拍平成
`Tok::{Ident, Literal, Punct, Group}` 时把 `Span` 丢了，`compile_error!` 只能用
`call_site()`。现在：

```rust
enum Tok {
    Ident(String, Span),
    Punct(char, Span),            // 只为 `B = 5` 这类用法能指向标点
    Literal(String, Span),
    Group(Delimiter, Vec<Tok>, Span),
}

struct Error { msg: String, span: Span }   // 取代裸 String
```

`compile_error!` 输出前用 `with_span()` 递归重设整段 token 的 span（group 内部也要重设，
否则 caret 会跳回 `call_site`）。结果：

|错误|caret 位置|
|:---|:---|
|重复 wire name|重复的那个字段名|
|tuple 带成员属性|违规字段名|
|未知字段选项|选项标识符（`nope`）|
|`rename` 非字面量|那个字面量|
|未知结构体级选项|选项标识符|
|union|`union` 关键字|
|tuple struct|括号组|
|空结构体 / 空枚举|花括号|
|泛型结构体 / 枚举|类型名|

**去重是白捡的**：`Encode` 与 `Decode` 各自独立解析，都会报同一个错。以前两条 span 不同
（分别指向 `Encode` 和 `Decode`），用户看到两条重复错误；现在 span 与文案完全相同，
rustc 直接合并成一条。

### 15.2 compile-fail fixture（当时 23 个，现 27 个）

`crates/binary-tests/tests/ui.rs`：

```rust
#[cfg(not(miri))]
#[test]
fn ui() {
    trybuild::TestCases::new().compile_fail("tests/ui/*.rs");
}
```

每个 `tests/ui/<name>.rs` 配一个 `<name>.stderr`，里面存的是**这段代码编译时 rustc
应当输出的完整诊断文本**（`error: ...` 行 + `-->` 位置 + 带 caret 的源码片段）。
trybuild 编译 fixture、把真实诊断与 `.stderr` 逐字符比对：一致就通过，不一致就带 diff
失败。它们不是运行时数据，是「错误消息 + caret 位置」的回归快照——改了错误文案或
span 计算而没重生成，测试就会红。

重生成快照：`TRYBUILD=overwrite cargo test -p binary-tests --test ui`。
每个 fixture 对应一条错误消息：`duplicate_wire_name` / `unknown_struct_option` /
`unknown_field_option` / `rename_not_literal` / `tuple_with_member_attr` /
`enum_without_int` / `int_on_struct` / `tuple_on_enum` / `union_unsupported` /
`tuple_struct` / `generic_struct` / `generic_enum` / `empty_struct` /
`enum_variant_with_fields` / `enum_bad_repr` / `enum_non_literal_discriminant` /
`enum_no_variants`。


**删掉的三个候选**（试跑后判定为噪声）：`missing_comma` / `enum_missing_comma` 钉的是
rustc 自己的语法错误——真少逗号时 rustc 的 parser 先失败，宏拿到的 token 流已被嚼掉一段，
只会生成缺字段的初始化式并被 E0063 兜住；`rename_bad_escape` 钉的是 rustc 的 lexer
（`"\q"` 在宏看到之前就被拒绝）。三者措辞随 rustc 版本变，留着重生成快照只会变成假警报。

### 15.3 依赖边界

`trybuild` 只出现在 `crates/binary-tests` 的 `[dev-dependencies]`：

- `cargo tree -e normal -p binary` 仍然只有 `binary → binary-derive`；
- `cargo build -p binary --no-default-features`（`no_std`）不受影响；
- 对外发布的两个 crate 依然零依赖；§19.14 的八个容器全在 `alloc` / `std`，
  `cargo tree -e normal -p binary` 仍是 `binary → binary-derive`，没有引入 `hashbrown`；
- 代价：21 个第三方 crate（`serde` 家族、`toml` 家族、`glob`、`termcolor`、`target-tuple`
  及传递依赖）、约 2 MB 源码、~9 s 冷启动；新机器第一次 `cargo test` 需要联网。
- 版本**不钉死**（`trybuild = "1"`）。`compile_fail` 这个调用面是稳的；而 `.stderr`
  快照钉的是 **rustc** 渲染出来的文本（本目录 23 个快照里没有一个含 trybuild 自己画的
  `┈` 边框），所以钉 trybuild 只钉住了漂移里较小的一半，真正的变量是 rustc 版本。
  升级后若渲染有变，测试会带着 diff 失败而不是静默放过：看 diff，再
  `TRYBUILD=overwrite` 重生成。`.gitignore` 忽略 `Cargo.lock`（库工作区惯例），
  因此也不存在「靠 lockfile 锁住」这条路——既然锁不住全部，就不假装锁住。

---

## 16. 已实施：`#[binary(union)]` 标记联合（2026-11-07）

### 16.1 wire 契约（三边一致的源码依据）

```text
[tag] [variant payload]
      ^ 与 tuple object 同规则
```

**tag 是紧凑整数，不是定宽整数**：

|边|实现|源码入口|
|:---|:---|:---|
|C#|`Converter::Encode(ref Allocator, int)` → `NumberModule::EncodeLength`（`n >> 7 == 0` → 1 字节，否则 4 字节大端 + bit31）|`Converter::Encode` / `NumberModule::EncodeLength`|
|C++|`::binary::Encode(Allocator&, size_t)` → `EncodeLengthPrefixLength` + `EncodeLengthPrefix`|`ConverterExtensions.cpp:Encode`|
|Rust|`Encoder::write_compact_int` / `Decoder::read_compact_int`，复用 `prefix_header_len` + `encode_prefix_wide` / `decode_prefix`|`encoder.rs:write_compact_int` / `decoder.rs:read_compact_int`|

也就是说「tag 编码」与「长度前缀编码」在三条实现里是**同一个函数的不同名字**。
`test_compact_int_width` 钉住 127/128 这个宽度拐点（`80 00 00 80` = 128 | bit31）。

payload 完全复用 tuple object 规则（§11 / §14.1）：

|变体形态|`encode`（raw）|`encode_auto`|
|:---|:---|:---|
|0 成员（unit）|tag|tag|
|1 成员|tag + `Encode::encode`|tag + `auto`|
|N 成员|tag + 前 N-1 个 `auto` + 最后一个 raw|tag + 全部 `auto`|

**两种形态都不加自身前缀**，与 `Option<T>`（§12.6）同理：tag 已指明是哪个变体，
变体 payload 自身可自界定（定长直读，变长自带前缀）。

源码依据：

- C# `UnionConverterCreator.GetEncodeExpression` 的 `MakeBody` 是
  `if auto || i <> properties.Length - 1 then "EncodeAuto" else "Encode"` ——
  与 `TupleObjectConverter` 逐字相同的分流，这就是「多成员变体 = 同成员 tuple」的证据。
- `UnionConverter` 只 override `Encode` / `EncodeAuto` / `Decode` / `DecodeAuto`。
  基类 `Converter<T>.EncodeAuto` 在 `Length == 0` 时走
  `EncodeWithLengthPrefixVariable`＝`Allocator.Anchor` + `Encode` + `FinishAnchor`
  （`Converter<T>::EncodeWithLengthPrefixVariable`），所以**不 override `EncodeAuto` 就会被 anchor 加前缀**；
  C# 这里 override 了，Rust 的派生代码也必须 override `encode_auto`。
- named 成员位置仍然带前缀，因为那是框架给的：`write_named_member` 调 `encode_with_prefix`，
  默认实现是 anchor over `encode`，与 C# `EncodeWithLengthPrefix` 的 anchor 路径一致。

**实测（2026-11-09，三边真实实现）**。同一组形状，单元格是 `auto / raw`
（`EncodeAuto` / `Encode`）：

|形状|C# `[Union]`|F# DU|C++ `std::variant`|Rust|
|:---|:---|:---|:---|:---|
|`Dot`（unit）|`00` / `00`|`00` / `00`|`00` / `00`|`00` / `00`|
|`Circle(1.5)`|`01 000000000000f83f`|`01 …`|`01 …`|`01 …`|
|`Text("a")`|`02 0161` / `02 61`|`02 0161` / `02 61`|`02 0161` / `02 61`|`02 0161` / `02 61`|
|`Rect{1,2}`|`03 01000000 02000000`|`03 …`|`03 …`|`03 …`|
|`Pair(1,"x")`|`04 01000000 0178` / `04 01000000 78`|`04 …`|`04 …`|`04 …`|

四边的 tag 与 payload **全部逐字节相同**。语料落在 `fixtures/csharp.tsv` /
`cxx.tsv` / `fsharp.tsv`（§12.5），五条 union 行都是 `same` 桶。

C# 与 C++ 的 `Dot` 走的是**手写 converter**，不是库生成的：那两个实现都没有
「零字节 payload 的 converter」，`[TupleObject] class Dot {}` 抛
`No available member found`，`TupleConverter<std::tuple<>>` 编译不过（§16.4）。
手写的那个把 `Length` 设为 `0`，并**同时覆盖 auto 入口**——这一步不能省：
两边的 `EncodeAuto` 默认实现都在 `Length == 0` 时加长度前缀（C#
`EncodeWithLengthPrefixVariable`、C++ `Converter::EncodeWithLengthPrefix`），
那样 `Dot` 会多出一个 `00`，与 F# / Rust 的「unit 只有 tag」不一致。覆盖成空实现后，
`Length == 0` 就只表示「raw 零字节」，不再暗示「变长、auto 带前缀」。

两处机制细节决定 tag 与 unit 的行为，一并记下：

- **C# 的 tag 是「先编号、后排序」**。`UnionConverterCreator` 先
  `caseTypeList.Select((x, i) => new UnionCaseInfo(i, ...))` 把**声明序号**写进 `Index`，
  再 `caseList.Sort(CommonModule.CompareConversion)`。排序只改 encode 的 `if` 链与 decode 的
  `switch` 顺序（派生类型必须先判），**写出去的 tag 仍是原始序号**。实测
  `[Union] (NamedOne, NamedTwo)`（`NamedTwo : NamedOne`）：base 得 `00`、derived 得 `01`，
  与声明顺序一致 —— 若排序改写了编号，这里会反过来。
- **F# 的 unit 用例不写任何字节**。`UnionConverterCreator.GetEncodeExpression` 里的
  `MakeCase` 在 `info.GetFields() |> Array.isEmpty` 时返回 `Expression.Empty()`，所以 `Dot`
  只有 tag。C# / C++ 的 unit 也走「case 类型 → 它的 converter」这条路，所以那个 converter
  必须手写（§16.4）；F# 这一侧不需要，它在生成表达式时就绕过了 converter。

### 16.2 决策

|问题|决策|理由|
|:---|:---|:---|
|tag 取值|声明顺序 `0..n`|C++ `item.index()`、C# F# union case 都是声明顺序|
|显式判别式 `B = 5`|编译错误|union 的 tag 不是 `Discriminant`；静默忽略会让 wire 与用户预期不符|
|`#[repr(...)]`|编译错误|tag 永远是紧凑整数，repr 不影响 wire；不能让人误以为 `repr(u8)` 能收窄 tag|
|变体成员属性|编译错误|成员位置化且全量编码，`rename` / `optional` / `skip` 无意义|
|结构体变体 `V { a, b }`|支持，按声明顺序位置化|C# union case 的多字段就是这么写的（`MakeBody` 遍历 `GetFields()`）|
|`Layout::LEN`|恒 `0`|tag 宽度随变体序号变（`0..=127` 一字节），C# `UnionConverter` 也是变长|
|`Option` 的 tag 错误|统一为 `Error::InvalidTag(u32)`|原来是 `Message("invalid Option tag")`，丢掉了具体值|
|冲突的布局模式|编译错误|`#[binary(tuple, union)]` 以前是后者胜出，静默得离谱|

### 16.3 新增用例（7 个，`crates/binary-tests/tests/derive.rs`）

- `test_union_tag_wire` —— unit 变体只有 tag；`LEN = 0`；struct 变体 = 成员按序拼接；
  `auto` payload 带成员自身前缀、raw 不带。
- `test_union_raw_vs_auto_payload` —— 多成员变体的「前 N-1 auto + 最后 raw」逐字节钉死
  （`[02, 01 00 00 00, 01 'x']` vs `[02, 01 00 00 00, 'x']`）。
- `test_union_round_trip` —— auto / raw 两条路径都幂等。
- `test_union_in_vec` —— 变长元素集合（anchor 路径）里能恢复元素个数。
- `test_union_invalid_tag` —— tag 7 报 `InvalidTag(7)`，不产生别名。
- `test_union_as_named_member` —— named 成员位置的前缀由框架给：
  `09 | 05 "shape" | 02 | 02 'a'`。
- `test_union_edge_shapes` —— `Nothing()`（空元组变体）等价 unit；`Pairs(Vec<(i32, i32)>)`
  证明字段类型的逗号在自身定界符里不会被 `take_type` 切断。

新增负向用例 5 条：`union_on_struct` / `union_repr` / `union_explicit_discriminant` /
`union_field_attr` / `conflicting_modes`；另把枚举变体上的 `#[binary(...)]` 从静默跳过
改成报错（`enum_variant_attr`），并顺带把 `enum_without_int` / `enum_bad_repr` 的 caret
从 `#[derive(...)]` 挪到 `enum` 关键字与 `#[repr(...)]` 上。

为此 `Tok::Punct` 现在也携带 `Span`（原先只有 `Ident` / `Literal` / `Group` 带），
否则 `B = 5` 的 caret 只能落在 derive 上——`Error::at` 拿到的是 `Span::call_site()`。

### 16.4 仍然存在的缺口

- ~~**union 的跨语言语料**~~ ✅ 已补全（2026-11-09，含 unit 用例）。三边都实测过
  （§16.1 的表）：F# `UnionConverter`、C# `[Union]`（`union` 关键字与 `[Union]` + `IUnion`
  两条声明方式最终拿到的是同一个反射 `UnionConverter`，字节也相同）、C++ `std::variant`。
  五条语料（`union_dot` / `union_circle` / `union_text` / `union_rect` / `union_pair`）落在
  三张表里，tag 与 payload 全部逐字节相同。
  `Dot` 在 C# / C++ 上的 converter 是**手写**的（用户 2026-11-09 指出），这不构成「自造参考
  实现给自己作证」：
  - 缺的不是 union 逻辑，而是一个**零字节类型的 converter**。tag 的写法、`auto` / raw 的
    payload 分流、非法 tag 的报错，仍由库的 `UnionConverter` / `VariantConverter` 产生；
    手写的那个只提供「写 0 字节、读出 `Dot`」这个顶点。
  - 与 nullable 那 6 条的区别：那 6 条要复刻整个 nullable 语义（tag + `auto(value)`），
    等于用本文件给库作证；这里补的是库本来就没有的**输入类型**，语义只有一条
    「`Length == 0` 且 auto 不写前缀」，而且这条语义由 F# 与 Rust 两侧独立给出、不是本文件的
    发明。
  - 确切写法：C# 侧 `sealed class DotConverter : Converter<DotCase>` 覆盖 `Encode` /
    `Decode` / `EncodeAuto` / `DecodeAuto`，再用库自带的 `[Converter(typeof(DotConverter))]`
    标在 `DotCase` 上 —— **不需要改 generator 装配**，`Generator.CreateDefault()` 原样可用
    （`FallbackAttributesMethods.GetConverter` 会在没有注册项时读这个特性）。C++ 侧没有特性
    机制，`class DotConverter : ::binary::Converter<Dot>` 后
    `generator.AddConverter(std::make_shared<DotConverter>())`。
    **auto 两个必须覆盖**，否则默认实现按「变长」加一个长度前缀，`Dot` 会变成 `00 00`
    （§16.1 已记这条）。
  - 库仍然不自带零成员类型的 converter，所以两张表的探针里各有一段手写代码（§12.5）。
    那是**语料生成器**的一部分，不是库的能力；但 C# 的注册走的是库的公开机制，不是绕过它。
  Rust 侧的 unit 形态另由 `tests/derive.rs::test_union_tag_wire` 钉住（`[0x00]`）。
- **两变体 union ≡ `Option<T>`（wire 等价，2026-11-09 钉住）**：`None`/unit 变体同写 `00`、
  `Some(v)`/单字段变体同写 `01` + auto(v)，`tests/derive.rs::test_union_two_variants_match_option`
  逐字节断言并双向互解（定长 `i32` 与变长 `String` 两种 payload）。等价是**钉住的巧合**而非共享
  实现：`option.rs` 写定宽 1 字节 tag，derive 写 compact int，重合只在 compact int 的单字节
  区间（union 第 129 个变体起 tag 变宽，`Option` 不会）。
- ~~**递归 union**~~ ✅ 已解决（2026-11-08，§19.13）：`Box<T>` 落地后，
  `enum Tree { Leaf(i32), Node(Box<Tree>, Box<Tree>) }` 可 derive 并 round-trip，
  由 `tests/smart_ptr.rs::test_recursive_union_round_trip` 钉住。仍无跨语言 fixture。

## 17. `no_std` / `alloc` / `std` 的分层评估（2026-11-07）

### 17.1 当时的账：`std` feature 只 gate 了一行（2026-11-07 快照）

```rust
// lib.rs（这两行现在仍然如此）
#![cfg_attr(not(feature = "std"), no_std)]
extern crate alloc;

// error.rs（当时的写法；现在是无 cfg 的 `impl core::error::Error for Error`，见 §18.7）
#[cfg(feature = "std")]
impl std::error::Error for Error {}
```

当时全 crate 只有 3 处 `cfg`（`lib.rs` 2 处、`error.rs` 1 处）。`std` 在 §19.14 落地后
`collections.rs` 另有 10 处 `#[cfg(feature = "std")]`，现状见 §17.5。也就是说：

- 当时「no_std」的准确说法是 **`no_std + alloc`**：`Vec` / `String` / `&str` / `[T; N]` 全都在，
  被 `std` 挡掉的只有 `Error` 的 `std::error::Error` impl 这一处；`HashMap` / `HashSet` 是
  后来才进 `std` gate 的（§19.14）。
- `cargo build -p binary --no-default-features` 一直通过，维护成本约等于 0。

### 17.2 收益有多大

不大，但不为零：

|使用者|能否用|说明|
|:---|:---|:---|
|常规 std 应用|✅|默认 feature，无需配置|
|`no_std` + 有分配器（embassy / esp-idf / UEFI / 内核模块 / no_std wasm）|✅|现在就能用，无需改代码|
|裸机无分配器|⚠️|用不了：`Encoder` 的 `Owned` 后端与 `finish() -> Vec<u8>` 都要求 alloc|

所以 `no_std` 是「顺带就有」，成本接近 0；**代价不在维护，而在 API 形状**：只要保留
`Encoder::new()` / `finish() -> Vec<u8>`，就永远绑死 alloc，无分配的目标也就永远差一步。
哪天真要支持裸机，做法是加一层 `alloc` feature，把 `Backing::Owned`、`Vec`/`String` 的 impl
和 `finish()` 移出去，让 `Encoder::on_slice` 成为唯一入口——估计 12 处改动，仍不需要拆 crate。

### 17.3 扩展「标准库常见类型」的代价：几乎都不需要 `std`

关键事实：Rust 把容器分在 `alloc` 和 `std` 两个 crate 里，其中**只有 `HashMap` / `HashSet`
需要 `std`**：

|类型|所在 crate|现状|
|:---|:---|:---|
|`Vec<T>` / `String` / `&str` / `&[u8]` / `[T; N]`|`alloc`|✅ 已有|
|`Box<T>`|`alloc`|✅ wire 透明（§19.13）|
|`VecDeque` / `LinkedList` / `BinaryHeap`|`alloc`|✅ 无条件编译（§19.14）|
|`BTreeMap` / `BTreeSet`|`alloc`|✅ 无条件编译（§19.14）|
|`Rc<T>` / `Arc<T>`|`alloc`|✅ wire 透明（§19.13）|
|`Cow<'a, T>`|`alloc`|✅ wire 透明、解码恒 Owned（§19.14）|
|`HashMap` / `HashSet`|`std`|✅ `#[cfg(feature = "std")]`（§19.14）|

**订正（2026-11-08，§19.14）**：原文这里写的「先写元素个数，再逐个 `auto`」**是错的**。
三边都不写元素个数 —— C++ `ContainerConverter::Encode` 只是 `for (auto& i : item)
converter->EncodeAuto(...)`，C# `EnumerableEncoder` / `DictionaryEncoder` 同样只铺元素，
解码侧都是 `while (span 非空) DecodeAuto(...)`。边界来自外层长度前缀；定长元素由
`Layout::LEN` 从 payload 长度反推个数，变长元素靠自身前缀自分隔。容器 raw payload 与
`Vec<T>` 完全相同。

量级：每类 15–50 行，全部加起来约 300 行实现（不含测试）。真正要小心的是协议层面的
两个问题：

**映射 / 集合不能承诺跨语言字节相等**。C# `Dictionary` 的枚举顺序是哈希序（依赖插入历史与
扩容），C++ `std::map` 是键序，Rust `HashMap` 又带随机种子。三边只能约定「是一个 KV 序列、
能互解」，不能约定「字节相同」。这和 C# string 的 `3n+3` 上界（§12.3）是同一类例外，必须
写进契约，否则 fixture 会造出一条永远不会通过的用例。相对地 `BTreeMap` / `BTreeSet` 是键序，
只要对端也用有序容器就能字节对齐。落地细节与验证见 §19.14。

### 17.4 拆两个 crate 还是留一个

||单 crate + feature|拆 `binary` / `binary-ext`|
|:---|:---|:---|
|孤儿规则|无影响（trait 是本地的）|同样合法|
|feature 合并|cargo 会合并下游所有 requester 的 feature：一个下游开 `std`，另一个也自动得到|无合并问题|
|版本 / MSRV / CI|一套|两套，且要分别承诺|
|先例|`serde`（`std` / `alloc` / `derive`）|`regex` → `regex-syntax` 那种有独立复用价值的才拆|

拆分的唯一硬理由是「ext 需要独立发布节奏 / 独立 license / 不同 MSRV」。本库的扩展 impl 全部
挂在自己的 trait 上、只依赖 `binary` 的两个 trait，没有独立复用价值；而 feature 合并对一个
序列化库是真实风险（下游只想 `no_std`，却被兄弟依赖的 `std` 一起打开）。

### 17.5 结论

1. **不拆 crate**：`binary`（core）+ `binary-derive`（宏）两个包足够，扩展类型留在 `binary`。
2. **实际只有两个 feature**（`std` / `derive`），分三层能力：**无 feature** = `no_std + alloc`
   （`Vec` / `String` / `Box` / `BTreeMap` … 全部无条件编译）、**`std`** = `#![no_std]` 开关
   加 `HashMap` / `HashSet` 的 impl、**`derive`** = proc-macro。初稿写过一个独立的 `alloc`
   feature，但 `Cargo.toml` 从未引入它：`alloc` 是「无 feature 时也成立」的基线，不是一层
   开关。`std` 在 §19.14 落地后才名副其实（此前只 gate 一行 `#![no_std]`）。错误的 trait
   impl 已从 `std::error::Error` 换成 `core::error::Error`（§18.7），两端 `source()` 都可用。
3. **`no_std` 只承诺 `no_std + alloc`**，不承诺裸机。这句要写进 README，避免「支持 no_std」
   被读成「无分配」；无分配是另一个决策，届时加 `alloc` feature 仍然不拆 crate。

## 18. 错误处理：自定义类型的错误如何到达顶层（2026-11-07）

### 18.1 约束：trait 签名里没有位置放 `E`

`Encode::encode` 和 `Decode::decode` 都返回 `binary::Result<T>`，也就是
`Result<T, binary::Error>`。签名里没有类型参数，所以一个自定义 impl 不能
`return Err(MyError::BadVersion)`。这是三个方案比较后的选择，不是疏漏。

### 18.2 三条候选路线

|方案|形态|代价|
|:---|:---|:---|
|泛型错误|`Encode<E = Error>` / `Decode<'de, E = Error>`|每个调用点、每个容器 impl、每段 derive 生成代码都要带 `E`；`Vec<T>` / `Option<T>` / tuple 的 impl 全部要加参数并转发。serde 这么做是因为一个 trait 要服务多种 wire 格式，这里 wire 是固定的，收益为零|
|装箱擦除|`Error::Message(Box<dyn Error + Send + Sync>)`|失去 `PartialEq` / `Eq` / `Clone`，每个错误一次堆分配；`assert_eq!(err, Error::InvalidTag(7))` 这类断言直接写不出来|
|**闭合枚举 + 边界降级**|默认：11 个变体，`Message(Cow<'static, str>)`；可选 `Wrapped(ErasedError)`（§18.7）|默认路径丢失原始错误类型，只剩 `Display` 文本；要类型就显式选 `wrapped`|

采用第三个。判据是「顶层调用方到底需要什么」：需要的是**能 match 的变体**和
**能打印的上下文**，不是原始错误的类型身份。`PartialEq` / `Clone` 是前者的前提，
所以 `Box<dyn Error>` 不能当**唯一**通道。

> 2026-11-07 追加：判据只否掉了「唯一通道」，没否掉「并排变体」。同日加了
> `Error::Wrapped(ErasedError)`，把装箱做成**显式可选项**而不是默认代价，见 §18.7。
> 下表第 3 行因此读作「默认路径」。

### 18.3 三种落地写法

**（a）`From` + `?`：impl 内部降级。** `impl From<MyError> for binary::Error` 在
orphan rule 下合法（`MyError` 是本地的），于是 impl 里可以直接 `?`：

```rust
impl From<OutOfRange> for Error {
    fn from(e: OutOfRange) -> Self { Error::message(e) }
}

impl Encode for Count {
    fn encode(&self, e: &mut Encoder<'_>) -> Result<()> {
        check_count(self.0)?;   // OutOfRange -> Error::Message
        e.write_byte(self.0)
    }
}
```

**（b）就地降级。** 没有自己的错误类型时直接
`map_err(Error::message)` / `Error::message_static("...")`。

**（c）外层包装：需要保留原始错误时。** 错误穿不过 trait，但可以在调用边界重建：

```rust
enum AppError { Wire(binary::Error), Mine(OutOfRange) }
let v = MyType::decode(&mut d).map_err(AppError::Wire)?;
```

**（d）`Error::wrapped`：让类型自己穿过去（§18.7）。** 降级时选 `wrapped` 而不是
`custom`，调用方拿 `downcast_ref` 就能把 `OutOfRange` 取回来，外层 enum 只剩「区分 wire 与
领域」的作用。

**由此得出一条 API 设计规则**：impl 里只做 wire 转换，领域校验放到外层。§18.7 之后这条
规则收窄成——**降级必须显式选**：默认的 `custom` 丢掉类型，`wrapped` 保留类型但付一次分配，
不选就等于选了「只剩字符串」。

encode 与 decode 两侧完全对称，没有单独的机制。

### 18.4 `Message` 从 `&'static str` 改成 `Cow<'static, str>`

旧类型连动态消息都放不下：只能写 `Error::Message("bad version")`，把 `9` 丢掉。
现在：

- `Error::message_static(&'static str)` → `Cow::Borrowed`，**不分配**，crate 自己的路径和
  derive 生成代码都用它；
- `Error::message(impl Display)` → `Cow::Owned(format!("{msg}"))`，一次分配，用于外部错误。

`PartialEq` 对 `Cow` 比较的是内容，所以 `Message(Owned("x")) == Message(Borrowed("x"))`——
构造方式不影响断言，这一点也有测试。

`core::error::Error::source()` 对 `Message` 返回 `None`（payload 是消息，不是 boxed error），
对 `Wrapped` 返回内层（§18.7）；trait impl 本身已不挂 `std` feature，见 §18.7.4。

### 18.5 位置：不放进 `Error`，靠游标折算

把 `offset` 放进错误要么把 enum 改成 struct，要么给 11 个变体各加字段，
现有全部 `assert_eq!(err, Error::X)` 都会失效；何况长度前缀作用域里同时存在
「payload 内偏移」和「绝对偏移」两套坐标，存哪个都要额外约定。

改成让 `Decoder` 的游标在失败后仍然有意义——但**不导出绝对位置**，只导出「还剩多少」：

- `take` 失败不改游标，停在**请求起点**；
- `with_prefix` 失败时把子游标的相对位置折回父坐标（`self.pos = start + sub.pos`）；
  每层都折，所以最外层拿 `buf.len() - d.remaining()` 就是失败字节的绝对偏移；
- 成功路径不受影响（仍然 `self.pos = end`）。

`position()` 一度存在，后来删了（2026-11-07）：它在顶层是绝对值、在 `with_prefix` /
named payload 里是作用域相对值，同一个名字两种含义；而它的信息从来不是私有的——持有原始
buffer 的调用方一次减法就能还原。C#（`ref ReadOnlySpan<byte>`）和 C++（`std::span`）把
cursor 放在 span 里，"我在哪" 天生是 `total - remaining`，连一个位置方法都不需要。

**已知缺口**：`NamedMembers::value` 用 detached payload 建 `Decoder::new(payload)`，
子游标没有折回，derive 生成的 named object 成员解码失败时，报出的偏移是「成员内坐标」，
外层 `remaining()` 不随成员前进。逃生口是自定义 impl 自己拼相对距离：

```rust
let start = d.remaining();
let n = d.take_byte()?;
if n > 4 {
    return Err(Error::message(format_args!(
        "count {n} out of range {} byte(s) into the value",
        start - d.remaining()
    )));
}
```

### 18.6 用例（`crates/binary-tests/tests/errors.rs`，15 个）

|用例|钉住的性质|
|:---|:---|
|`test_from_impl_makes_question_mark_work`|`From` + `?` 在 encode / decode 两侧都产生 `Error::Message`，消息是 `Display` 输出|
|`test_message_shape_static_vs_allocated`|`message` → `Cow::Owned`、`message_static` → `Cow::Borrowed`，且两者相等|
|`test_degraded_error_cannot_be_recovered`|`message` 降级后只能 match 到 `Message`；外层包装能区分两侧|
|`test_failure_offset_folds_through_nested_prefixes`|两层 `with_prefix` 内失败，`buf.len() - d.remaining()` = 3（buffer 末尾）|
|`test_offset_reported_from_inside_a_custom_impl`|impl 内用 `start - d.remaining()` 报相对距离，调用方一次减法得绝对偏移|
|`test_wrapped_error_keeps_type_and_source_chain`|`wrapped` 后 `Display` 委派给载荷、`source()` 可取回载荷、载荷自身的 chain（`BadFrame → Checksum`）仍通、`downcast_ref` 拿到原类型、`Error` 保持 `Send + Sync + 'static`、`clone` 后仍能 downcast|
|`test_wrapped_equality_is_textual`|两个无关类型文本相同即相等（`BadFrame(7)` == `Impostor`）；同类型不同载荷不等；`Wrapped` 与 `Message` 不互相相等；`Message` 仍按内容比较|
|`test_wrapped_error_crosses_the_trait_boundary`|`From<BadFrame> for Error` 用 `wrapped`，impl 内 `?` 之后调用方仍能 downcast 到 `BadFrame(2)`，且正常路径不受影响|

上表是降级路径的 8 个；另外 7 个（`test_truncated_payload_is_not_a_short_read` 等）钉的是
变体语义而非降级路径，列在 §18.9.4。

`error.rs` 上还有 4 个 doc test（`Error::message` 的 `From` 示例与位置示例、
`Error::wrapped` 的 `?` + downcast 示例、`ErasedError::inner` 的取回示例）。

### 18.7 追加：`Wrapped` 变体，把装箱做成可选项（同日晚）

"custom 不能是 `dyn Error` 吗"——能，而且不必牺牲已有的派生。做法不是把 `Message` 改成
`Box<dyn Error>`，而是**并排加一个变体**，让调用方在降级点显式选择付不付这笔代价。

#### 18.7.1 形态

```rust
pub struct ErasedError(Arc<dyn core::error::Error + Send + Sync + 'static>);   // 16 字节

pub enum Error { …, Message(Cow<'static, str>), Wrapped(ErasedError) }

impl Error {
    pub fn wrapped(err: impl core::error::Error + Send + Sync + 'static) -> Self { … }
}
```

`Error` 仍是 24 字节：`Wrapped` 载荷 16 < `Cow<str>` 载荷 24，枚举大小由最大变体决定，
加变体没有让错误值变胖。`match` 只有 `Display` 一处是穷尽的，编译器强制补分支；
`source()` 与 `PartialEq` 都不用改结构。

#### 18.7.2 为什么是 `ErasedError` 这个 newtype，而不是裸 `Arc`

裸 `Arc<dyn Error>` 不能比较，而 `Error` 必须留 `PartialEq`（否则 116 处
`assert_eq!(err, Error::X)` 全部失效）。两条实现路径：

|写法|后果|
|:---|:---|
|给 `Error` 手写 `PartialEq`：`match (self, other) { … _ => false }`|能编译，但**新增变体时静默错误**：新变体与自身比较会落到 `_ => false`。这是延迟爆炸的 bug，不是风格问题|
|把「怎么比」封进 `ErasedError`，`Error` 继续 `derive(PartialEq, Eq, Clone, Debug)`|新增变体自动正确；奇怪语义只存在于 16 行之内，且有独立文档与测试|

选后者。`ErasedError` 公开（`binary::ErasedError`），因为它是公开变体的字段类型，
藏起来只会在下游产生「无法命名」的报错。

#### 18.7.3 相等是文本相等——这是唯一的真代价

`dyn core::error::Error` 不暴露 `type_id`（`Any::type_id` 不可达），所以没有别的可比项。
`ErasedError::eq` 比 `Display` 输出：

- `Error::wrapped(A) == Error::wrapped(B)` 只要两者打印相同文本就成立，**即使 A 与 B
  是无关类型、或 A 的字段差异没进 `Display`**；
- 同类型不同载荷通常不等（因为消息里有区别），但若 `Display` 不区分字段，也会误判相等；
- 变体之间不串：`Wrapped` 永不等于 `Message`，哪怕文本一模一样；
- 每次比较都会格式化一次（两侧各一次分配）。错误路径上无妨，热路径别拿它做分支条件。

`test_wrapped_equality_is_textual` 把这四条都钉住，包括「`Impostor` 冒充 `BadFrame(7)`
也相等」这个不体面但必须承认的结论。

#### 18.7.4 顺带的三处调整

1. **trait impl 从 `std` 移到 `core`**：`impl core::error::Error for Error` 不再挂
   `feature = "std"`。1.81 起 `std::error::Error` 就是 `core::error::Error` 的再导出，
   一份 impl 同时满足 `std` 与 `--no-default-features`，`source()` 因此在 `no_std` 下也可用。
   `Wrapped` 的 bound 也必须是 `core::error::Error`，否则 no_std 下无法构造。
2. **`Send + Sync + 'static`**：为的是 `Error` 自己保持 `Send + Sync`（跨线程搬运、塞进
   `anyhow` 形状的上下文都依赖它）。代价是不接受 `!Send` 的 source（含 `Rc`、裸指针的
   错误）——那种情况只能用 `custom` 降级成消息。
3. **声明 MSRV**：`rust-version = "1.87"`。1.81 是 `core::error::Error` 的要求，1.87 是被
   `container.rs` 的 `usize::is_multiple_of` 顶上去的——写了 `rust-version` 之后
   clippy 的 `incompatible_msrv` 会主动盯住这类滑移，这正是声明它的收益。

#### 18.7.5 三档怎么选

|场景|用法|代价|
|:---|:---|:---|
|crate 内部、derive 生成代码、常量消息|`Error::message_static("…")`|零分配|
|需要运行时细节（版本号、长度、偏移）|`Error::message(impl Display)`|一次 `String`，类型丢失|
|调用方要按错误**种类**分支、或要打印完整 chain|`Error::wrapped(impl core::error::Error + Send + Sync)`|一次 `Arc` + 文本相等|

默认仍是 `custom`：错误路径的常见需求是「打出来给人看」，`wrapped` 的分配与文本相等都不该
让所有人一起付。

### 18.8 决策记录：保留闭合枚举，整体擦除挂到 benchmark 之后（2026-11-07）

决策人是用户，写在这里是为了让「为什么现在不动」和「什么条件下会动」都可查，不用三个月后
靠回忆重推一遍。

#### 18.8.1 两层理由：一层现在就能裁，一层必须实测

「把整个错误模型换成 `dyn Error`」的理由分属两层，性质完全不同：

|层|论点|能否现在裁定|
|:---|:---|:---|
|可观测性|变体可 match、可 `assert_eq!`，`InvalidTag(u32)` 与 `MissingMember(&str)` 有结构化出口；擦除后只剩 `Display` 文本，14 处预定义构造点各写一次 `format!`（现状是零分配的变体或常量）|能。这是契约问题，与性能无关|
|代价|现状「错误大多零分配」vs 擦除后「每次失败一次 `Box`」，以及 `?` 传播会不会重装箱|当时不能，**同日已实测**：失败路径 ~5.5 ns + 1 次 16 B 分配，成功路径零差异（§19.3）|

本轮只裁第一层：**保持现状**。第二层不裁——拿「感觉不贵」当理由会留下一个无法复核的结论。

**判据先写死，防止事后找理由**：

- 若实测显示错误路径开销可忽略（失败不在任何循环里、单次失败分配量小于一次 `format!`）→
  代价侧不再成立，决策只剩「可观测性 vs 向后兼容」，必须重新权衡，**不能再用「零分配」挡回去**；
- 若错误路径将来变成热点（试探式解码、named 协议重复解析同一 payload）→ 现状胜出，该候选作废，
  并把结论写回本节。

#### 18.8.2 为什么 C#/C++ 的「异常统一接口」不能照搬

C#/C++ 里任何类型都能抛自己的异常，**接口签名不变**，「通用优于私有」由异常机制免费提供。
Rust 没有异常，错误是返回类型的一部分：`decode` 只能有**一个**错误类型，「通用」必须买在别处。
本 crate 买在「通道固定 + 边界降级」：

- 通道两条：`Message` 走文本，`Wrapped` 走类型（`downcast` 可取回）；
- 预定义变体与类型自有错误**没有等级差别**——它们都只是通道的一端，`MissingMember` 不比用户
  的自定义错误更「官方」（用户观点，且正确）；
- 代价是「通用」只到通道这一层：Rust 做不到「任何错误类型都自动穿过 `decode`」，除非整体擦除
  （§18.8.5 的等价物）。

#### 18.8.3 准入规则：预定义变体不是免费添加的

此后每加一个变体，必须同时满足四条（进 review checklist）：

1. **可达**：至少一个构造点，且在非 `#[cfg(test)]` 代码里。零构造点的变体是纯 API 负担。
2. **可分支**：有调用方会据此改变控制流。否则用 `Message` / `Wrapped`。
3. **自解释**：名字单独成立，不靠相邻变体解释；成对的语义用成对的词。
4. **可演进**：新语义默认走 `Message`；确需变体时靠 `#[non_exhaustive]` 消化（§18.8.5）。

#### 18.8.4 审计：11 个变体，命名问题已裁决（§18.9）

变体集合在 0.1 冻结前做过一次完整审计，结论与改名方案写进 §18.9。下表是**修正后**的状态；
「core 内构造点」一列就是「可达」这条规则的证据——除 `Message` / `Wrapped` 由调用方构造，
其余每个变体都有真实构造点。

|变体|core 内构造点|语义|
|:---|:---|:---|
|`BufferFull`|`Encoder::ensure_capacity`|写借用 buffer 越界（encode 侧，与 `NotEnoughBytes` 配对）|
|`NotEnoughBytes`|6 处（`Decoder::take` / `take_byte`、`length::encode_prefix`）|读不足（decode 侧）|
|`LengthTooLarge`|`Encoder::write_prefixed_bounded` / `write_prefixed` ×2|长度前缀超 `i32::MAX`|
|`MissingMember(&'static str)`|`NamedMembers::require`|named 协议缺成员|
|`DuplicateMember`|`NamedMembers::parse`|named 协议重复成员|
|`InvalidMemberIndex`|`NamedMembers::parse`|名字查表返回的索引越界（内部不变量）|
|`InvalidTag(u32)`|`Option<T>::decode` / `decode_auto` ×2、derive 生成代码|tag 非法（`Option` 与 union）|
|`PayloadTruncated`|`Decoder::with_prefix`|前缀声明的负载超出可用字节|
|`MisalignedElements`|`<Vec<T> as Decode>::decode`|剩余字节不是元素大小的整数倍|
|`Message(Cow<'static, str>)`|crate + derive + 用户|文本通道|
|`Wrapped(ErasedError)`|用户|类型通道|

三个变体在这一轮里从「可疑」变成「确定」：

1. **`LengthTooLarge` 从死变体变成真正的边界检查。** 原来它零构造点，真正卡 `i32::MAX` 的是
   `length.rs` 的 `debug_assert!`：debug 下 panic，**release 下 `encode_prefix_wide` 照写
   `(len as u32) | 0x8000_0000`，解码侧 `& 0x7FFF_FFFF` 得到错长度——静默写坏 wire**。
   现在 `write_prefixed_bounded` 在写任何字节之前返回 `Err(LengthTooLarge)`；`write_prefixed` 在负载已经
   写完之后返回同一个错误（编码器状态因此不确定，但调用方一律把错误当终止符，文档已写明）。
   2026-11-07 收尾：`encode_prefix` / `encode_prefix_wide` 本身也改成 `Result<usize>`，
   `debug_assert!` 全部删除，最底层不再留任何「release 下静默截断」的路径；`write_compact_int`
   同源的 `debug_assert!` 一并换成 `LengthTooLarge`（§9.7 的 API 审计表）。
2. **`LengthPrefixMismatch` 拆成两个。** 它同时承担「前缀声明多于可用字节」（截断）与
   「剩余字节不是元素大小的整数倍」（错位）。两者对调用方含义不同：截断可以等更多数据再试，
   错位说明数据本身不合法，重试没有意义。
3. **`InvalidMemberIndex` 改名，因为原名在说反话。** 未知成员名在 `NamedMembers::parse` 里被
   `continue` **有意跳过**（前向兼容），从不报错；唯一触发点是 `find` 返回了 `>= N` 的索引，
   那是生成代码与常量 `N` 不一致，属于内部不变量。

#### 18.8.5 用 `#[non_exhaustive]` 换「加变体不 breaking」

「全 `dyn Error` 最大化向后兼容」这个判断是对的：调用方本来就不能 match，库可以任意增删内部
错误。但它买的是**永远不 breaking**，付的是**永远不能 match**。同一条路上有个便宜得多的中间点：

|方案|加变体|下游能否 match|成本|
|:---|:---|:---|:---|
|现状（公开 enum）|**breaking**|能，可穷尽|—|
|加 `#[non_exhaustive]`|minor|能，必须带 `_`|一行|
|全 `dyn Error`|永不 breaking|只能 `downcast` + `to_string()`|每次失败一次分配，丢 `PartialEq`|

`#[non_exhaustive]` 的三条事实要说清：

1. 只限制**外部穷尽 match**，不限制外部**构造** unit 变体，所以 `assert_eq!(err, Error::NotEnoughBytes)`
   在测试里照旧（测试 crate 也是「下游」，但它不做穷尽 match）；
2. 本 crate 内部不受影响，`Display` 的穷尽 match 仍在编译期受检；
3. 代价是下游失去「编译器保证已处理所有错误」——对开放错误集合，这本来就是错觉。

补一条边界：属性管的是**错误变体集合的开放性**，和 §1 的 wire 前向兼容无关。早先拿
`TooNew` / `Versioned` 当例子，是把两件事混在了一起（§1 末已订正）：例子错，属性对。

**它管什么、不管什么**（用两个 crate 实测，Rust 1.8x：一个 `enum Open {A,B}`，一个
`#[non_exhaustive] enum Closed {A,B}`，各版本重新编译下游）：

|库侧动作|`Open` 下游|`Closed` 下游|
|:---|:---|:---|
|写穷尽 `match`（无 `_`）|通过|**E0004 `_` not covered**|
|加变体 `C`|**E0004**，必须改代码|通过，新变体落进 `_`|
|把 `A` 改名成 `Alpha`|E0599|**E0599，属性不保护**|
|删掉一个下游提过的变体|E0599|E0599|
|`assert_eq!(e, Closed::B)` / `matches!` / 构造已有变体|—|**照旧可用**|

一句话：**它只买「加变体不 breaking」，别的一分不保**。最后一行是关键——正是因为
unit 变体仍可构造，`binary-tests`（独立 crate，也算下游）里 71 处 `Error::X` 一处都不用改。
实测把属性临时加到 `Error` 上跑全量：**87 passed，bench 也照常编译**，改动面就是那一行。

错误集合还会长（§9.1 的容器要加重复键一类），所以**已在 0.1 冻结前加上**：

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error { … }
```

**不为它写 fixture。** 起初加过一条 `tests/ui/error_non_exhaustive.rs`（下游穷尽 match、
钉 E0004），已删除：那条快照钉的是**语言规则**而不是本库的诊断，`#[non_exhaustive]` 的语义
由 rustc 保证，再拿 trybuild 守一遍属于重复；而它一旦因为变体增删而需要重生成，维护成本
会落在 `tests/ui/` 那份「派生宏诊断」清单上，把两类东西混在一起。属性被误删就误删了——
`cargo doc` 和 review 看得见，不值得为它加装置。

#### 18.8.6 待办清单

|项|类型|状态|
|:---|:---|:---|
|变体命名修正（6 条规则、5 处改名、1 处拆分、1 处接线）|代码 + 文档 + 测试|**已完成**（§18.9）|
|`#[non_exhaustive]`|代码（一行）|**已完成**（2026-11-07）：属性在 `Error` 上，不加 fixture（理由见 §18.8.5）|
|整体换 `dyn Error`|架构|**benchmark 已跑（§19.3）**：代价可忽略，结论见 §18.8.7|

### 18.8.7 事后补记（同日，benchmark 已跑）

§19.3 的实测结果推翻了 §18.8.1 里第二层的判据本身：擦除的代价只有 **~5.5 ns + 1 次分配/失败**，
成功路径零差异。按预先写死的规则，这意味着「零分配」不再是保留 enum 的理由。结论不变，
但理由重写为：

1. **可观测性**（可 match、`InvalidTag(u32)` 与 `MissingMember(&str)` 有结构化出口）——
   这是现在唯一还站得住的正面理由；
2. **兼容性应该用 `#[non_exhaustive]` 买，而不是用擦除买**：前者一行、保住 match；
   后者每次失败分配 + 文本相等（§18.8.5 的三方案表）。

换句话说：benchmark 把「枚举很便宜」从**论据**降级成**事实**，而事实不构成理由；
真正的理由是可断言性。

### 18.9 错误变体命名（0.1 冻结前的一次性修正）

命名不是风格问题。审计发现**三个名字在陈述错误的事实**：`UnknownMember` 只在索引越界时触发
（未知名字是被跳过的），`LengthPrefixMismatch` 同时指「截断」和「错位」，`BoxedError` 装的是
`Arc`。这比「不够优雅」严重——按名字写的 match 分支会走到错误的一侧。

#### 18.9.1 六条规则

|#|规则|反例（修正前）|
|:---|:---|:---|
|1|说**症状**，不猜动机|`Overflow` 与算术溢出同名，看不出是「缓冲写满」|
|2|**一个变体一个故障**|`LengthPrefixMismatch` 一名两义|
|3|成对错误共用一根轴|`Overflow` / `NotEnoughBytes` 不成对|
|4|不借 std 方法名表达别的语义|`Source` 是 `source()` 的返回物，变体却是**本体**|
|5|名字里不出现使用者等级|`Custom` 读作「用户自定义 vs 内置」，与「错误无高低」相悖|
|6|名字与 `Display` 文本互为翻译|三处名不副实，见 §18.9.2 末列|

规则 6 是其中唯一可机械检查的一条：七个无载荷变体的 `Display` 首词必须就是变体名的直译。
`test_display_text_matches_the_variant_name` 钉住它，新增变体时若只改一边，测试会红。

#### 18.9.2 改名对照

|旧|新|原因|旧名与 `Display` 的对齐|
|:---|:---|:---|:---|
|`Overflow`|`BufferFull`|与算术溢出撞名；且它与 `NotEnoughBytes` 是写/读一对（规则 1、3）|✗ 打印 "buffer overflow"|
|`Custom(Cow)`|`Message(Cow)`|去掉「用户 vs 内置」等级暗示；与 `Wrapped` 构成文本/类型两条通道（规则 5）|— 委派|
|`Source(BoxedError)`|`Wrapped(ErasedError)`|`source()` 的返回物才叫 source，这个变体是**被包装的本体**（规则 4）|— 委派|
|`UnknownMember`|`InvalidMemberIndex`|未知名字从不报错；触发点是索引越界（规则 1）|✗ 打印 "unknown member"|
|`LengthPrefixMismatch`|`PayloadTruncated` + `MisalignedElements`|一名两义（规则 2）|✗ 打印 "does not match"|
|`BoxedError`|`ErasedError`|newtype 里装的是 `Arc`，不是 `Box`|— 无 `Display`|

构造器同步：`custom` → `message`、`custom_static` → `message_static`；`wrapped` 名称不变，
于是「变体名 = 构造器名 = 语义」三角对齐。

`custom` 这个名字的取舍值得单独记一笔：serde 的 `Error::custom` 是生态固定词，保留它更好搜。
但这里是**变体 + 构造器**的组合，保留旧构造器会留下 `Error::custom()` 造出 `Error::Message`
的错位，比失去检索熟悉度更糟。0.1 未发布，没有迁移成本。

#### 18.9.3 顺带修掉的结构问题

改名过程中有两件事必须一起做，否则名字改了、行为还是错的：

1. **`LengthTooLarge` 接线**（§18.8.4 第 1 条）：`write_prefixed_bounded` 入口返回错误，
   `write_prefixed` 在写完负载后返回错误。前者是零代价的（一次比较，分支可预测），
   后者只可能在 >2 GiB 的消息上发生。
2. **`LengthPrefixMismatch` 拆两个**：`decoder.rs` 的「前缀声明的负载超出缓冲区」是
   `PayloadTruncated`；`container.rs` 的「剩余字节不是元素大小整数倍」是 `MisalignedElements`。

#### 18.9.4 测试怎么钉住语义

`crates/binary-tests/tests/errors.rs` 末尾新增一节，每个用例都对应一条「旧名字会给出错误答案」
的语义：

|用例|钉住的性质|
|:---|:---|
|`test_truncated_payload_is_not_a_short_read`|同一位置上「头被截断」→ `NotEnoughBytes`、「负载被截断」→ `PayloadTruncated`，可分别重试|
|`test_misaligned_fixed_size_elements`|6 字节的 `Vec<i32>` → `MisalignedElements`；8 字节同一路径正常解码|
|`test_length_beyond_i32_max_is_reported_before_writing`|`write_prefixed_bounded(MAX+1)` 返回 `LengthTooLarge` 且 `e.len() == 0`（守卫在写入之前）|
|`test_compact_int_beyond_i32_max_is_rejected`|`write_compact_int(i32::MAX+1)` 返回 `LengthTooLarge` 且不写字节；`i32::MAX` 仍编码成 `FF FF FF FF` 并回读一致|
|`test_unknown_name_is_skipped_but_bad_index_is_an_error`|未知名字 → `Ok`（前向兼容）；`find` 返回越界索引 → `InvalidMemberIndex`|
|`test_display_text_matches_the_variant_name`|规则 6 的机械检查|

`tests/wire.rs` 的 `test_borrowed_encoder_overflow` 同时改名为 `test_borrowed_encoder_full`，并把
`is_err()` 换成 `assert_eq!(err, Error::BufferFull)`——改名若只改拼写不改断言，等于没改。

#### 18.9.5 顺序：先改名，后 `#[non_exhaustive]`

两件事都必须在 **0.1 冻结前**做完，但**属性不保护改名**——这一点容易被误读，写清：

|动作|加了属性之后对下游|
|:---|:---|
|加变体|不破坏（新变体会落进下游的 `_`）|
|**改名**变体|仍然 `E0599`（下游写过旧名就报错）|
|**删除**变体|仍然 `E0599`（同上）|
|改变体的载荷类型|仍然破坏|

所以「先改名」不是为了让属性生效，而是因为**两件事各自都是一次 breaking**：0.1 未发布、
无下游、无 commit，现在做都是免费的，合并进同一个窗口最省事。属性加得越晚，已经写好穷尽
`match` 的下游越多，那一次 breaking 的波及面越大。

反过来「先加属性、再改名」并不会失败，只是把两次 breaking 拆成两次发布——在还没有下游的
今天没有区别，等有了下游就成了两个大版本。

动作顺序写死为：**命名修正 → `#[non_exhaustive]` → commit**。属性落地后守一条规矩：
**只加变体，不改不删**；改名与删除等于把 §18.9 这次重新付一遍。


---

## 19. 首批 benchmark 结果（2026-11-07）

### 19.1 装置与不对称之处

|边|代码|口径|
|:---|:---|:---|
|Rust|`crates/binary-bench`|手写 harness：`min` over 7 轮，每轮 0.05 s 预算，`black_box` 结果；`cargo run --release`，另加 `CARGO_PROFILE_RELEASE_LTO=fat CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1` 对齐 C++ 的 IPO|
|C#|`Benchmarks.IntegrationTests`（C# 仓库）|正式项目用 BenchmarkDotNet，内置 JIT 与 source-generated AOT 两个 generator；§19.2 的同表对照另用一份同测量规则的 JIT harness（`Stopwatch`，min over 7 轮 × 50 ms，.NET 11 preview，TieredPGO）|
|C++|`binary-benchmarks` 的 integration-tests 目标|google benchmark，Release + IPO（`-O3` + LTO），系统 `libbenchmark` 是 debug 构建（只影响 harness 循环，不影响被测代码）|

三边编的是**同一份 payload**：`TypeX1 = (int, string, int[], (double, List<string>))`，值
`(1024, "csharp", [7,11,555,1313], (2.2, ["one","two","three"]))`；named 侧是
`Type01 { Id, Name, List, Item{Data, Tags} }`。C# 仓库的 `Benchmarks.IntegrationTests` 用的
正是这两个类型（`TypeX1` 与 `Type01`），所以三边 payload 不是临时凑的；但该项目是
BenchmarkDotNet 口径，与 §19.2 的 `min over rounds` 不同，故不并入同表 —— AOT 变体见 §19.6。

机器：AMD Ryzen 7 5800X，WSL2，**未绑核**，单次运行。绝对值只当量级看，横向比例更可靠。

### 19.2 三边对照（ns/op）

|操作|Rust|C#（JIT）|C++ virtual|C++ template|
|:---|---:|---:|---:|---:|
|encode tuple（raw，50 B）|23.3|36.3|26.7|17.7|
|encode tuple（auto，51 B）|27.9|39.5|—|—|
|decode tuple|94.3（raw）/ 96.0（auto）|76.5|96.0|83.0|
|encode named（raw，85 B）|53.1|62.8|50.5|40.8|
|decode named（auto，89 B）|108.1|119.7|141|126|
|encode `i32`（raw）|0.44|7.8|—|—|
|decode `i32`（raw）|0.65|8.1|—|—|

读法：

- **同一量级**，既没有「十倍慢」也没有「反超」；`i32` 单值 0.44 ns 说明 Rust 侧 harness
  没有把开销算进被测代码（C# 的 7.8 ns 是虚拟调用 + 委托，属于另一条路径）。
- **encode 比 C++ template 慢 20–30%**（23.3 vs 17.7；53.1 vs 40.8）。第一嫌疑是
  `Encoder` 的 `Inner` 枚举在每次 `write` 都要判 Owned/Borrowed，而 template 版是编译期
  展开成裸 span 写；第二嫌疑是每层 `?` 的 `Result` 检查。这两条都要改结构，不进本轮。
- **named decode 反而是 Rust 最快**（108 vs C++ 126/141、C# 120）：`NamedMembers` 对 3 个
  成员做线性名字比较，比 C# 的委托链与 C++ 的 `type_index`/descriptor 遍历便宜。
- **C# 的 tuple decode 最快**（76.5）：`SpanLikeMethods.GetPartialList` 用 32 元素栈上
  `InlineBuffer` 先接、再一次性 `SetCount`，没有 push 扩容；Rust/C++ 都是 push。
  这是 §19.10 整块解码的第一份数据依据。

### 19.3 错误模型：enum vs `Box<dyn Error + Send + Sync>`

|项目|enum|dyn|
|:---|---:|---:|
|`size_of::<Result<i32, _>>()`|24|16|
|构造（静态消息）|0.43 ns|5.96 ns|
|构造（动态消息）|37.0 ns（`custom(format_args!)`）|29.0 ns（`Box::new(MsgError(format!))`）|
|传播 3 层（最内层失败）|3.93 ns|8.63 ns|
|边界转换 `enum → dyn`|—|15.1 ns|
|成功路径：decode 256 × `i32`|289.9 ns|289.2 ns|
|`to_string()`|19.7 ns|15.9 ns|
|每次失败分配|0 次 / 0 B|1 次 / 16 B（带消息：2 次 / 48 B）|

结论：

1. **成功路径完全无差**（289.9 vs 289.2）。「`Result` 从 24 字节缩到 16 字节」不存在可测收益，
   这一条从此不能当理由。
2. **失败路径 ≈ 5.5 ns + 1 次分配**，与一次 `format!`（37 ns）同量级，且只在错误路径上发生。
3. 于是 §18.8.1 的第二层判据被推翻：「零分配」不再能支撑保留 enum 的结论。
   结论本身保留，理由改为可观测性 + `#[non_exhaustive]`（§18.8.7）。
4. 反过来看，`wrapped`（`Arc`，1 次 32 B 分配）比 `dyn` 的 `Box`（16 B）贵一倍，
   但换来了 `Clone` 与 `PartialEq`——这个交换在数据下依然是划算的。

### 19.4 named object 的成员顺序不属于 wire 契约（2026-11-08 定型）

同一个 `Type01` 值，三边写出来的顺序不同：

- **C#** 按**名字序**写：`Id, Item, List, Name`；
- **Rust** 按**声明序**写：`Id, Name, List, Item`；
- **C++** 虚拟 API 按 `MemberDescriptor` 顺序（声明序），template API 对聚合类型直接产出
  tuple 形式（无名字）。

三者互相都能解码，长度也一致（raw 85 / auto 89）。C# 那份排序**只是为了让 encode 输出稳定、
测试好对**，不是格式要求 —— 两边 README 原话都是这个意思：

- C#：*"Members are written in name order to keep the output deterministic (the tests rely on
  it), but the format does not require any order: decoding looks members up by name."*
- C++：*"Members are written in the order declared by the schema, but the format does not
  require any order: decoding looks members up by name."*

所以这件事**不是待办**，也不再讨论「要不要对齐字典序」：

- 不做「derive 展开期按名字排序」：那会改掉 Rust 的 named wire（现有 named 断言与
  `Type01` 的 85 字节全部重算），换来的只是与 C# 字节相同，而字节相同本身不是契约。
- §1 已写明 named 解码按名字查表，不需要再加条款。**Rust 保持声明序**（与 C++ 一致）。
- 跨语言对照 named 时，只断言「另一方解出同一组值」，**不比字节**；`tuple` / tuple object
  的顺序仍然是契约（位置即语义），那两类继续要求逐字节一致。

这条以前被记成「覆盖缺口 + 两个选项、倾向排序」，是把「测试装置要稳定」误当成 wire 要求。

### 19.5 复现

以下命令复现的是各仓库**正式**的 benchmark 项目；§19.2 的三边同口径数字另由一份与 Rust
测量规则一致的 JIT harness 产生（口径见 §19.1），它依赖外部 checkout，不入库。

```bash
# Rust（在仓库根执行；不带参数就是 wire，即三边同 payload 的 integration 组）
./scripts/run-benchmark.sh
./scripts/run-benchmark.sh errors

# C#（在 C# 仓库执行，.NET 10+ SDK）
dotnet run -c Release --project <project-dir>/Benchmarks.IntegrationTests

# C++（在 C++ 仓库执行；脚本自己 configure + build，再跑 integration 目标）
./scripts/run-benchmark.sh integration
```

一次性、依赖外部 checkout 的 harness 与 fixture 生成程序都不入库：它们只在装有对应仓库的
机器上可构建（§12.5 讲过同样的理由），机器本地的临时内容统一放在被忽略的缓存目录下。
需要长期保留的是本节的数字与口径、`crates/binary-bench` 的源码与 `csharp.tsv`；C# 侧正式
可复现的装置是 `Benchmarks.IntegrationTests`。

### 19.6 未覆盖

- C# **AOT** 未并入 §19.2 的三边同口径表：C# 仓库的 `Benchmarks.IntegrationTests` 已用
  `[SourceGeneratorContext]` 接好 source generator，并同时测 `Generator.CreateDefault()`
  与 `CreateAotBuilder()` 两个 generator；但它是 BenchmarkDotNet 口径，本轮三边同表只跑 JIT；
- C++ 的 union / enum / Option 场景（其 benchmark 只有 tuple 与 named 两类）；
- 分配次数只测了错误模型；wire 路径的分配（解码出的 `String`/`Vec`）只在 C# 侧有字节数
  （tuple 256 B、named 336 B）；
- **字节类型那一组还没加**（2026-11-08）：现有行仍是 `String` / `std::string`，两边都分配、
  都 memcpy，但 Rust 多一次 `str::from_utf8` 校验、C++ 不校验。按 §19.15.1 的口径要换成
  `&[u8]` / `std::string` 当 bytes / `byte[]`；借用视图（`&[u8]` / `std::string_view` /
  `ReadOnlyMemory<byte>`）是**另一组行**，零拷贝零校验，不能和拥有型同列比，也不在计划内。
  **这两笔差价已量化**：94.1 ns 的 owned `decode tuple` 里 31.0 ns 是 4 次 `String` 分配、
  29.3 ns 是 4 次 UTF-8 校验（§19.15.16）；
- **owned 行只有 `new()`（256 字节种子），没有 `with_capacity(上界)` 行**（2026-11-08）：
  两者在 128 字节以内同速，1 KiB 上差 1.9×（§19.15.15）。矩阵里的 owned 负载都 ≤ 85 字节，
  所以现有比值不受影响；要测大包再单独加行，别把 `new()` 的数字读成「预分配没用」；
- C++ google benchmark 的 DEBUG 构建**不再是阻塞项**：§19.15 用测量规则相同的自有 harness
  （§19.15.2 装置），不依赖那套 CMake 的默认配置；
- 单次运行 + 未绑核 → 5–10% 的波动是正常的，跨语言比较请按「同量级 / 差 20–30%」读。

这一节记的是矩阵的**适用范围**，不是待办清单；口径在 §19.15.1，要加场景照 §19.15.2 的装置
加行即可，口径不变。

### 19.7 raw 与 auto 不能混着比

同一份 tuple：C# raw = 50 B，C++ raw = 50 B，Rust raw = 50 B，三者逐字节相同；auto 都是
51 B（多出来的 `0e` 是内层 `Vec<string>` 的长度）。第一版 harness 拿 C# 的 raw 去比 Rust 的
auto，看起来像「Rust 多写了一个前缀」——其实只是口径不同（§19.4 的 named 顺序问题也顺带在
这次对齐里暴露出来）。

结论：**benchmark 必须声明量的是 raw 还是 auto**，否则一个字节的差别就能推出相反结论。
§19.2 因此两种口径都列了出来。此后 benchmark 只报 raw 一行（§19.15.2）：auto 与 raw 的字节
关系由单元测试钉住，多报一条时机只会在噪声里挑数字。

### 19.8 复跑（2026-11-07 晚，同一台机器同一时段）

Rust 侧（`cargo run --release -p binary-bench -- wire`，连跑三次取范围）与 C++ 侧
（C++ `binary-benchmarks` 的 integration-tests 目标，-O3）在同一次会话里跑：

|用例|Rust|C++ virtual|C++ template|
|:---|---:|---:|---:|
|encode tuple（raw / 对应 C++ `Encode`）|28.5 – 29.1|28.8|18.1|
|decode tuple|102 – 105|108|93.2|
|encode named|63.8 – 66.8|49.3|39.6|
|decode named|119.5|135|123|

三条结论：

1. **上一轮的「Rust encode 23.3」不可复现**（现在 29 左右），两次都是 release + 同一 harness，
   差的只能是机器状态。这就是「所有语言在同一时段、同一个 WSL 里跑」的意义：跨会话的绝对值
   不可比，只有同一次会话里的比值有意义。
2. **Rust 与 C++ virtual 打平，输给 template 约 1.6 倍**。virtual 那列是 `Converter<T>` 的虚
   调用，template 列是编译期单态化——Rust 的泛型本应站到 template 这一侧，实际落在 virtual
   这一侧。**当时的头号嫌疑**是每次写入的 `Encoder::Inner` 分支。**订正（§19.15.4）**：这条
   已被实测否决——后端在构造时确定，内联后 discriminant 是常量，两次 `match` 被 LLVM 完全
   折叠；即使用 opaque 值强制后端运行时不可知，16 次 `write` 也只多 3.3 ns（≈0.2 ns/write），
   解释不了几十 ns 的差距。真正的开销在 `write_prefixed` 与逐成员的子 encoder。
3. **字符串口径**：三种语言的 string 内存表示不同（C# UTF-16 / C++ 8 位序列 / Rust
   UTF-8），跨语言行改用字节类型（C# `byte[]`），`string` 不参与比值（§19.15.1，用户裁定
   2026-11-08）。在此之前，涉及字符串的行只用于双边归因，不外推成跨语言结论。

另外发现 C++ 那份 binary 的 google benchmark 库是 DEBUG 构建（运行时自报
`Library was built as DEBUG`），被测库仍是 `-O3`；harness 自身的开销可能让 C++ 数字略偏高，
重编 benchmark 后需再核一次。

### 19.9 容器整块写的 A/B（2026-11-08）

`Layout::POD` 落地后（§6 第 2 条），同一个二进制里放了两个逐字节同构的 1024 元素用例：
`Vec<i32>`（`POD = true`，整块写）与 `Vec<LoopI32>`（`POD = false`，逐元素；`LoopI32` 是
`binary-bench` 里的 4 字节包装类型，除 `POD` 外与 `i32` 完全一样，存在意义就是量这条路径）。
连跑三次：

|用例|整块（`Vec<i32>`）|逐元素（`Vec<LoopI32>`）|
|:---|---:|---:|
|encode 1024 × i32（4100 字节）|38.1 / 38.4 / 39.5 ns|80.9 / 82.9 / 92.4 ns|

2.1–2.4×，且 wire 完全不变（bench 里断言了 `vec_len == loop_len`，
`test_pod_containers_are_byte_identical` 另行钉住字节）。同一份数据 `decode vec<i32> x1024`
当时是 **1.44 µs**（≈1.4 ns/元素），比编码慢 36 倍——容器解码仍是 `with_capacity` + `push`
循环；这一半的 memcpy 已在 §19.10 定型并落地（26–31×）。

（下表写于 `POD` 时代；2026-11-08 之后同一个对比由「是否覆盖 `encode_slice`」承担，
数据不变，见 §19.16。）

这个 A/B 也说明了为什么这里要用常量而不是 trait：`if T::POD` 在单态化后被折叠成一个臂，
`Vec<LoopI32>` 的生成代码里没有 `POD` 残留，`Vec<i32>` 的生成代码里没有循环——两个用例的
差异就是生成代码的差异。

### 19.10 解码整块读：把 `bool` / `char` 剔出 POD（2026-11-08，已定型：方案 A）

§6 第 2 条原本把解码 memcpy 卡在「`bool` / `char` 有非法 bit pattern」。反提案是：把这两个
类型标成非 POD，其余照旧整块拷。**实测收益 26–31×；虽有三个代价，仍定型为方案 A。**

**决策理由（使用者口径）**：大量 `bool` 集合在实际应用中很难出现，真有密集布尔需求应当用
**bitset**。为这一个罕见场景把 `POD` 拆成两个常量、维护两份白名单、并让公开面多一条
「全部 bit pattern 合法」契约，不划算。因此 `bool` 永久留在逐元素路径，**不为编码侧开特例**；
`Vec<bool>` 的编码回退是明确接受的代价，不是待修的性能问题。

**若将来要支持密集布尔集合**：正解是独立的 bitset 编码（例如 `Vec<u64>` 加位计数，或专门的
`BitSet` 类型配自定义 `Encode` / `Decode` 实现），它改变的是 wire 形状，与 `POD` 无关；
先基准再决定是否进公开 API。不要为了 `Vec<bool>` 重新拆开 `POD`。

先看 C++：`ContainerConverter::Decode` 用的是**同一个** `Copyable` 编译期标志
（`sizeof(E) == 1 || (小端 && trivially_copyable)`）加运行时 `dynamic_cast` 得到的 `copy`，
解码分支直接 `reinterpret_cast<const E*>(span.data())` 造 `std::span<const E>` 再交给容器的
迭代器构造。`sizeof(bool) == 1` 也满足 `Copyable`，所以 **C++ 连 `bool` 一起 memcpy** ——
把 `02` 读进 `bool` 在 C++ 里同样是 UB，只是主流编译器表现得像 `true`。Rust 不能照抄。

定型做法（当前代码状态，已落地）：

- `Layout::POD` 的契约从「内存字节 == wire 字节」（单向布局声明）收紧为**双向**，额外断言
  全部 bit pattern 是合法值、类型无 `Drop` / 无借用。
- `bool` 改 `POD = false`（`char` 本来就是 `false`）。两者留在逐元素路径，`b != 0` 与
  `char::from_u32` 的宽容 / 校验语义不变。
- `Vec<T>::decode` 定长分支在 `T::POD && !needs_drop::<T>() && size_of::<T>() == LEN` 时，
  用 `take(remaining)` + `copy_nonoverlapping` + `set_len(count)` 取代 `push` 循环。
- 测试：`test_vec_decode_bulk_arm_matches_per_element_arm`（两条路径同 wire、同结果）、
  `test_bool_and_char_stay_off_the_bulk_decode_path`（常量断言 + 宽容 `02` / `ff` + `char`
  拒绝非法 scalar）、`test_vec_of_fixed_arrays_uses_bulk_decode`（`[i32; N]` 继承 POD）；
  另加 `test_pod_whitelist_is_endian_aware`（端序白名单 + wire 字节序，§19.11）。

WSL 同一会话、各三次：

|用例|改前|改后|比值|
|:---|---:|---:|---:|
|`decode vec<i32> x1024`|1215 / 1379 / 1440 ns|44.8 / 46.9 / 47.8 ns|**26–31×**|
|`encode vec<i32> x1024`|35.5 / 39.2 / 38.5 ns|34.3 / 36.3 / 35.4 ns|不变|
|`encode vec<loop> x1024`（逐元素对照）|81.3 / 85.0 / 77.4 ns|142.8 / 146.1 / 141.8 ns|机器状态漂移|
|`decode vec<u8> x1024`（POD，1 字节对照）|—|13.5 / 14.8 / 13.9 ns|—|
|`decode vec<bool> x1024`（非 POD）|—|1202 / 1220 / 1216 ns|~85× 慢于 u8（decode 数据可靠）|
|`encode vec<bool> x1024`（非 POD）|—|2272 / 2502 / 2374 ns|逐元素，倍率见注|

注：`encode` 的绝对数字受 LLVM 影响（`vec_buf` 内容未被观察，逐元素对照 `vec<loop>` 明显
被折叠到约 0.14 ns/元素），所以表里 encode 侧的倍率**只作代码路径差异的定性提示，不是可
复现的定量结论**，也不作为本次决策依据。可依赖的数字是 decode 的 26–31×，以及
`Vec<bool>` encode 的绝对量级 2.27–2.50 µs（≈2.3 ns/元素）；`decode` 用 `bench_ret` 返回
容器，不受优化消除影响。

**三个代价（已接受）**：

1. **`bool` 编码侧从整块写退回逐元素**。写 `bool` 本来完全安全（内存只可能是 `0` / `1`），
   现在为了让同一个常量在解码侧成立而被牺牲：`Vec<bool>` ×1024 从一次 bulk write 变成
   1024 次 `write_byte`（约 2.3–2.5 µs）。这是决策**明确接受**的代价，依据是此类数据
   在实际应用中罕见、真需要应改 bitset。`char` 本来就是 `false`，无变化。
2. **`POD` 变成 unsafe 语义契约，且下游可写**。`Layout` 是公开 trait，
   `impl Layout for MyType { const POD: bool = true; }` 是合法代码。编码侧误设最多写出脏字节；
   解码侧误设会把**不可信输入**采纳成非法值（`bool` 式 UB），或绕过自定义 `Decode` 的校验。
   `needs_drop` / `size_of` 两道守卫能挡住「带析构」与「长度写错」，挡不住「bit pattern
   受限但无析构」的类型。这是本方案最重的一条，目前靠「默认 `false` + 只由本 crate 显式
   打开」与文档契约兜住；彻底封死需要 sealed 常量（§6 第 2 条已评估并否决：会把用户的
   `impl Layout` 一起封死）。若将来出现外部误设的实际案例，再单独处理。
3. **一个常量耦合两个方向**。将来任何「可整块写但不可整块读」的类型（`bool` 就是第一个）
   都只能牺牲一侧。C++ 用同一标志换来的代价是连 `bool` 一起 UB，不是好先例。

**三个方案与裁定**：

||做法|收益|代价 / 裁定|
|:---|:---|:---|:---|
|**A ✅ 采用**|收紧 `POD` 为双向，`bool` 改 `false`|解码 26–31×|`bool` 编码回退逐元素；公开契约升级；一个常量耦合两侧 —— **接受**，理由见本节开头|
|B ❌ 否决|拆成 `POD`（编码侧，`bool` 仍 `true`）+ 新常量 `ANY_BITS`（解码侧，默认 `false`，只整数 / 浮点 `true`）|同样 26–31×，且保住 `bool` 编码 13 ns|公开面 +1 关联常量、两份白名单需同步维护，只为服务一个罕见场景 —— 不划算|
|C ❌ 否决|回退解码整块读，只保留 `with_capacity` + `push`|无|放弃 26–31×，无对价|

**两项验证均已关闭（2026-11-08，不参与决策本身）**：大端一项用 s390x + qemu-user
实测关闭（§19.11），并顺带订正了这里「大端下 `POD` 恒 `false`」的错误推断：1 字节标量
仍为 POD，只有多字节标量回退。miri 一项用 nightly + Miri 在 host 与 s390x 抽象机上跑完
全部运行期测试（§19.12），`copy_nonoverlapping` + `set_len` 未检出 UB。本决策的依据是
API 面与使用场景，不依赖这两项。**仍未消除**的是 `POD` 作为公开可写契约的 soundness
风险：外部误设 `true` 时 Miri 也救不了，见 §19.12 末段。

> **2026-11-08 后续（§19.16）**：这条风险已经关掉 —— `Layout::POD` 被删除，整块能力改成
> `#[doc(hidden)]` 的元素钩子（默认逐元素），外部类型不再有任何**安全**途径打开整块路径。
> 本节保留的是当时的决策过程与实测数据；「代价 2」描述的是 `POD` 时代，结论仍成立，
> 只是载体换了。

### 19.11 大端实测：s390x + qemu-user（2026-11-08）

§19.10 曾把「大端下 `POD` 恒 `false`」当作理论推断，其实**是错的**：`primitives.rs` 的
`size_of::<$ty>() == 1 || cfg!(target_endian = "little")` 让 `u8` / `i8`（以及由它们组成的
`[u8; N]`）在大端下**仍是 POD**，两侧都走 bulk；只有多字节标量回退。这与 C++
`ContainerConverter::Copyable = sizeof(E) == 1 || (native == little && trivially_copyable)`
（`ContainerConverter.hpp` 的 `ContainerConverter::Copyable` 判定）同形：1 字节没有字节序
问题，`u8` 的全部 bit pattern 又都合法，所以 bulk 读也安全。`bool` 仍是 `false`（§19.10）。

**做法**（参考 C++ `scripts/test-multiarch.sh` 的「检查交叉编译器 → 编译 → 跑测试」）：
一开始是独立的 `scripts/s390x-check.sh`，检查 `s390x-linux-gnu-gcc` / `qemu-s390x` /
`/usr/s390x-linux-gnu` sysroot，按需 `rustup target add s390x-unknown-linux-gnu`，设
`CARGO_TARGET_S390X_UNKNOWN_LINUX_GNU_LINKER=s390x-linux-gnu-gcc` 与
`..._RUNNER="qemu-s390x -L /usr/s390x-linux-gnu"`，再 `cargo test --all --target ...`。
本机 Debian forky/sid 已具备 gcc / qemu / s390x libc；WSL 的 binfmt_misc 也已注册 qemu，
但脚本显式指定 runner，不依赖宿主配置。脚本放 `scripts/` 是为了入库、可在 CI 复现。
这套逻辑后来抽成了通用的 `scripts/test-multiarch.sh`（§20.1），`s390x` 只是矩阵里的一个
目标，独立脚本已删除。

**证据**：

- 对交叉编译出的测试二进制跑 `file` →
  `ELF 64-bit MSB pie executable, IBM S/390` —— 确为大端，不是模拟器里跑 host 二进制。
- 首次**未改代码**的基线编译就抓到两个硬编码小端假设（E0080）：
  `test_vec_decode_bulk_arm_matches_per_element_arm` 与
  `test_vec_of_fixed_arrays_uses_bulk_decode` 里的 `const { assert!(<i32 as Layout>::POD) }`
  在 s390x 上求值失败。已改为
  `assert!(<i32 as Layout>::POD == cfg!(target_endian = "little"))`，测试名保留、两端都成立
  （大端下两条路径都是逐元素，仍断言同 wire、同结果）。
- 新增 `test_pod_whitelist_is_endian_aware`：钉住 `u8` / `i8` / `[u8; 3]` 恒 POD、
  `bool` / `char` 恒非 POD、`i32` / `u64` / `f32` / `usize` 只在大端为 `false`，以及 wire
  字节序不随宿主变（`0x01020304i32` 恒为 `04 03 02 01`）、`Vec<u8>` 的 bulk decode
  在大端仍可用。

**结果**：`s390x` 与 host 跑同一套用例全绿，连 trybuild 的编译失败快照与 doctest 都在
qemu 下通过（计数见 §20.1）。至此 §19.10 的「大端未实测」已关闭；另一项（miri）见 §19.12。
注意两者验的东西不同：qemu 跑真实 `ELF MSB` 二进制 + 真实 libc，验的是**行为与端序**；
Miri 解释 MIR，验的是 `unsafe` 有没有 UB。

### 19.12 Miri UB 检查（2026-11-08）

§19.10 定型时唯一挂着的验证项是「`Vec<T>::decode` 的 `copy_nonoverlapping` + `set_len`
未经 UB 检查器验证」。Miri 是 Rust 官方的 UB 解释器：不生成机器码，逐条解释 MIR，检查
越界 / 未初始化内存 / 对齐 / 非法 `bool` 位模式 / 别名规则等。它必须跑 nightly，本机原先
只有 stable 1.99.1，故补装 `nightly-x86_64-unknown-linux-gnu`（1.101.0-nightly,
2026-11-06）+ `miri` / `rust-src` 组件。

**一行代码改动**：`tests/ui.rs` 的 trybuild 测试加 `#[cfg(not(miri))]`。实测在 Miri 下
`trybuild::TestCases` 会因 glob 走 `std::fs::metadata` → `statx`，被 Miri 的 isolation
直接中止（`unsupported operation`，栈在 `trybuild::expand::glob`）。compile-fail 快照验的是
**编译期诊断文本**，本来就没有运行期 UB 可查；真实二进制仍由 `cargo test` 与 §19.11 的
qemu 跑这 23 个 fixture。

**结果**：三种模式全部 `EXIT=0`、未检出 UB —— host 抽象机、s390x 大端抽象机（其中
`i32` 非 POD 走逐元素、`u8` / `[u8;N]` 仍 bulk），以及加 `-Zmiri-strict-provenance` 只跑
`tests/wire`。计数见 §20.1。

Miri 官方 README 把 `s390x-unknown-linux-gnu` 列为 big-endian target of choice，无需交叉
gcc / qemu，sysroot 由 `rust-src` 现编；它补的是 qemu 验不到的那一半：`Vec::with_capacity`
的已对齐分配被 `copy_nonoverlapping` 写入 `remaining == count * size_of::<T>()` 字节、
`set_len(count)` 在拷贝之后接管、`bool` / `char` 的 `02` / `ff` / 非法 scalar 仍由宽容解码
处理而未生成非法值，这些路径在两端抽象机上均未检出 UB。

**结论与边界**：§19.10 的最后一项验证关闭，定型结论不变。但 Miri 只检查「这一次执行、
这一组输入」，**不报错 ≠ sound**；它不测性能、不支持 FFI，也不能替代 §19.11 的真实大端
二进制。`POD` 依旧是公开可写、编译期不强制的 unsafe 契约 —— 下游若写
`impl Layout for MyType { const POD: bool = true; }` 而类型有受限 bit pattern 或自定义
`Decode`，Miri 也救不了，仍靠「默认 false + 只由本 crate 打开 + 文档契约 +
`!needs_drop` / `size_of == LEN` 两道守卫」兜住（sealed 常量方案此前已评估否决）。

复跑：`./scripts/check-miri.sh`（host + 大端），或 `host` / `be` 单独跑；脚本会自动检查并
安装 nightly 组件，`MIRIFLAGS` 可透传额外开关。与其它脚本一样放 `scripts/` 以便入库复现。

---

### 19.13 `Box` / `Rc` / `Arc`：wire 透明指针（2026-11-08）

**结论**：三者在 wire 上与 `T` 完全一致 —— 不写 tag、不写长度、不写引用身份。
C# 用可空引用（`CircularTypeReferenceTests/LinkedListTests.cs` 的 `LinkedList<T>? next`）、
C++ 用指针表达同一形状，都没有额外字节。新增 `smart_ptr.rs`（`macro_rules!` 生成 3 组
impl）+ `lib.rs` 一行 `mod smart_ptr;`，**不改 wire、不改 derive 宏、不新增公开 API**；
`alloc::boxed::Box` / `alloc::rc::Rc` / `alloc::sync::Arc` 都在 `alloc`，`no_std` 直接可用。

**四个必须踩准的点**：

1. `encode` / `encode_with_prefix` / `encode_auto` 三个方法都**显式转发**，不能依赖默认实现：
   默认 `encode_auto` / `encode_with_prefix` 按 `Self::LEN` 分派，会丢掉 `T` 上的 override
   （tuple 与 `#[binary(tuple)]` 的 flatten、`Option<T>`、`String` 的 O(1) 前缀）。
2. `Layout::LEN = T::LEN`：否则 `Vec<Box<i32>>` 会把元素当变长、逐项 `write_prefixed`，
   与 `Vec<i32>` 字节不一致。
3. `Layout::POD` 恒 `false`：`size_of::<Ptr<T>>()` 是指针，而 `encode_slice` 编码侧没有
   `needs_drop` 守卫；误设 `true` 会把指针字节写进 wire，解码 bulk 分支更会把 wire 字节
   当成野指针接管。**字节一致 ≠ 走同一条快路径**。
4. 递归 trait 义务能闭合，derive 宏一行未改：`Node: Encode → Option<Box<Node>> →
   Box<Node> → Node`；`gen_layout_tuple` 求 `LEN` 时 `Option<T>::LEN == 0` 短路，
   const 求值不循环。§16.4 的递归 union 阻塞随之解除。

**`?Sized` 机制**：Rust 泛型 `T` 默认隐式 `T: Sized`；`?Sized` 是**移除**这个隐式 bound，
不是要求 DST。`str` / `[u8]` / `dyn Trait` 的大小只在运行时知道，只能经指针持有
（`Box<str>` 自己是 16 字节胖指针）。`Encode: Layout` 的 `Self` 允许 `?Sized`，
`encode(&self)` 只借用，故 `Box<str>` / `Rc<[u8]>` 可编码；`Decode<'de>: Layout + Sized`
要按值返回 `Result<Self>`，`T: Decode<'de>` 因而隐含 `T: Sized`，`Ptr::new` 也需要
Sized 的 pointee，故 `Box<str>` 无 `Decode`。**这与 `T` 是对称的**：裸 `str` 本身也只有
`Encode`（`&'de str` 才有 `Decode`）。不人为收窄编码侧，也不新增 API。

**两个语义边界**（写进 `smart_ptr.rs` 模块文档）：

- 解码总是新建分配，**共享所有权不保留**：两个 clone 的 `Rc` 编码成两个值、解码成两个
  独立 `Rc`。wire 没有对象身份，C# / C++ 同样如此；测试用 `Rc::ptr_eq` /
  `Arc::ptr_eq` 钉住。
- **不做任何深度检查**（2026-11-09 决定）。C# 有三处 `EnsureSufficientExecutionStack`：
  `ContextMethodsOfNamedObject` 的 encode / decode delegate（开关是 `hasSelfTypeReference`，
  即某个成员的 converter 尚未就绪）与 `GeneratorObjectConverter::Ensure`（`object` 的静态类型
  没有信息，无条件插，**且只在 encode**——它的 `Decode` 返回惰性 `Token`，一次递归都不做）。
  这三处的性质是**兜底**：把不可捕获的 `StackOverflowException` 变成可捕获的
  `InsufficientExecutionStackException`，不是格式约束。C++ 侧完全没有。
- **Rust 侧不加计数器、不加 `depth` 字段、不加 `Error` 变体。** 技术上 `Decoder` 加一个字段、
  `with_prefix` 进出各改一次就做得到（递归类型每层必过 `with_prefix`，因为
  `Option<Box<Node>>` 的 `LEN == 0` 走变长分支），但**深度不是格式的一部分**：wire 由字节
  定义，不由实现的常数定义。加了之后同一个字节串在 A 实现里深度 200 合法、在 B 实现里非法，
  是格式被实现污染；C# / C++ 都没有上限，Rust 单方面加上就破了「三边同一份数据同结果」。
  超深输入的正确归因是**资源不足**而非格式错误——10 万层的链表是合法数据，只是当前栈解不了；
  报成 `Error::TooDeep` 会让调用方按「数据坏了」处理并丢弃，而它换个大栈重试就能成功。
  同理也不加栈探测：`stacker` 依赖 `psm` 汇编与平台栈边界 API，`no_std` 层用不了，
  只在 `std` 下加会让同一份输入在两种构建里表现不同，比不加更糟。兜底是宿主的事
  （`Builder::stack_size`、按平台查剩余栈后自行决定），不是库的 API 面。

**验证**（`tests/smart_ptr.rs` 12 例）：raw / auto / named 前缀三处字节与 `T` 相同、
`Vec<Ptr<i32>>` 与 `Vec<i32>` 相同、tuple flatten 与 `String` O(1) 前缀 override 不丢、
`LEN` 继承 / `POD` 恒 false（const 断言）、`?Sized` 指针按借用值编码、嵌套与递归
struct / union / tuple round-trip、共享不保留。全门通过（host / Miri 两端 / `s390x` 真大端 /
`--no-default-features` 构建，`Rc` / `Arc` 都在 `alloc` 里），计数见 §20.1。

clippy 两个新 lint 顺带处理：`assertions_on_constants` 改用 `const { assert!(!…::POD) }`，
`arc_with_non_send_sync` 把嵌套用例的 `Arc<Rc<Box<i32>>>` 换成 `Box<Rc<Arc<i32>>>`
（`Arc` 包非 `Send + Sync` 的 `Rc` 本身就是 API 坏味道，不是测试要表达的东西）。


### 19.14 标准容器落地：零新增依赖（2026-11-08）

**问题**：§13.1 的 P1 剩下八个容器，会不会像 serde 生态那样要引 `hashbrown` / `indexmap`？

**结论**：不会。`Cargo.toml` / `Cargo.lock` 一个字没改，`cargo tree -e normal -p binary` 仍是
`binary → binary-derive`（`--no-default-features` 下为空）。C# / C++ 库本体同样只用 BCL /
标准库，外部包只出现在各自的测试与 benchmark 项目里。

#### 依赖分层（实测）

|容器|crate|编译条件|
|:---|:---|:---|
|`VecDeque` / `LinkedList` / `BinaryHeap` / `BTreeMap` / `BTreeSet` / `Cow`|`alloc`|无条件，`--no-default-features` 覆盖|
|`HashMap` / `HashSet`|`std`|`#[cfg(feature = "std")]`；`std = []` 本就是默认 feature|
|`KeyValuePair`|—|不新增类型，直接用 `(K, V)` tuple|

`no_std` 探针：`#![no_std] + extern crate alloc` 下前六个类型编译通过；直接引用
`std::collections::HashMap` 报 E0433（crate 未链接），所以把它 gate 掉不是砍功能 —— 那个
target 本来就没有这个类型。唯一会引入外部依赖的路径是「坚持 `no_std` 下也要 hash map」
（`hashbrown`），而且它的类型与 `std::collections::HashMap` 不同，得维护两套 impl，已否决。

`HashMap` 泛型化到 `S: BuildHasher`（encode 只迭代）/ `S: BuildHasher + Default`（decode 用
`collect()`），`RandomState` 天然满足；探针用 `BuildHasherDefault<DefaultHasher>` 验证自定义
hasher 也能编解。这是不引 `hashbrown` 的关键。

#### wire 形状订正（三边证据）

§17.3 原写「先写元素个数，再逐个 `auto`」，**是错的**。三边都不写元素个数：

- C++ `ContainerConverter::Encode`：`for (const auto& i : item) converter->EncodeAuto(...)`，
  `Decode` 是 `while (!copy.empty()) Insert(result, converter->DecodeAuto(copy))`。
- C# `EnumerableEncoder` / `DictionaryEncoder` / `KeyValueEnumerableEncoder`：
  `init.EncodeAuto(key)` + `tail.EncodeAuto(value)`；`DictionaryDecoder` 是
  `while (intent.Length is not 0)`。
- 边界全部来自外层长度前缀；定长元素由 `Layout::LEN` 从 payload 长度反推个数，变长元素靠
  自身前缀自分隔。

所以容器 raw payload 与 `Vec<T>` 完全相同，map 的一个条目就是 `(K, V)` tuple 的 flatten
形式（两个成员都 `auto`，**没有 pair 头**）—— Rust 的 `(K, V)::encode_auto` 正好是这个形状。
字节级证据是间接但闭合的：`Vec<T>` 与 tuple 的 wire 已由 33 条真实 C# fixture（§12.5）和
10 元三边字节（§11.5.2）钉住，容器只是「不写个数地重复它们」，因此无需新增 C# fixture
也能保证三边互解；唯一无法钉死的是哈希容器的**顺序**。
`collections.rs` 里 `encode_items` / `encode_pairs` 两个 helper 与 `Vec<T>` 共用
`write_prefixed_bounded(len * LEN)` / `write_prefixed` 的 O(1) vs anchor 分裂；`VecDeque` 用
`as_slices()` 两段各调 `encode_slice`，保住 POD 整块写。所有 `Decode` 都委托
`Vec<T>::decode` 再 `collect()`，因此自动继承定长计数、`MisalignedElements` 检查与
`Layout::POD` bulk 读，没有复制第二份 `unsafe`。

#### 契约与语义边界

- **顺序不作跨语言字节保证**：C# `Dictionary` 哈希序、C++ `std::map` 键序、Rust `HashMap`
  随机种子；只承诺「是 KV 序列、能互解」。`BTreeMap` / `BTreeSet` 键序，可与有序容器对齐。
  `BinaryHeap::iter()` 是堆内部数组序，且 C# / C++ 无对应类型，属 Rust 单边扩展。
- **重复键**：Rust `collect()` 覆盖，C# `DictionaryDecoder` 用 `Add` 抛异常 —— 已知差异，
  未加检测（不为单个边界新增 `Error` 变体），已写进模块文档。
- **`Cow`**：encode 三方法显式转发到 `B`（保住 `str` 的 O(1) 前缀 override），decode 恒为
  `Owned`；零拷贝仍走 `&'de str` / `&'de [u8]`。
- 递归深度不受限制（§19.13 决定不做深度检查）、`Layout::POD` 公开可写的 soundness 风险，
  均为既有决策。

#### 验证

`tests/collections.rs` 11 例：字节与 `Vec<T>` / `Vec<(K, V)>` 逐字节相等（含 named 前缀、
raw、空容器、wrap 后的双段 `VecDeque`、变长值、嵌套容器）、自定义 hasher、Cow 透明、
`MisalignedElements`、全形状 round-trip。质量门见 §20.1（Miri 两端与 s390x 同样全绿，
`--no-default-features` 覆盖 `alloc` 层）。

### 19.15 C++ / Rust 同口径剖析（2026-11-08，两边都用原生 `u8` 字符串）

#### 19.15.1 口径分层：三语言对比只在字节层

**结论（2026-11-08 用户裁定，取代本节原来「给 C++/Rust 补 UTF-16 适配器」的写法）**：
`string` 编解码**不参与跨语言比值**。真要三边同表，C# 用 `byte[]`、C++ 用 `std::string`
但只当字节、Rust 用 `&[u8]` / `Vec<u8>`；三边都不转码、不校验，量的是编码器本身（头宽、
预留、memcpy），不是 string handling。

**为什么「同一行 encode string」根本不是同一件事**：

||内存表示|写到 wire|从 wire 读|
|:---|:---|:---|:---|
|C#|`string` = UTF-16，逐 `char`（码元）索引|**转码**成 UTF-8 字节（`Encoding.UTF8.GetBytes`）|转回 UTF-16，非法序列替 `U+FFFD`|
|C++|`std::string` = 8 位字节序列，库不保证是 UTF-8|原样 memcpy|原样 memcpy|
|Rust|`String` = UTF-8，类型不变量保证合法|原样 memcpy|`str::from_utf8` **校验**，非法即错误|

同一行在 C# 里量的是「转码 + 拷贝」，在 C++/Rust 里量的是「拷贝」；解码侧三种行为（转码 /
原样 / 校验）也不一样。**行的名字一样，被测物不一样。**

**为什么不用「三边都套 UTF-16 适配器」抹平**：那只是把偏差换到另一边，代价有两层。

1. **转码层不属于任何一边的库热路径**。加进去以后量的是「适配器写得好不好」，而库自己的
   `String` / `string` 路径一点没变。C++ 那份适配器
   （`binary-benchmarks/cxx/src/Utf16StringAsUtf8BytesConverter.hpp`）自带 SSE2 的 ASCII
   快路径和手写 UTF-8 编码器，Rust 侧要复刻的是同一套东西——那是一份新实现，不是同一份
   被测代码。
2. **适配器拿不到「一次预留」的入口，会被入口税吃掉**。C++ 那份直接调
   `binary::internal::AllocatorUnsafeAccessor::Assign(allocator, len)` 拿 `&mut [u8]` 原地
   写；Rust 的 `Encoder::write_prefixed_bytes` 是 `pub(crate)`，库外只能走
   `write_prefixed_bounded` —— 6 字节字符串 **6.2 ns 对 2.6 ns**（§19.15.11）。一个成员贵
   3.6 ns，**比整条 `String` 成员（2.6 ns）还多**，而转码在 ASCII 负载上退化成窄拷贝，
   救不回这笔钱。于是「公平」变成给 Rust 凭空加一笔假成本。

**落地口径**：

- 三语言矩阵里字符串相关的行一律换字节类型：C# `byte[]`、C++ `std::string`（当 bytes）、
  Rust `&[u8]` / `Vec<u8>`。顺带修掉一处遗留偏差：C# 的 `string` 用 `GetMaxByteCount` 上界
  定头宽（42..=127 字符写 4 字节头，正是 `csharp-only` 桶的来源），`byte[]` 用精确长度，
  三边头宽策略从此一致（§12.3）。
- 现有负载全是 ASCII（`"csharp"` / `"one"` / `"two"` / `"three"`），换成字节类型后 **wire
  逐字节不变**，所以历史行与字节数仍可比，只是被测类型换成 bytes。
- C# 的 `string` 行留在它自己的 benchmark（`Benchmarks.AllocatorStringTests`：`Append` /
  `AppendWithLengthPrefix` / writer 两种写法），不进跨语言矩阵。
- Rust ↔ C++ 的**双边**行仍用 `String` / `std::string`：两边都是原生 8 位、都不转码，同口径；
  §19.15.5~§19.15.13 的数字就是这么量的，只是不外推到三语言结论。

#### 19.15.2 装置

Rust：`cargo run --release -p binary-bench -- wire | profile`，fat LTO + `codegen-units=1`，
对齐 C++ 的 Release `-O3` + IPO。C++ 侧改用一份**同测量规则**的 harness
（warmup 2000、7 轮、每轮 50 ms 预算、取最小值、`black_box`），直接调 template converter，
不经过 google benchmark —— 系统 `libbenchmark` 是 DEBUG 构建且 `Time` 列是均值，与 Rust 的
`min over rounds` 不是同一个估计量。该 harness 里用 `static_assert` 钉住
`Converter<TypeX1>` / `Converter<Type01>` 的具体 converter 类型，并断言 6 个成员全部往返成功；
两边 payload 的 hex 由程序打印后逐字节比对。

一个方法学警告：正式 google benchmark 的 `Decode Template Named Object` 是 142–146 ns，
而同规则 harness 只有 68–69 ns（encode 两行倒是吻合，36–40 ns）。正式套件里
`NamedObject.cpp` 与 `TupleObject.cpp` 为同一个 `Type01` 各声明了一套 ADL schema，IPO 下
converter 的解析容易串味；**decode 行以 `static_assert` 钉住的同规则 harness 为准**。

**只测 raw 形式**（2026-11-08 用户裁定）：C# 量 `Encode(ref allocator, value)` /
`Decode(bytes)`，C++ 量 `Converter<T>::Encode(allocator, value)` / `Decode(span)`，两边都没有
「auto」这个被测物。Rust 侧再报一条 `encode_auto` 只是同一份代码多走一次 `LEN == 0` 分支
（tuple 的 auto 与 raw 逐字节相同）或多写一个 4 字节前缀（named）；同一次会话里四条 auto/raw
对的差是 **-2.0 / +1.9 / +4.6 / +2.8 ns**，符号都不稳定，就是噪声。auto 与 raw 的字节关系
由 `tests/wire.rs`（`auto_bytes` 对 `encode`）与 `tests/named.rs`（`encode_auto`）钉住，
benchmark 不必再报一遍。本节及以前各小节表格里的 auto 行都早于这次裁定，保留作记录。

**`wire` 只放这四行**（2026-11-08 用户裁定）：C# `IntegrationBenchmarks` 与 C++
`binary-benchmarks/integration-tests` 都只有 Encode / Decode × named / tuple 这四条，混进
容器、owned 或标量行就没法与另外两个仓库逐行对 diff。原来挤在 `wire` 里的容器行搬到
`collections`（1024 元素 bulk vs 逐元素；C# 侧对应独立套件 `Benchmarks.CollectionTests`），
owned `Vec` 两行与 `i32` 基线在 `objects` 里本来就有，删掉重复。脚本的默认组仍是 `wire`，
所以 `./scripts/run-benchmark.sh` 的输出就只剩这四行。

#### 19.15.3 同会话主表（同一台机器、同一时段、两轮取范围，ns/op）

> **已被 §19.15.8 取代（2026-11-08 晚）**：本表的 C++ 列出自「通用 `bench` + lambda」的
> harness，其内联程度与 §19.15.8 固定的形状不同，C++ 绝对值不可与那一节互比。保留本表
> 只作为当时的方法记录与倍率历史；当前数字看 §19.15.8。

|操作（都是 raw / 逐字节相同）|Rust fat LTO|C++ template|Rust / C++|
|:---|---:|---:|---:|
|encode tuple（50 B）|25.6 – 25.7|17.9 – 18.1|**1.42×**|
|decode tuple|92.1 – 94.0|76.4 – 78.8|1.20 – 1.23×|
|encode named（85 B）|76.9 – 77.8|36.6|**2.10×**|
|decode named|105.0 – 106.4|68.0 – 69.2|1.53×|

**跨会话绝对值不可比**：同一个 fat-LTO 源码，在只有 `wire` 组的二进制里 named encode 是
57–61 ns，加入 `profile` 组并把类型改成 `pub(crate)` 后是 77 ns（+30%）。LTO 下无关代码会
改变内联决策与代码布局，所以**只有同一次会话、同一份二进制的比值可信**；§19.8 第一条
的结论在这里再次得到印证。

#### 19.15.4 归因：微基准（写入类每组重复 16 次，ns/op）

所有输入经 `black_box`，闭包结尾把写出的字节也 `black_box` 掉；否则 LLVM 会把整段写缓冲
折叠成常量或提升出循环（第一版跑出清一色 0.42 / 1.05 ns，就是常量折叠 + 循环不变量提升）。

|组|行|结果|读法|
|:---|:---|---:|:---|
|每次写入 ×16|flat cursor（1 次边界检查 + `copy_from_slice`）|1.07|下界|
||`Encoder::write 4B`（borrowed，后端编译期已知）|1.07|**与下界相同**|
||`Encoder::write 4B`（后端由 opaque 值决定）|4.32|枚举只值 3.25 ns / 16 = **0.20 ns/write**|
||`Encoder::write 4B`（owned）|47.9|1 次 1024 B 分配占绝大部分|
|前缀 ×16|flat 两次写|20.0|下界|
||`Encoder` 两次 `write`|34.0|2.12 ns/次|
||`write_prefixed_bounded(6, …)`|26.6|1.66 ns/次，**比两次顶层 `write` 还便宜**|
||**`write_prefixed`**|**91.8**|**5.74 ns/次，是 precise 路径的 3.4×**|
||`String::encode_with_prefix`|74.2|4.64 ns/次（堆上 `String`，长度是运行时 load）|
|named 成员 ×16|flat 8 B 一次写|2.12|下界|
||`write_named_member(Id, i32)`|21.8|1.36 ns/成员|
||`write_named_member(Name, String)`|183.3|11.46 ns/成员|

**编码侧结论**：`Encoder::Inner` 不是瓶颈（§19.8 的头号嫌疑被否决）；真正贵的是
`write_prefixed` —— 先占位 4 字节（不清零）、写 payload、再回填或定长 `u64` 搬移压缩。这份
payload 里它被调用 3 次（named 的 auto 外层、`Item`、`Tags`），C++ `Allocator::FinishAnchor`
也做同样的 wire 动作，但 Rust 用闭包 + 子切片实现更重。其次是每个 named 成员：
`write_name`（当时的私有半操作，现已删除）把前缀编进栈上 `[u8; 4]` 再**两次** `write`，
值又走 `encode_with_prefix` →
`write_prefixed_bounded` → 新建一个子 `Encoder`。`encode tuple (owned Vec)` 57.4 ns vs `on_slice`
25.6 ns（2.2×），差额是 1 次 256 B 分配 + 每次 `Vec::resize` 检查 + `finish()`。

> **倍率口径订正（2026-11-08，见 §19.15.5）**：上表「3.4×」**只在 fat LTO 下成立**。
> 同会话 default release 跑同一份源码是 `write_prefixed_bounded` 50.2 ns / `write_prefixed` 105.0 ns
> = **2.1×**。倍率变大不是兜底路径变慢，而是 LTO 把主路径从 50.2 内联到 26.3 ns、兜底路径
> 只从 105.0 降到 88.0 ns。引用这条结论必须带上 profile；它衡量的是**可内联性差异**，
> 不是两条算法路径的固有代价比。同一现象在 `write_named_member(Id, i32)` 上更极端：
> default 67.9 ns → fat LTO 21.7 ns（3.1×），而手写 4 次 `write` 几乎不变（60.5 / 57.9 ns）
> —— 收益集中在闭包 + 子 `Encoder` 调用链的内联，而不是字节写入。

**解码侧结论**（1 KiB ASCII）：

|行|结果|分配|
|:---|---:|---:|
|`&[u8]` 零拷贝|0.84|0|
|`&str` 零拷贝 + UTF-8 校验|25.4|0|
|`String`|47.1|1 / 1024 B|
|`Vec<u8>` bulk copy|11.9|1|
|`NamedMembers::parse`（只解析前缀、跳过全部成员）|7.5|0|
|`decode named (raw, full)`|101.8|**6 / 129 B**|
|C++ template `decode named`|68.0 – 69.2|4|

名字查找只占 7.5 ns / 0 分配，**瓶颈全在值的解码与分配**。Rust `String` 恒分配且恒校验
UTF-8；C++ `std::string` 的短串走 SSO（实测 6 字符 `std::string` = **0 次分配**）且不校验
——这是语义差异，不能靠「优化」抹平，只能靠零拷贝借用。实测：

|解码|owned|borrow（`Vec<&str>`）|加速|
|:---|---:|---:|---:|
|`Vec<String>` ×8|130.7 ns / 10 分配|65.3 ns / 2 分配|**2.0×**|
|`Vec<String>` ×64|1596.5 ns / **69 分配**|350.6 ns|**4.6×**|

69 次分配 = 5 次 `Vec` 扩容 + 64 次 `String`。这给 P1 `borrow`（§19.17）与 §6 第 3 条
「解码预分配」提供了直接数据依据，但仍是**先基准后修改**，本轮不改代码。

#### 19.15.5 构建配置本身是混淆变量

同一会话里 `default release` 与 `fat LTO + CGU=1` 两个二进制交替跑：

|行|default|fat LTO|影响|
|:---|---:|---:|:---|
|encode tuple raw|23.6|23.5|持平|
|decode tuple raw|97 – 99|90 – 91|LTO 快 ~8%|
|**encode named raw**|**62.9 – 63.1**|74.8 – 76.5|**LTO 慢 ~18%**|
|decode named raw|107 – 108|99 – 101|LTO 快 ~7%|
|`encode vec<bool>` ×1024|2117 – 2124|1010 – 1024|**LTO 快 2.1×**|

所以 §19.1 里「加 fat LTO 对齐 C++ IPO」不是免费的：它让逐元素 `bool` 循环快一倍，却让
named encode 慢近两成。跨会话比较必须同时固定二进制组成与 profile；只报「release」不够。

#### 19.15.6 已接受的代价，顺带量化

`bool` 剔出 POD（§19.10 方案 A）在 1024 元素上的代价：encode 1038 ns / decode 927 ns，
对照 `Vec<u8>` 的 9.2 / 12.5 ns（约 113× / 74×）。结论不变——密集布尔应走 bitset，
不为它拆第二个常量；这里只是把「不算修的性能问题」变成有数字的决策记录。

#### 19.15.7 ≤16 B 压缩：100 串混合长度实测与最终抉择（2026-11-08）

**口径**：100 个长度 0~32 的 UTF-8 字符串、固定种子打乱（≤16 B 46 个、>16 B 54 个、
空串 8 个、33 种长度全覆盖）。每 op 把它们依次用各语言**当前的 Allocator / Encoder
兜底入口**（C# / C++ `Allocator::AppendWithLengthPrefix`、Rust `Encoder::write_prefixed`）
写入提前分配的 4096 B buffer，全程无 realloc。三份实现都从库源码复制后剪裁，**唯一变量
是 ≤16 B 压缩的搬移形状**：A 定长 `u64`（当前实现）、B 精确长度 `memmove`；阈值、
`(-length) & 7` 尾部余量条件、wire 完全相同。C# 用 `byte[]`、不做 UTF-16 转码。两变体在
同一二进制内单态化，7 轮交替先后次序、A/B 背靠背计时取中位数（2000 次预热、每轮校准
迭代数）。单位 ns/op，一 op = 编码 100 个值；比值 < 1 表示 `u64` 更快。

|语言|default `u64`|default `memmove`|比值|fat LTO `u64`|fat LTO `memmove`|比值|
|:---|---:|---:|---:|---:|---:|---:|
|C#|480.0|674.5|**0.712**|445.8|727.1|**0.613**|
|C++ (`-O3` / `-O3 -flto`)|390.0|404.1|**0.965**|404.1|407.6|**0.992**|
|Rust|322.7|433.9|**0.744**|325.3|425.4|**0.765**|

三语言、两构建、两变体的 wire **完全一致**：1947 B，FNV-1a 64 = `59b2006d10e6e3c9`。

**最终抉择（用户裁定，已落地，库代码不再改动）**：照抄 C#/C++ `Allocator::FinishAnchor`
的定长 `u64` 搬移（两次非对齐 8 字节整字搬移 + `(-length) & 7` 余量判断），不采用精确
长度 `memmove`，也不采用无 slack 的阶梯搬移。三条理由：

1. **wire 逐字节一致**：与 C#/C++ 同阈值、同余量条件，fixture 可直接断言字节。
2. **延迟分布集中**：不随长度与 libc 实现抖动，没有 15 B 那样的长度突变，上界可预测。
3. **真实混合长度下均值也更低**：比值 0.61~0.99，三语言 × 两构建方向一致；C++ 差距
   最小（0.965 / 0.992），C# 与 Rust 明显。按 46 个压缩值摊，C# 4.2~6.1、C++ 0.1~0.3、
   Rust 2.2~2.4 ns/值 `u64` 占优（54 个 >16 B 值是两变体相同的公共路径，会稀释差异）。

代价与口径：尾部 0..7 字节安全余量，余量不足保留 4 字节头（两种宽度都是合法 wire，
提前分配足够长的 buffer 即可避免）；相对「不压缩」每个小嵌套值约 +4~5 ns，属已知并
接受的代价。只有同会话同二进制的比值可信（§10 / §19.15.4）；跨语言绝对 ns/op 受各自
调用形式影响，不要横向比较，C# 未开 AOT 仅作同语言内 A/B；端到端 A/B 不能判断 ns 级
差异。

**2026-11-09 补测（纯分支重构，wire 不变）**：兜底分支合并成与 C#
`Allocator.FinishAnchor` / C++ `Allocator::FinishAnchor` 同形的单一 `if`
之后，`profile` 的相关行交替 A/B（两份二进制、各两轮）：`write_prefixed`（6 B）
154.8 / 153.1 → 140.1 / 136.8 ns/op，`write_prefixed`（20 B，宽头）104.6 / 97.7 →
95.4 / 69.1；但同一二进制里**未改动**的 `write_prefixed_bounded` 对照行同样漂移
±8~20%（82.7 / 85.9 → 77.7 / 73.2）。判为**无回归、落在噪声内**，不宣称变快——
合并的目的是消掉重复分支，不是性能改动（端到端 A/B 判不了这个量级，见下）。

**此前的踩坑（简要）**：

- 「`int64` 大多数情况更优」「`memmove` 只在 2 的幂长度占优」是错的。编译期常量长度的
  隔离扫描里 `memmove` 在 1~6 / 9~10 / 12 B 反而快 4~5 ns，8 / 16 B 两者持平；正确说法
  是 `u64` 方差更小、上界可预测，而非均值更低。
- 曾把 ~5 ns 回归判成 harness 假象（跨迭代别名）。C++ / Rust 双边同形微基准把它定位为
  刚 store 完 payload 就做 8 字节整字 load 的 store-to-load forwarding，`mfence` / fence
  均不能消除；是真实代价，不是测量误差。
- 隔离微基准把长度固化成编译期常量，编译器按长度展开成定长指令；库里 `actual` 是运行
  时值，精确长度搬移会退化成通用 `memmove` 调用（调用与长度分派成本超过 forwarding
  代价）。所以短长度的隔离结论**不能外推**到真实编码路径，实测方向反转 —— 这正是上表
  要单独做的原因。
- 端到端 A/B 被 LTO 代码布局主导（同一改动在 MSVC 快 8~10%、GCC 慢 6~10%，不相关 case
  漂移 ±15~38%），不能用来判断 5 ns 级差异；倍率也随 profile 变（fat LTO 3.3× /
  default 2.1×，§19.15.4）。
- 更早的两处：曾以为 anchor 对全部 `<=127` 都压缩，实测 C#/C++ 只在 `<=16` 压缩
  （§12.4）；下调阈值后还暴露 `encode_prefix` 在宽头分支只写 1 字节的隐藏 bug。数据集
  第一版用纯 hex 行，空串记录被按行读取吃掉，改成显式 `<长度>:<hex>` 才补齐 100 条。

---

#### 19.15.8 复跑：tuple / tuple object / named（2026-11-08，改进后代码 vs C++）

Rust 侧新增 `objects` 组（`cargo run --release -p binary-bench -- objects`），C++ 侧对应
工程落在 `binary-benchmarks/tuple-named-bench`。三种形状、同一 payload、同一 1024 B
借用缓冲、raw 形式、两边都是原生 `u8` 字符串：

|形状|Rust|C++|wire|
|:---|:---|:---|---:|
|tuple|`TypeX1`（真元组）|`TupleConverter<TypeX1>`|50 B|
|tuple object|`#[binary(tuple)] Type01Tuple`|`TupleObjectConverter<Type01T, …>`|50 B（与元组逐字节相同）|
|named object|默认 derive `Type01`|`NamedObjectConverter<Type01, …>`（声明序）|85 B|

测量规则两边同一份：warmup 2000、7 轮、每轮翻倍到 50 ms、取最小、结果过 `black_box`。
Rust 用 `--release` 与 fat LTO + CGU=1，C++ 用 `-O3 -DNDEBUG -flto`（与项目 Release 的 IPO 一致）；四个二进制在同一会话里交替跑三轮，下表取最小值。

|操作（raw）|Rust fat LTO|C++ template|Rust / C++|
|:---|---:|---:|---:|
|encode tuple（50 B）|24.1|14.4|1.68×|
|encode tuple object|23.5|14.4|1.63×|
|encode named（85 B）|69.5|16.0|**4.35×**|
|decode tuple|85.7|55.4|1.55×|
|decode tuple object|88.7|56.6|1.57×|
|decode named|96.5|69.8|1.38×|
|named 名字解析（只读前缀、不解值）|10.8|11.7|0.93×|
|encode tuple（owned，每次新分配）|54.9|40.7|1.35×|
|encode named（owned，每次新分配）|120.7|53.7|2.25×|
|encode / decode `i32`|0.63 / 0.63|0.42 / 0.42|1.5×|

**下表是改动前的数字**：`encode named` 那两行的 4.35× / 2.25× 在 §19.15.9 归因并修掉，
之后是 2.2~2.7×。其余行不受影响（同一轮重跑确认 tuple 与 decode 在噪声内）。

同一轮的 C++ 虚拟分发行（`IGenerator` + `Converter<T>*`）：tuple encode 22.3 / decode 63.5、
tuple object 21.3 / 67.7、named 26.5 / 77.7。Rust 没有这一层，对位的是「静态单态化」：

- **tuple encode 与虚拟分发只差 8%**（24.1 对 22.3），说明这 1.6× 的差距来自逐成员的
  前缀写入，不来自分发方式；
- **named encode 对虚拟分发仍差 2.6×**（69.5 对 26.5），对 template 差 4.35× —— 这是本表
  里唯一的结构性差距。当时的猜测是「每个成员一次 `write_prefixed`（≈5.7 ns）加一次子
  `Encoder` 构造」，§19.15.9 逐段实测后订正为：主因是名字段的两次 `write`，值前缀的
  子 `Encoder` 只占剩下那一半；
- **decode 1.4–1.6×**，但 C++ 的 `std::string` 短串走 SSO 且不校验 UTF-8，Rust 恒分配
  + 恒校验；名字解析本身已经打平（10.8 对 11.7），差额全在值与分配上；
- **owned 路径差距更小**（tuple 1.35×）：一次 1024 B 分配把两侧拉近了一部分。

wire 依然逐字节一致：tuple / tuple object / named 三份 hex 两边完全相同，`auto` 形式也相同
（named 两边都是声明序；C# 写名字序只是为了让输出稳定，顺序不是契约，见 §19.4）。

**装置形状本身是混淆变量，数字必须带形状引用。** 同一台机器、同一份源码、同样的操作：

|行|pinned（本表）|通用 driver + lambda|上一轮 `harness.cpp`|
|:---|---:|---:|---:|
|decode tuple|55.4|105.2|74.8|
|encode named|16.0|47.0|35.6|
|decode named|69.8|150.5|66.7|

差异只来自 GCC 是否把 `Converter<T>::Decode` / `Encode` 内联进循环：case 体一旦经通用
driver 的 lambda 传入，就被留在外部调用上（`decode tuple` 直接翻倍）。本表用
`__attribute__((flatten))` + 每个 case 一个函数模板实例固定在「完全内联」这一侧，理由是
对位的 Rust 侧 LLVM 一定把单态闭包内联（没有等价开关），不固定就只是在比内联策略。
上一轮那份 C++ harness（一次性，不入库）是「通用 `bench(name, lambda)` + 少量行」，
§19.15.3 的 C++ 列出自它 —— **两轮的 C++ 绝对值不可互比**，§19.15.3 的倍率保留但只作
历史记录。

两条与 §19.15.5 一致的构建效应：C++ 不开 LTO 时 `Allocator::Append` 是跨 TU 调用，
named encode 16.0 → 94.8、`i32` encode 0.42 → 3.4；Rust fat LTO 反而让 named encode
56.0 → 69.5、decode 快 7%。官方 google benchmark 同轮给出 82 / 88 / 117（mean 口径，且
`NamedObject.cpp` / `TupleObject.cpp` 对 `Type01` 各声明一套 schema 的 ADL 冲突未消），
只作量级对照。这个「LTO 反而更慢」在 §19.15.9 找到了原因并修掉。

#### 19.15.9 named encode 的 4×：归因与改动（2026-11-08）

> 命名说明：本节记录当时的 `Encoder::named` 与私有半操作 `write_name`。现在成员
> 写入是自由函数 `write_named_member`，名字段仍是一次预留，机制与数据不变。

§19.15.8 留下一个反常点：C++ 的 named 只比 tuple 贵 1.4 ns（多 35 B、多 4 个成员），
Rust 却贵 32.7 ns。**差距按写入次数增长，不按字节数增长** —— 说明问题在每次写入的固定
成本，而不在 wire。

**归因方法**：把同一份 85 B named payload 用五种实现写出来（每种都断言与 derive 结果
逐字节相等），同一测量规则（预热 2000 次、7 轮 50 ms 取最小）计时：

|变体|内容|默认 release|fat LTO|
|:---|:---|---:|---:|
|C 裸指针 sink|只搬字节，无检查、无 `Result`|14.1|14.1|
|D 具体后端 + 完整契约|边界检查 + `Result<(), Error>` + anchor 压缩，无 enum|28.8|27.6|
|G D + `Owned`/`Borrowed` enum|加回后端 tag 与 `Vec::resize` 分支|32.4|27.2|
|B 真实 `Encoder`，名字用常量字面量|每名字一次 `write`|44.9|41.5|
|A derive 现状|`write_name` 两次 `write`|59.8|57.7|

C++ `NamedObjectConverter` 同会话是 16.0。三点结论：

- **字节搬运本身只值 ~14 ns**（与 C++ 的 16.0 同量级）。named encode 的 56~70 ns 里，
  40 ns 以上是每次写入的固定开销。
- **enum 不是原因**：G 与 D 相差 0~3.5 ns，`Owned`/`Borrowed` 的 tag 判断本身很便宜。
  同一个二进制里它只解释 3%，不解释 4×。
- **内联与否能把同一段代码搬动 ±15 ns**：D 的固有方法版（完全内联）测得 14.3，改写成
  走一层泛型 trait 的同一算法就是 28.8。这条比任何微优化都重要 —— 它决定了下面两处是
  收益还是损失。

**三条具体机制**（对应 A 相对 C 的 45 ns）：

1. `Encoder::named` 先调私有的 `write_name`：前缀写进栈上 `[u8; 4]` → `write` 前缀 →
   `write` 名字。一个成员三件事、两次运行时长度的 `memcpy`。C++ 的
   `EncodeWithLengthPrefix(Allocator&, span)` 是「一次 `Assign(n + prefix)` + 一次
   `memcpy` + 一个字节 store」。A − B = 16 ns / 6 个名字 ≈ **2.7 ns 每个名字**。
2. 每次 `write` 要过：后端 tag → 边界检查 → `copy_from_slice`（长度是运行时值，未常量
   折叠时是真 `memcpy` 调用）→ 返回 24 B 的 `Result`（`size_of::<Error>() == 24`，
   x86-64 上走 sret）。named 全程约 20 次写、6 次 anchor 压缩，这条固定成本被乘 20 次；
   tuple 只有 8 次左右。**这是「按次数增长」的来源。**
3. 默认构建里 `write_name` 没有被内联：`objdump` 数到 8 处 `call write_name`。原因是它
   的展开体带着 `Inner::Owned` 分支里的 `Vec::resize`（分配 + 容量溢出 + panic landing
   pad），超过 LLVM 的内联阈值 —— 也解释了 §19.15.8 的「fat LTO 反而 70 ns」：全内联把
   这段冷代码搬进了每一个写入点。

**改动（只动编码热路径，wire 不变）**

- 新增 crate 内 `Encoder::write_prefixed_bytes`：一次 `ensure_capacity(header + len)`，
  然后把头字节和 payload 直接写进该区域，与 C++ `EncodeWithLengthPrefix` 同形。
- `Encoder::named` 的名字段改走它（`write_name` 从两次 `write` 变成一次保留）。
- 结果（同会话，C++ 对照组三次稳定在 encode 15.9 / decode 69.9）：

|行|改前|改后|C++|
|:---|---:|---:|---:|
|encode named raw（默认 / fat LTO）|56.1 / 70.0|**42.4 / 35.4**|15.9|
|encode named owned|116.8 / 121.6|99.4 / 86.1|—|
|decode named raw|105.7 / 97.0|109.1 / 99.0|69.9|
|encode tuple raw|23.4 / 24.3|23.2 / 24.8|14.3|

  named encode 默认 −24%、fat LTO −49%，与 C++ 的比值从 3.5~4.4× 降到 **2.2~2.7×**
  （`auto` 形式 2.35×，与 C++ virtual 的 2.6× 基本持平）。wire 三份 hex 逐字节不变。
  decode 行 +3% 没有代码路径相连（`Decoder` 不经过 `Encoder`），且 C++ 对照组同会话
  三次稳定在 69.9 —— 判为布局，`encode tuple owned` 同时 −5% 也是同一原因。

**被否决的两条（留记录，别再试）**

- `String` / `Vec` 用「已知长度的 bounded 前缀」：库里**已经是**这样（`container.rs` 的
  `encode_with_prefix` 调 `write_prefixed_bounded(self.len(), …)`）。隔离实验里那 7 ns
  差异在两种构建下方向相反，判为布局噪声。
- 把扩容分支标 `#[cold] #[inline(never)]`：LTO 下 named 再快 2.5 ns（38.0 → 33.2），但
  owned 路径付 6~7 ns（tuple owned 58.7 → 62.8，默认 70.4 → 74.3），净负。

**剩下的 2.2~2.7× 在哪**：每个成员仍是「名字一次保留 + 值一次 `write_prefixed_bounded`」，
后者为了拿到 `actual` 会建一个子 `Encoder`（另一个 enum + 自己的 `pos`）再读回长度。
按上表的阶梯，把它也压平到「已知长度直接写头」是下一步唯一的量级来源 —— 已在 §19.15.10
做掉：子 `Encoder` 拿掉了，但那 2.2× 并没有跟着消失，来源是每次写入的固定成本。

#### 19.15.10 去掉值前缀的子 `Encoder`（2026-11-08）

§19.15.9 结尾把剩余的 2.2~2.7× 归到「值前缀为拿 `actual` 建了一个子 `Encoder`」。这一节把
它拆掉：`write_prefixed_bounded` 现在只在一个编码器上工作 —— 只预留 header，`f` 直接写这个
`Encoder`，写完读 `self.pos` 得 `actual`，超界报错。wire 不变。

**为什么子 `Encoder` 是错的**（是「不该存在」，不只是「慢」）：

- **两个状态机**：第二个 `Inner` 判别 + 第二个 `pos`，`f` 里每一次写都要按一个调用方从未
  选择的后端重新分支；
- **把上界当预留**：`ensure_capacity(h + max_len)` 让 owned 后端清零 `max_len` 字节（估算
  偏大就白付内存与 memset），让 borrowed 后端按上界判剩余空间 —— 明显装得下的 payload 被
  `BufferFull` 拒掉，而 C# 遇到同一情形是扩容（`Create(maxLength + numberLength)`）；
- **错误类型说谎**：`BufferFull` 的语义是「调用方给的 buffer 满了」，owned 编码器没有这个
  buffer，这个错误只因多出来的那层才存在；
- **围栏是伪需求**：`f` 里每次写都过 `ensure_capacity`，越界写不出内存，事后校验
  `actual <= max_len` 就够 —— C# 也是事后的 `if ((uint)actual > (uint)maxLength)
  ThrowInvalidReturnValue()`。

**取舍要实测**：「少一层」不等于「更快」。同一份 `objects` 负载、同一会话、六个形态交替跑
三轮取最小（默认 `--release` / fat LTO + CGU=1，ns/op；A 与 D 每轮都在，作锚点）：

|形态|named raw 默认|named raw LTO|tuple raw 默认|tuple raw LTO|named owned 默认|tuple owned LTO|
|:---|---:|---:|---:|---:|---:|---:|
|A 子 `Encoder`（改前）|42.2|35.3|23.2|24.7|98.6|55.1|
|B 单编码器 + 回滚 + 内联 message|45.7|38.6|26.4|27.1|105.5|57.8|
|C B 但 message 走 `#[cold]`|44.3|36.1|25.4|25.5|101.4|56.5|
|**D C 去掉回滚（落地）**|**42.2**|**35.8**|**23.7**|**24.9**|**104.8**|**57.8**|
|E D + owned 预留 `h + max_len`|43.8|36.5|25.1|25.3|100.5|54.7|
|F D + 只对 owned 预留（`Inner` 判别）|44.2|39.2|26.1|28.8|105.5|57.6|

三点：

- **回滚值 2 ns**（B/C 对 D）：`self.pos = at` 这条只在错误路径执行的 store 把 `pos` 钉回
  内存，整个成员再也回不到寄存器。这与 §19.15.9「扩容分支标 `#[cold]` 反而更慢」是同一类
  现象 —— **极少执行的分支不是免费的**。于是错误语义与 `write_prefixed` 对齐：出错即终止，
  不保证位置，也不保证已写下的字节构成一个成员。
- **owned 预留不是免费的**（E/F）：E 把上界当预留，borrowed 行全部变差（named 42.2 → 43.8、
  tuple 23.7 → 25.1）；F 想只给 owned 预留，那次后端判别本身就把 borrowed 拖垮
  （tuple LTO 24.9 → 28.8）。两条都否决。
- **D 与 A 的差只在 owned 行**：borrowed 打平（42.2 / 35.8 / 23.7 / 24.9 对
  42.2 / 35.3 / 23.2 / 24.7，最差 +2%），owned 慢 4~6%（104.8 对 98.6、57.8 对 55.1）。
  owned 的差来自每个 payload 写各自的 `ensure_capacity` + `Vec::resize`，而 A 是「一次
  `h + max_len` 预留」；用 4~6% 换掉一个编码器、一个后端、一个说谎的错误类型，划算。

官方 `run-all.sh` 复跑（`results/run-*.txt`，fat LTO，三轮取最小）落在同一区间：
`encode named` 35.4 → **36.4**、`encode tuple` 24.8 → 25.2、`encode named owned`
87.7 → 86.9、`encode tuple owned` 56.8 → 59.0；默认构建 42.4 → 42.9 / 23.2 → 24.1。
C++ 对照组同会话不变（named 16.1、tuple 14.5、parse 11.7），所以 named encode 对
C++ template 从 2.23× 变成 **2.26×**，tuple 从 1.72× 变成 **1.74×**。

还有一条行为改进（有测试钉住）：borrowed 上「估算偏大」从 `BufferFull` 变成正常成功
（`test_write_prefixed_bounded_estimate_is_not_charged_to_borrowed_space`），payload 超过
声明上界则报 `Error::Message`（`test_write_prefixed_bounded_overshoot_is_an_error`），
不再是借来的 `BufferFull`。

#### 19.15.11 基础 tuple：按步骤拆解，前缀路径的最后一刀（2026-11-08）

§19.15.10 之后 tuple encode 仍是 C++ 的 1.6~1.7×。这次先把 25 ns 拆开再动手。

**装置**：`examples/tuple_steps.rs`（临时，量完即删）与 `/tmp/cppstep` 的同构 C++ 版，
规则同 `objects`（1024 B borrowed、2000 warmup、7 × 50 ms 取最小、每 case 一个函数模板，
C++ 侧 `flatten`）。负载 `(i32, String, Vec<i32>, (f64, Vec<String>))` = 50 B，5 个前缀。
下表是同一会话、三轮取最小（ns/op）；"改前"= §19.15.10 的落地形态。

|步骤|Rust 改前|Rust 改后|C++ LTO|
|:---|---:|---:|---:|
|S0 只构造 `Encoder` / `Allocator`|0.42|0.42|0.42|
|S1 一次写 50 B（memcpy 下界）|2.14|2.13|2.32|
|S2 12 次裸写（前缀写字面量）|16.4|16.4|12.95|
|S3 成员走真实 API（手写展开）|27.2|24.4|14.5|
|S4 tuple `encode`（真实，探针内）|24.4|16.8|14.5|
|M1 `i32` auto|1.01|1.05|0.10|
|M2 `String` auto（前缀路径）|6.2|2.6|2.31|
|M3 `String` 两次 `write`|3.4|2.6|3.37|
|M4 `Vec<i32>` auto|5.8|6.2|2.32|
|M5 `Vec<i32>` 手写|5.8|4.9|2.88|
|M6 `f64`|0.95|1.05|0.21|
|M7 `Vec<String>` auto|18.4|15.6|9.33|
|M8 3 × `String` auto|16.4|10.3|8.1|
|M9 3 × `String` 手写|10.8|10.9|8.1|
|N1 名字段：`named("Name", …)` / `EncodeWithLengthPrefix`|6.5|5.5|1.06|

三条读数：

1. **tuple 层不花钱**：S3（手写展开，成员走真实 API）与 S4（`t.encode(e)`）在同一构建下
   互有胜负，差 ≤10%，就是函数落位；4 元组的分发、`auto` 转发、宏展开都是 0。**别再怀疑
   tuple 层。**
2. **钱在「每个变长成员的前缀」**：S2 → S3 是「把字面量前缀换成真实 API」，Rust 改前
   +10.8 ns，C++ 只有 +1.5 ns。逐成员看更清楚 —— `String` 前缀 Rust 6.2 vs 两次 `write`
   3.4（贵 2.8），C++ 反过来：`EncodeWithLengthPrefix` 2.31 vs 两次 `Append` 3.37（**一次
   完成比两次 Append 便宜**）。同样的语义，两边符号相反，这就是缺口。
3. **原因在 API 形状**：`String::encode_with_prefix` 走的是
   `write_prefixed_bounded(len, |e| e.write(bytes))` —— 长度本来精确，bounded 只是借来复用。
   它要 `ensure(h)` → 手动推进 `pos` → `f` 里再 `ensure + memcpy + pos` → 事后回写 header，
   两次容量检查、两次 `as_mut_slice`、两次 `pos` 更新；`write_prefixed_bytes` 只有一次预留、
   一次写头、一次 memcpy。

**改动**：`String` / `str` / `[u8]` / `&str` / `&[u8]` 的 `encode_with_prefix` 改走
`write_prefixed_bytes`（`encoder.rs` 里本来只给成员名用，§19.15.9），并把它的 >127 分支拆成
`#[cold] #[inline(never)]`，1 字节臂只留「ensure + 写头 + memcpy」。`Vec<T>` **不动**：
M4/M5 说明 POD 容器的 bounded 与手写打平（16 B memcpy 主导），改它没有收益。wire 不变
（头宽仍由 `prefix_header_len(len)` 决定，`test_long_prefix_boundary` 覆盖 127/128 边界）。

**一处反常，必须记住**：同一个 `tuple encode` 体，`objects` 里 fat LTO 是 23.0，探针里
16.8；默认构建反过来（16.0 / 22.1）。`#[inline(always)]` 加在装置 helper 上、或者改写调用
形态（闭包里直写 vs 过 `encode_raw`），都不动它 —— 变的是函数在二进制里的落位。于是：

- **跨语言比值只在「各语言最优构建」下取**：Rust 16.1（默认 `--release`）对 C++ 14.5
  （`-O3 -flto`）= **1.11×**。同配置 LTO/LTO 的 23.0 / 14.5 = 1.59× 里有约 6 ns 是落位，
  不是实现。C++ 不能不开 LTO（`Allocator::Append` 跨 TU 会留在外部调用上，14.5 → 39.2），
  Rust 反过来 LTO 更慢（16.1 → 23.0），所以「同配置」在这对语言之间测的是构建系统。
- **单个 binary 里的一行也不是常量**：±10% 的互有胜负是落位，`decode` 行同样如此。
  要下结论就换形状再测一次，或者只取量级。

**改后总表**（`run-all.sh`，三轮最小，ns/op；C++ 为 `-O3 -flto`）：

|行|改前默认|改后默认|改前 LTO|改后 LTO|C++ LTO|
|:---|---:|---:|---:|---:|---:|
|`encode tuple (raw)`|24.1|**16.1**|25.2|23.0|14.5|
|`encode tuple object (raw)`|25.5|**21.5**|24.0|22.3|14.5|
|`encode named (raw)`|42.4|**38.0**|36.4|33.9|16.0|
|`encode tuple (owned)`|70.5|**66.8**|58.6|54.8|40.8|
|`encode named (owned)`|103.5|**97.0**|86.9|80.1|53.9|
|`decode tuple (raw)`|95.7|91.0|84.0|84.5|55.7|
|`named parse only`|12.7|12.7|10.7|10.8|11.8|

tuple 的 1.75× 消失；named 仍是 2.1×（它的一半成本是 4 个名字段，§19.15.9 已修，剩下的
是「值前缀 + 名字段各一次 write_prefixed_bytes」的固定开销，C++ 那份固定开销更小 —— M1/M6
显示 Rust 单次 `write` 1.0 vs C++ 0.1~0.2）。**tuple 与 named 现在是同一类差距，不再有
「named 特有」的部分。**

#### 19.15.12 `Error` / `Result` 有多大，以及它在热路径上花了多少（2026-11-08）

尺寸（x86-64，`size_of` / `align_of`，全部 8 字节对齐）：

|类型|size|说明|
|:---|---:|:---|
|`Error`|**24**|最大 payload 24 + 判别字节，补齐到 8|
|`Result<()>` / `Result<usize>` / `Result<&[u8]>` / `Result<[u8; 16]>` / `Result<(i32, i32)>`|**24**|与 `Error` 同宽|
|`Result<String>` / `Result<Vec<u8>>` / `Result<Vec<String>>`|**32**|24 字节 `String`/`Vec` + 判别字节|
|`Option<Error>`|24|判别值填在 `Error` 的 niche 里|
|`Cow<'static, str>`|24|见下|
|`ErasedError`（`Arc<dyn Error>`）|16|胖指针|
|`&'static str`|16||
|`Result<(), ()>` / `Result<(), u8>`（基线）|1 / 2||

**为什么是 24**：三个大 payload 撑着 —— `Message(Cow<'static, str>)` 24 字节、
`MissingMember(&'static str)` 16、`Wrapped(ErasedError)` 16；再加 1 字节判别，按 8 对齐
就是 24。**把 `Cow` 装箱到 8 字节并不能降下来**：实测只改 `Message(Box<Cow<…>>)` 后
`Error` 仍是 24（16 字节 payload + 判别字节向上取整）。要真的降到 16，得把
`MissingMember(&'static str)` 和 `Arc<dyn Error>` 一起装箱 —— 错误路径多一次分配、公开 API
变形，换 8 字节，不做。

**成功路径不搬这 24 字节**：`Ok(())` 只写一个判别字节，payload 保持未初始化（原始字节里只有
首字节是确定值，其余是栈上残留）；`Result<()>` 与 `Option<Error>` 都不额外加 tag，判别值填在
`Error` 自己的 niche 里。`Result<String>` 是 32 而不是 24，是因为 `String` 没有可用 niche。

**代价只在「不被内联的边界」上收**：SysV 上 >16 字节的返回值走隐藏指针（sret），所以
`Result<(), Error>` 一旦跨过 `#[inline(never)]` 边界，就是「caller 栈上开 24 字节 + 传地址 +
callee 写判别 + caller 读判别」。实测（同一 50 字节 tuple 体、只改返回类型、`#[inline(never)]`、
5 轮取最小、3 次重复，ns/op）：

|返回类型|三次重复|
|:---|---:|
|`()`|28.33 / 24.17 / 28.16|
|`Result<(), u8>`|24.61 / 24.81 / 28.35|
|`Result<(), Error>`|24.81 / 24.90 / 28.57|

这三个数整体随函数落位跳（24 vs 28 两档，同 §19.15.11 的落位现象），所以只看**同一轮内**的
差：`Result<(), Error>` 相对 `Result<(), u8>` 只多 **0.09~0.22 ns**，相对 `()` 是
−3.52~+0.73（噪声覆盖）。结论：24 字节错误通道的价格 ≈ **0.2 ns/call**，与 2 字节结果几乎
不可分；真正贵的是「过边界」本身，不是错误类型的大小。

**热路径上没有这个边界**：`Encoder::write` / `ensure_capacity` / `encode_with_prefix` 全是
`#[inline]`，泛型在调用方单态化，`?` 在成功路径上退化成一个判别比较，LLVM 不会为 `Result<()>`
开槽。所以 §19.15.11 的 16.1 ns 里没有「错误返回值」这一项，把 `Error` 缩到 16 字节也不会
更快。反过来算一下量级：tuple encode 有 17 次写入，**假如**每次都落在非内联边界上，
17 × 0.2 ≈ 3.4 ns —— 比现在整条 tuple encode 的差距还大；实际上一次都没发生。

**对照**：C++/C# 走异常，成功路径的返回值是 `void`，错误通道 0 字节；Rust 的 24 字节是
「不用 unwinding」换来的，价格就是上面那 0.2 ns/call。已定型：不为缩 `Result` 去装箱 payload。

#### 19.15.13 owned 后端的增长模型（已落地，2026-11-08）

§19.15 里「owned 比 borrowed 慢 2.5~4×」的归因不是分发、不是前缀，而是**每段写入都调一次
`Vec::resize(needed, 0)`**：12 段写入就是 12 次容量判断 + 12 次 memset，而 memset 写的正是
紧接着被 memcpy 覆盖的那段字节。

拆分（临时探针，同会话，ns/op，51 B 负载，`encode_auto`）：

|场景|ns/op|
|:---|---:|
|borrowed `on_slice`|23.6~24.0|
|owned `new(256)` + `finish`|60.9~67.5|
|owned `with_capacity(1024)` + `finish`|61.5~61.9|
|owned `new(256)`，不 `finish`|50.3~51.3|
|`Vec::with_capacity(256)` 单独|5.35|
|`Vec` 256 + `resize(50, 0)`|5.40|

读法：malloc 只占 5.3，`finish`（含 drop/free）约 10，剩下 25~35 全在 encode 内部那 12 次
`resize` 上。`resize(50, 0)` 单独只要 5.40，说明 memset 本身不贵 —— 贵的是**每次写入都重新
走一遍 resize 的容量判断 / len 更新**。

**落地形态：水位线放构造期，不碰 `ensure_capacity`。** `Encoder::with_capacity(n)` 先用
`resize(min(n, COMMIT_AHEAD), 0)` 把 `Vec::len` 抬到水位线（`COMMIT_AHEAD = 128`），
`ensure_capacity` / `ensure_prefix_tail` 一字不改。`len` 从此表示「已提交到哪」，只有 `pos`
表示写了多少，`finish` / `as_bytes` 都按 `pos` 截断，所以 **wire 逐字节不变**（三语言 hex 全同）。

六个候选形态，base 与候选在同一会话交替、各跑 3 轮取最小（`objects` + `collections`，ns/op；
括号是该列相对 base 的比值）：

|形态|改在哪里|owned tuple|owned named|`tuple auto`（borrowed）|`vec<i32> x1024` 逐元素|
|:---|:---|---:|---:|---:|---:|
|base（无水位线）|—|63.9~65.6|93.6~96.3|21.4~21.9|79.7|
|A：`#[cold]` helper|`ensure_capacity`|51.1|67.4|25.7（+19%）|未测|
|B：A + 尾部同 helper|+ `ensure_prefix_tail`|49.4~50.4|66.3~66.7|25.0~25.6（+17%）|**788（9.9×）**|
|C：内联算术|`ensure_capacity`|51.7~52.6|62.8~65.2|25.0~25.1（+15%）|**2219（27.8×）**|
|H：B 去掉 `#[cold]`|`ensure_capacity`|51.2|67.8|25.9（+21%）|未测|
|M：提交整个 capacity|`with_capacity`|58.3|69.8|22.3（+4%）|80（1.01×）|
|**M2：提交 `min(cap, 128)`**|`with_capacity`|**47.0**|**61.5**|22.8（+4%）|80（1.01×）|

三条结论：

1. **水位线不能放在写入路径上**。`vec<i32> x1024`（同一 encoder 逐元素写 1024 次）在 base 是
   79.7 ns，B 变 788、C 变 2219 —— base 那个数字本身就意味着 LLVM 把整段循环折叠 / 向量化了，
   owned 分支里多出来的工作让它无法跨迭代优化。A/H 与 B 的差别（`#[cold]` 与否）只是把
   borrowed 行在 ±5% 内挪位置，不是原因。这条比 1~2 ns 的读数重要得多，也解释了为什么凡动过
   `ensure_capacity` 的形态，`tuple auto` 都 +15~21%。
2. **构造期提交同时拿到收益和干净的热路径**。M2 的 owned 收益比 B 还大（tuple 47.0 对 49.4、
   named 61.5 对 66.3），因为它连写入时的容量判断都不再触发；borrowed 六行全部落回 base 的
   ±4~6% 噪声带（`i32` 0.44、`vec<loop>` 80 都不动）。代价只有「构造时零填充
   `min(capacity, 128)` 字节」。
3. **水位线长度就是代价**。M 三档实测：提交 128 / 256 / 512 时 owned tuple 54.8 / 58.5 / 65.5、
   named 67.3 / 70.2 / 76.6，约 0.02 ns/B —— 先按容量付费。所以取 128（覆盖 50 B / 85 B 两种
   对象形状），而不是整个 capacity。

全矩阵（同会话三轮最小，`run-all.sh`；Rust 默认 release / fat LTO，C++ `-O3` / `-O3 -flto`）：

|行|Rust|Rust LTO|C++|C++ LTO|最优/最优|
|:---|---:|---:|---:|---:|---:|
|encode tuple (raw)|15.9|22.9|40.0|14.7|**1.08×**|
|encode named (raw)|38.0|34.8|97.1|16.3|**2.13×**|
|encode tuple (owned)|47.2|36.6|60.2|41.6|**0.88×**|
|encode named (owned)|61.6|46.9|112.0|55.0|**0.85×**|
|decode tuple (raw)|94.6|93.0|65.0|56.7|1.64×|
|encode i32 (raw)|0.44|0.64|3.4|0.43|1.02×|

即 owned 一侧从此**不再落后于 C++**（0.85~0.88×）；borrowed 的 tuple / named 仍是
1.08× / 2.13×，与 §19.15.11 的口径一致。

**残余**：owned 仍比 borrowed 慢（tuple 47.2 对 15.9、named 61.6 对 38.0）。这一层已经不是
memset —— malloc + drop/free 约 10 ns（上表的 5.35 加 `finish`），剩下是每段写入一次的容量判断
与 `pos` 更新。要再压只能自管 buffer（不用 `Vec`），不在当前计划内。

**超过水位线的载荷**：`> COMMIT_AHEAD` 的 payload 先付 128 B 零填充，然后回到「每次写入一次
`resize`」的老行为 —— 与改动前相同，只多这一笔（几纳秒）。

#### 19.15.14 `Vec<String>` 解码预分配：扫描不划算，杠杆在借用（2026-11-08）

§6 第 3 条留了一个没量过的疑问：变长元素（`T::LEN == 0`）的 `Vec<T>` 解码是 `Vec::new()` +
`push`，容量靠增长；先扫一遍前缀数出元素个数、再 `with_capacity(count)` 是否划算。用一次性
探针（只调公开 API，不入库）在同一个二进制里比四种写法，负载是
`"s0".."s7"` / `"s0".."s63"`：

|写法|x8（24 B）|x64（246 B）|分配数|
|:---|---:|---:|---:|
|现状：`Vec::new()` + `push`|150|1702|10 / 69|
|扫描计数 + `with_capacity(count)`|132|1722|9 / 65|
|不扫描，按 payload 字节数上界预留（封顶 1024）|125|1625|9 / 65（预留 592 / 6086 B）|
|`Vec<&str>` 零拷贝|73|470|2 / 5|

（ns/op，三轮取最小；扫描本身 8 元素 6.3 ns、64 元素 88.7 ns，约 1.4 ns/元素。）

两条结论：

1. **扫描计数不划算**。`Vec` 的增长在 64 元素上只多 4 次 realloc（约 14 ns/次），扫一遍
   `read_prefix` 却要 88.7 ns —— 净慢 1.2%。小集合（8 元素）能快 12%，因为省下 1 次 realloc
   只花 6.3 ns 扫描；但那个规模整条只有 150 ns，省下的是 18 ns。
2. **免扫描的上界预留有收益、内存不可控**。每元素至少 1 字节前缀，所以 payload 长度是元素数
   的上界：64 元素快 4.5%，代价是预留 6086 B、实用 1718 B（3.5×），元素越短放大越多（1 字节
   元素就是 24×）。不采纳。

真正的杠杆不在预分配：`Vec<String>` ×64 的 1.7 µs 里，64 次 `String` 分配是主体，`Vec<&str>`
零拷贝只要 470 ns（3.6×）。这与 P1 `borrow` 的结论同向 —— 想解快，调用方应当解到 `&'de str`，
而不是让解码器去猜元素个数。**§6 第 3 条在变长元素上按此结论关闭**；定长元素那条本来就已经
落地（`decode_slice` 的默认实现是 `with_capacity(count)` + push，`count` 由前缀算出）。
解码侧**没有**「长度已知」这回事，所以才只能猜元素个数，而猜不划算；编码侧正相反 ——
payload 在内存里，容量由调用方在构造期给上界，收益实测到 1.9×（§19.15.15）。

#### 19.15.15 容量预算在构造期，不在写入路径（2026-11-08，用户裁定）

编码侧和 §19.15.14 的解码侧不是同一个问题：编码时 payload 已经在内存里，**长度可以提前知道**，
所以「预留多少」是调用方的决定，不是库的启发式。三条实现都只把这个决定放在构造处 ——
Rust `Encoder::with_capacity(n)` / `Encoder::on_slice(&mut [u8])`，C++ `Allocator(std::span<std::byte>)`
/ `Allocator(IAllocator&, maxCapacity)`，C# `Allocator(Span<byte>)` /
`Allocator(IAllocator, int maxCapacity)`；**没有一个提供写入中途的 reserve**。这不是巧合，是
同一套 wire 实现共享的形状：容量在创建时定，写入路径只做容量判断。差别只在「预分配」怎么表达：
Rust 的 owned 后端收一个**初始容量**；C++ / C# 的 growable 重载只收 `maxCapacity` 这个**上限**
（底下由 `IAllocator` / `ArrayPool` 按需增长），真要预分配就把已经分配好的 buffer 包成 span
传进去 —— 也就是三边都有的 `on_slice` / `Allocator(span)` 形态。

一次性探针（同 harness 规则：`black_box` + 七轮取最小，三轮读数
一致，ns/op）：

|负载（wire 字节）|`new()`（256 种子）|`with_capacity(wire)`|`with_capacity(payload)`（少 1 B）|`on_slice(wire)`|
|:---|---:|---:|---:|---:|
|`String` 8 B（9）|16.2|16.9|22.9|3.3|
|`String` 64 B（65）|16.6|17.8|34.4|3.2|
|`String` 1024 B（1028）|66.5|**35.1**|58.2|11.5|
|`Vec<String>` ×100（804）|621|587|—|402|

三条读法：

1. **水位线以内，预分配没有收益**。9 / 65 字节的 wire 全部落在 `COMMIT_AHEAD = 128` 里，
   `new()` 与精确预分配同速（16.2 对 16.9，差 0.7 ns、方向稳定，属于分配器 size class 的差别，
   不是增长）。这与 §19.15.13 的 `new(256)` ≈ `with_capacity(1024)`（60.9 对 61.5）是同一件事。
2. **水位线以上是 1.9×**。1 KiB 负载 `new()` 66.5 ns 里有 31 ns 花在 256 → 1028 的那一次增长上
   （1 次 `realloc` + 拷贝），`with_capacity` 只剩 35.1；100 串那条只差 5%，因为 100 次元素写入
   把它摊薄了。
3. **估成下界比不估更差**。`with_capacity(payload)` 少算那 1 字节前缀：64 B 负载从 16.6 涨到
   34.4（比不估慢 2.1×），1 KiB 只从 66.5 降到 58.2。所以传给 `with_capacity` 的必须是**上界**，
   而且要连每个成员的前缀、每个容器的头一起算进去；估不准就交给 `new()` 的 256 兜底。

`on_slice` 那一列顺带标出 owned 的固定成本：9 字节负载 3.3 对 16.2，差的 13 ns 就是一次
malloc + free（§19.15.13 拆过：malloc 5.3、`finish` 里的 free 约 10）。长度确定、缓冲可复用时，
借用后端把这两笔一起省掉。

因此分工是：**解码侧不在 `Vec::decode` 里猜元素个数（§19.15.14 否决），编码侧在 `Encoder`
构造时由调用方给上界**。库这边不做增长启发式 —— `ensure_capacity` 只 `resize` 到
`pos + additional`；「grow to needed + headroom」那版曾把 `vec<i32> x1024` 从 80 ns 打到
788 / 2219 ns（§19.15.13），形态问题比省下的几次 realloc 重要得多。

#### 19.15.16 `decode tuple` 的 94 ns 拆成四块（2026-11-08）

§19.6 一直记着「decode 行是拥有型，倍数里混了分配与逐元素」，但没拆过。探针（一次性，不入库）用**同一条 50 字节 wire**，只把字符串字段在 `String` /
`&'de str` / `&'de [u8]` 之间替换，字段顺序与其余类型不变，三轮取最小（ns/op）：

|变体|ns/op|分配数|
|:---|---:|---:|
|owned：`(i32, String, Vec<i32>, (f64, Vec<String>))`|94.1|6|
|名字 `&str`，tags 仍 `Vec<String>`|90.2|5|
|名字 `String`，tags `Vec<&str>`|72.6|3|
|全部借用（仍校验 UTF-8）|63.0|2|
|全部借用且不校验（`&[u8]`）|33.8|2|
|只走 wire、不建值（手工 skip）|2.4|0|

相邻两行相减就是各块的价钱：

|项|ns|占 94.1|
|:---|---:|---:|
|4 次 `String` 分配（含 17 B 拷贝）|31.0|33%|
|4 次 UTF-8 校验（17 B）|29.3|31%|
|2 次 `Vec` 分配 + 元素拷贝 + 控制流|31.3|33%|
|走 wire（读前缀、推进 `pos`）|2.4|3%|

单点对照（同一轮）：`String` 6 B **18.2** / `&str` 6 B **8.0** / `&[u8]` 6 B **5.1**；
`malloc+free` 在 6 / 16 / 24 / 48 B 上都是 **8.6~9.0**（这几档同属一个 size class，成本与尺寸
无关）；`Vec<String> ×3` **63.3** 对 `Vec<&str> ×3` **41.5**（差 21.8 = 3 × 7.3，正是 3 次
`String` 分配）；`Vec<i32> ×4` **15.8**（1 次分配 8.7 + 16 B 整块拷贝）。

两条结论：

1. **那 31 ns 是「Rust `String` 没有 SSO」的价钱**。C++ 同一装置的 `decode tuple` 计到 **4 次
   分配**、`decode std::string (6 chars)` 计到 **0** —— 负载里的 6/3/3/5 字符全部落在
   `std::string` 的 SSO 里，那 4 次全是 `vector`（`vector<string>` 无 reserve 的 1→2→4 增长占
   3 次）；Rust 是 2 次 `Vec` + 4 次 `String` = **6 次**。同会话 C++ `decode tuple` **78.2 ns**
   对 Rust 94.1（**1.20×**）：扣掉「4 次分配 + 4 次校验」的 60 ns，Rust 的剩余路径（33.8）
   只有 C++ 的一半不到 —— `Vec<i32>` 是 1 次分配加整块拷贝，而 C++ 那条三次增长的
   `vector<string>` 更贵。**「Rust 解码慢」不是解码逻辑慢。**
2. **UTF-8 校验是每次调用的钱，不是每字节的钱**：`str::from_utf8` 在 1 B 上 5.3、6 B 5.8、
   64 B 5.8、256 B 7.6、1 KiB 20.9（≈0.016 ns/B ≈ 60 GB/s）。短串多的时候它是大头（4 个串
   29.3 ns，平均 7.3）；长串少的时候可以忽略（1 KiB 一个串 21 ns）。

落点：**要快就借用解码**（§19.17）。`&'de str` 省掉那 31 ns 分配、保留校验；`&'de [u8]` 把
29.3 ns 也省掉、总共 2.8×。但拥有型与借用型不是同一个被测物（§19.15.1 的口径），要三边同表
就得整行一起换：Rust `&[u8]` / C++ `std::string_view` / C# `ReadOnlyMemory<byte>`。

口径提醒：C++ 的 78.2 与 §19.15.8 记的 65.0 是**不同构建**（`build-benchmarks` 的 `libbinary.a`
重建过），跨会话不可比，本节只用同会话的一对数字。探针第三轮出现 +10~60% 的整体漂移，
表里取三轮最小值。

#### 19.15.17 owned 后端的写回：大 payload 原来要写两遍（2026-11-08）

§19.15.13 把 `Vec::len` 定成水位线（构造期提交 `min(cap, 128)` 字节），换来的是
`ensure_capacity` 保持原样。当时没记代价：水位线以上每长一截都走 `Vec::resize(len, 0)`，
也就是**先把要写的字节清零，再让 `memcpy` 覆盖一遍**。wire 与 API 都没有问题，白花的是
一倍写带宽。

一次性探针（同 harness 规则，三轮取最小，ns/op）：

|负载|`resize`（清零）+ 拷贝|`extend_from_slice`|`spare_capacity_mut` + `set_len`|
|:---|---:|---:|---:|
|64 B|9.2|7.8|7.7|
|4 KiB|69.2|40.1|39.4|
|64 KiB|1359.8|921.4|917.0|
|1 MiB|28805.5|18606.8|18610.0|

三列说明两件事：安全的 `extend_from_slice` 与 `spare_capacity_mut` + `set_len` 打平（40.1 对
39.4），清零那一列在 4 KiB 上就贵 72%、1 MiB 上贵 55%。同一探针的端到端读数：

|用例|改前|改后|
|:---|---:|---:|
|`String` 4 KiB，`with_capacity(wire)`|81.0|52.3|
|`String` 64 KiB，`with_capacity(wire)`|1375.4|927.4|
|`String` 64 KiB，`on_slice`|913.2|912.3|
|`Vec<i32>` ×1024，`with_capacity(wire)`|75.5|49.2|
|`Vec<i32>` ×16384，`with_capacity(wire)`|1372.7|930.3|

改完 owned 与 borrowed 只差一次 malloc + free（64 KiB 上 927 对 912），这也解释了 §19.15.15
里 owned 那一列的固定成本。实现只动 `Encoder::write` 的 owned 分支：`src.len() >=
BULK_WRITE_MIN`（256）时，先把落在已提交区间里的头部就地拷进去，其余 `extend_from_slice`
追加；小于 256 字节仍走原路径 —— 那个规模清零只值 1~2 ns，而 `write` 在标量路径上，多一个
分支不划算。`write_prefixed_bytes_wide` 改成「先写 4 字节头，payload 交给 `write`」，它原来
在一次 `ensure_capacity(4 + len)` 里把整段一起清零。水位线模型没变：`pos` 之外的字节仍然不会
被观察到，`finish` / `as_bytes` 仍按 `pos` 截断，wire 逐字节不变。

**这不是「Rust 特有的开销」，而是三边里只有 Rust 这么写**：C++ `Allocator::Resize`
（`src/Allocator.cpp`）在 `bridge == nullptr` 时是 `malloc` + `memcpy` 已写前缀，`bounds` 与
`offset` 分开，从不给未写区域清零；C# `Allocator` 同样是 `bounds` / `offset` 两个字段加
`IAllocator`（`ArrayPoolAllocator` / `NativeMemoryAllocator`）扩容。所以这一改是把 Rust 拉回
同一起跑线，不是拿不公平的优势。不走 `spare_capacity_mut` + `set_len`：它需要 `unsafe`，还会
破坏 `as_mut_slice` 的「整段已初始化」不变量 —— §19.15.7 的 ≤16 字节压缩要**读**锚点之后的
字节（`move_u64` 读源块），读未初始化内存是 UB，Miri 也会报。`extend_from_slice` 把同一件事
做在 `Vec` 内部、不暴露未初始化视图，数字还打平。

阈值 256 的依据就是上表：64 B 差 1.4 ns，4 KiB 差 29.1 ns。取 256 留了两档余量，也保证
`vec<i32>` ×64（260 B wire）这种小容器不会被拖进新分支。

口径：改后的 A/B 用两份 bench 二进制各跑三轮取最小，`encode tuple (owned Vec)` 48.6 → 49.4、
`encode named (owned Vec)` 62.8 → 64.0；同一轮里**没被改到**的 borrowed 行同时漂了 +9~10%
（`tuple (auto, on_slice)` 22.8 → 25.0），所以那 2% 是代码落位噪声，不是新分支的价钱。大
payload 的 30% 才是这一节的结论。

**容器解码的「中间 `Vec`」不是热点（同轮否决）**。`HashMap` / `BTreeMap` / `VecDeque` /
`LinkedList` / `BinaryHeap` 的 `decode` 都是先解成 `Vec<T>` 再 `collect()`，直觉上应该改成
边解边插。实测相反（同一探针，ns/op，括号内分配次数）：

|用例|现在的「`Vec` + `collect`」|直接插/推|
|:---|---:|---:|
|`VecDeque<u32>` ×64|**7.6**（1）|107.4（5）|
|`VecDeque<u32>` ×1024|**39.1**（1）|906.6（9）|
|`HashMap<u32,u32>` ×64|**644.8**（2）|1595.7（6）|
|`HashMap<u32,u32>` ×1024|**11896**（2）|24684（10）|
|`BTreeMap<u32,u32>` ×64|**403.0**（8）|701.9（10）|
|`BTreeMap<u32,u32>` ×1024|**6925**（97）|28664（171）|

原因就是 §6 第 2 条：`Vec<T>` 遇到定长元素会走元素的 `decode_slice` 钩子，**整块 `memcpy`**；
直接循环则退化成逐元素 `T::decode`（每次一次 `take` 加边界检查）。`VecDeque` 只花 1 次分配，
说明 `collect` 连缓冲区都复用了。`BTreeMap` 的 `collect` 另外拿到 `size_hint`，可以一次建树，
逐条 `insert` 要反复分裂节点。结论：**中间 `Vec` 是快路径的一部分，不是浪费**，这条不改；
真要改得让元素钩子直接写进目标容器，那要求容器暴露未初始化尾部，是另一类接口问题。

---

#### 19.15.18 "allocator 不预清零"：append-only 模型实测否决（2026-11-08，用户裁定）

原则（用户 2026-11-08 定）：C# `Allocator` 用 `bounds` / `offset` 两个字段，扩容只是
`new byte[bounds]` 或 `ArrayPool` 取块 + 拷已写前缀，从不主动清零；C++ `Allocator::Resize`
同样只有 `malloc` + `memcpy` 已写前缀。**每个转换器对自己从 allocator 划出的那段负责**：
写多少填多少，没写的字节不进 wire。Rust 的 `Vec<u8>` 做不到「划一段不填」—— `len` 就是
已初始化字节数，`&mut [u8]` 不能指向未初始化内存 —— 所以 owned 后端只有两种等价物：

1. **水位线**（现状，§19.15.13）：构造期 `resize(min(cap, 128), 0)` 把 `len` 抬到水位线，
   之后的写入落在已初始化区；水位线字节全在 `pos` 之外、`finish` 截断，不进 wire。
2. **append-only**：`len == pos` 恒成立，每个写点改成 `Vec::push` / `extend_from_slice`，
   allocator 一个字节都不填，语义上最贴近 C#/C++。

第二种整份实现出来做了 A/B（同一台机、release、`objects` + `collections` 各 4 轮取最小，ns/op）：

|行|水位线|append-only|差|
|:---|---:|---:|---:|
|`encode tuple (auto, on_slice)`|25.0|27.0|+7.9%|
|`encode tuple (owned Vec)`|49.4|53.3|+7.9%|
|`encode named (raw, on_slice)`|41.2|46.7|+13.6%|
|`encode named (owned Vec)`|65.7|79.8|+21.4%|
|`encode vec<i32> x1024`（逐元素）|80.0|128.2|+60%|
|`encode vec<bool> x1024`（逐元素）|976.9|1072.9|+9.8%|
|decode 各行|—|—|±3%（噪声）|

三个来源，逐个都能对上：

- 窄前缀臂（`write_prefixed_bytes`）原本是「一次容量检查 + 两次直写」，append-only 下变成
  `push` + `extend_from_slice` 两次检查，4 个 name 段就是 named 那 +13.6% 的主要部分；
- `write_prefixed` 的 4 字节 anchor 占位从「把 `pos` 推过去」变成真的写 4 个字节，缩头时还
  要把 `(-length) & 7` 个 pad 字节写出来再截掉（C++ 那边 pad 是垃圾，Rust 必须初始化才能
  被 `move_u64` 读）；
- 最要命的一条：`write` 的 owned 臂从「一次 `len` 比较」变成 `extend_from_slice`（内含
  `reserve` 与可能的 realloc），它被内联进 borrowed 的逐元素循环后 LLVM 不再向量化，
  `vec<i32> x1024` 80 → 128 ns。这与 §19.15.13 记的「增长策略不能放进写入路径」是同一个
  机制：**owned 臂多一点工作，borrowed 的循环就少一条向量指令**。

结论：**保留水位线**。它满足原则里可验证的那一半 —— 没有转换器依赖别人留下的字节，wire 与
缓冲区初值无关；代价是构造期一次 ≤128 B 的填充（≈4 ns），而 §19.15.17 已把大 payload 的
「写两遍」去掉。换成真正的 `bounds` / `offset` 需要 `unsafe` + `MaybeUninit`，会毁掉
`as_mut_slice` 的「整段已初始化」不变量（§19.15.7 的压缩要读锚点之后的字节），收益上限就是
那 4 ns —— 不做。

**原则落成了断言**：`tests/buffer_contents.rs` 把 33 个 fixture case 分别编码进 owned、
零填充切片、`0x5A` / `0xFF` 填充切片、以及 `with_capacity(0)`（无水位线，必须增长一次），
要求逐字节一致。反过来，这也是唯一能发现「转换器预留了两倍只写一半」的地方：owned 路径的
水位线本来就是 0，泄漏的未写字节在那边看起来就是合法的零字节。借用缓冲留 16 B slack ——
`write_prefixed` 的压缩需要 `(-length) & 7` 的余量，不够就保留 4 字节头（合法 wire，C#/C++
同规则），那是「可用空间」的差异，不是「初值」的差异。

顺带修了一处测量污染：`profile` 的 `Encoder::write_byte (borrowed)` 行原来把 `black_box`
放在循环里，每次迭代一次 store 加一次依赖它的 load（store-to-load forwarding），报出
22.19 ns，而同一组的 `write 1B` 是 2.19 ns。把 `black_box` 提到循环外后是 **2.15 ns**，与
`write 1B` 持平 —— 之前那个数字描述的是 harness，不是库。凡是要在循环里重复的标量都必须
提到循环外；这不是优化，是让数字描述库。

---

### 19.16 把 `POD` 收回内部：容器改为委托元素钩子（2026-11-08，用户裁定）

**问题**。§19.10 方案 A 把 `POD` 收紧成双向契约，但它是**公开 trait 上的普通关联常量**：

```rust
impl Layout for MyType { const POD: bool = true; }   // 安全代码，编译器一条也不查
```

Rust 没有 `unsafe const`，也没有语言级的「无 padding / 任意 bit pattern 合法」约束，那五条
契约只写在文档里。误设的后果按方向分：编码侧把 padding 的未初始化字节写进 wire（读未初始化
内存）；解码侧 `copy_nonoverlapping` + `set_len` 把**不可信输入**采纳成值 —— `NonZeroU8`
读到 `00`、`repr(u8)` 双变体 enum 读到 `03` 都是立即 UB。`needs_drop` / `size_of` 两道守卫
挡得住「带析构」和「长度写错」，恰恰挡不住「bit pattern 受限但没有析构」——而这类类型
（`NonZeroU8`、C 类枚举、`repr(u8)` 状态机）正是最容易被顺手标成 POD 的。这曾是待办清单上
唯一一处公开面 soundness 缺口（当时记在 §20.3，现已清空），本轮收回内部后已关闭
（本节结论）。

**为什么不能直接封死那个常量**（三条都试过）：

- `pub unsafe trait Pod` + `impl<T: Pod> Encode for Vec<T>` 与 `impl<T: Encode> Encode for
  Vec<T>` 重叠（E0119，stable 无 specialization）——§6 第 2 条已删过这个空壳。`unsafe trait`
  表达的是「实现者的承诺」，但要用它给容器分流就必须特化，而特化不可用；
- sealed 常量要把 `Layout` 整体 sealed，连带封死用户自己的 `impl Layout for MyType`，也就是
  这个库的用法本身；
- 用关联类型默认值把「能不能选 POD」藏进不可命名的私有类型，但 associated type defaults
  至今不稳定。

所以只要「容器在同一个泛型 impl 里按类型查一个常量」，那个常量就一定在公开 trait 上，
也就一定能被外部安全地写错。

**做法：把开关换成钩子**。

- `Layout` 删掉 `const POD`（公开面 −1，「POD」这个概念从 API 上消失）；
- `Encode` 增 `#[doc(hidden)] fn encode_slice(items: &[Self], e) -> Result<()>`，
  `Decode` 增 `#[doc(hidden)] fn decode_slice(d, count) -> Result<Vec<Self>>`，
  **默认实现就是逐元素循环**；
- `Vec<T>` / `[T; N]` / `VecDeque<T>` 不再判断任何标志，直接 `T::encode_slice(...)` /
  `T::decode_slice(...)`；
- 整块语义只出现在 `primitives.rs` 的定宽标量里：
  `if size_of::<Self>() == 1 || cfg!(target_endian = "little") { unsafe { pod::write_bulk(..) } }
  else { 逐元素 }`；`unsafe` 全部收进私有 `src/pod.rs` 的两个函数（`write_bulk` / `read_bulk`），
  一处一份 `# Safety`；
- `bool` / `char` / `Box` / `Rc` / `Arc` / derive 结构体不覆盖 → 永远逐元素。

**为什么这就关掉了缺口**：外部类型**没有安全途径**打开整块路径。要打开只能自己写
`impl Encode for MyType { fn encode_slice(..) { unsafe { .. } } }` —— UB 就落在**它自己的
`unsafe` 块**里，属于「unsafe 作者的责任」，不再是「安全代码触发 UB」。这正是用户要的
「外部类型一律视为非 POD」。顺带把 §19.10 的两道守卫变简单了：`needs_drop` / `size_of` 不再
需要在容器里对任意 `T` 检查，而是退化成钩子实现点上的 `debug_assert` —— 判断就发生在
看得见类型不变量（「它是 `NonZeroU8`」）的那一行。

**代价**。公开面 −1（`POD` 不可读，也就不可被外部误设，这正是目的）；`#[doc(hidden)]` 方法
技术上可以被外部覆盖（sealed 不了，理由同上），但那需要外部自己写 `unsafe`；每个定宽标量多
两个方法体，容器侧反而更薄（少了分支与守卫）。

**性能**。同会话交替 A/B（`binary-bench`，默认 release；基线是改动前的同一棵树，两个二进制
各自 `cargo build --release`，四轮取最小）：

|行|改前|改后|比值|
|:---|---:|---:|---:|
|encode tuple (raw)|15.93|16.31|1.02|
|encode named (raw)|37.99|38.13|1.00|
|decode tuple (raw)|90.43|92.51|1.02|
|decode named (raw)|109.65|110.05|1.00|
|encode tuple (owned)|46.75|47.48|1.02|
|encode named (owned)|62.18|59.87|0.96|
|encode vec<i32> ×1024（bulk）|33.70|34.50|1.02|
|decode vec<i32> ×1024|41.25|41.82|1.01|
|decode vec<u8> ×1024|12.70|12.74|1.00|
|encode vec<bool> ×1024（逐元素）|2146.91|**957.58**|0.45|
|encode vec<loop> ×1024（逐元素）|80.26|79.53|0.99|

两件值得记的事：

1. **钩子本身必须 `#[inline]`**。第一版忘了标，两个助手各多一层不内联的调用，`decode vec<i32>`
   41.3 → 44.2（+7%）、`decode tuple (raw)` +5%；补上后回到 +1.4% / +2.3%（与同一二进制多跑
   几轮的噪声同量级）。和 §19.15.11 是同一条教训：**容器与元素之间的每一层都要能被内联掉**。
   同一轮里 `named encode` 出现过 +7.7%，补 `#[inline]` 后消失（1.004）——判为落位噪声，
   不是这次改动引入的。
2. **`vec<bool>` 2150 → 958（2.2×）**。旧循环写的是 `e.auto(item)`，即
   `Encoder::auto` → `Encode::encode_auto` 多一跳且没被内联；新的默认钩子直接
   `item.encode_auto(e)`。也就是说**外部类型的逐元素路径顺带快了一倍**（那条循环每元素一次
   调用 + 一次 `Result`）。`LoopI32` 行不动，因为那条循环本来就被 LLVM 向量化掉了。

**全矩阵**（`tuple-named-bench`，四配置三轮最小值；比值取「各语言最优构建」）：

|行|Rust|Rust LTO|C++|C++ LTO|最优/最优|
|:---|---:|---:|---:|---:|---:|
|encode tuple (raw)|16.4|24.1|39.9|14.7|1.11×|
|encode named (raw)|38.4|35.2|96.9|16.3|2.15×|
|encode tuple (owned)|48.8|35.9|60.4|41.6|0.86×|
|encode named (owned)|66.1|47.6|112.4|54.8|0.87×|
|decode tuple (raw)|93.7|82.2|65.1|56.9|1.45×|
|decode named (raw)|109.0|96.1|102.8|71.7|1.34×|
|encode i32 (raw)|0.44|0.64|3.45|0.43|1.02×|

与 §19.15.13 的记录（1.08× / 2.13× / 0.88× / 0.85×）逐项在噪声内，borrowed 与 owned 都
没有退化，decode 反而更好（1.45× / 1.34×，上一轮记的是 1.64×）。`binary-bench` 的
`encode vec<i32> x1024` / `encode vec<u8> x1024` 两行名从 `(auto, POD bulk)` 改为
`(auto, bulk)`；历史表里出现旧名字时按同一行读。

**验证**：四配置 wire hex 逐字节相同；Miri（host + 大端抽象机）与 s390x qemu 全绿，计数见
§20.1。端序那一条现在靠 `cfg!` 而不是常量，所以 §19.11 的 s390x 用例
（`test_wire_order_is_endian_independent`）删掉了对常量的断言，改为只断言 wire 字节与往返 ——
「大端下多字节标量回退逐元素」这件事从「可断言的事实」变成「实现细节」，由 Miri 的 BE 抽象机
与 qemu 实机覆盖。

### 19.17 `with` / `borrow`：成员级转换器与借用解码（2026-11-08）

待办清单上最后一项功能缺口（当时记在 §20.3，现已清空），本轮关闭。两项都**不动 wire**：`with` 让成员的 wire 形状与 Rust 类型解耦，
`borrow` 让解码结果指回输入缓冲。对端早有对应能力（C# `[Converter]` / `SpanLike` 一族，
C++ `FixedConverterPolicy` / `std::span`），Rust 缺的是 derive 的入口。

#### `with`：契约是「换一个 `Repr` 类型」，不是「换一对函数」

三个候选，只有第三个同时满足「零新增公开 API」与「没有第二份长度契约」：

|候选|形状|结论|
|:---|:---|:---|
|三函数|`encode` / `decode` / `len`|否。`write_prefixed_bounded` 的头宽由传入长度决定（§19.15.11），`len` 一旦是上界，同一个值就会写出比 `Encode` 路径更宽的头 —— 而 `with` 的全部意义就是与对端同字节|
|生成 wrapper 类型|derive 每成员吐一个 `struct __W<'a>(&'a T)` + `impl Encode`|否。零 API，但每成员约 10 行生成代码，且把 `Encode` 的实现塞进用户 crate 的命名空间|
|**`Repr` + `to` / `from`**|模块声明 `type Repr`、`fn to(&T) -> Repr`、`fn from(Repr) -> Result<T>`|采用|

`Repr` 方案把「wire 长什么样」完全交给 `Repr` 自己的 `Layout` / `Encode` / `Decode`：`LEN`
决定 tuple 里是 raw 还是带前缀、前缀宽度、嵌套规则，全部继承，没有第二处可写错。生成代码
只做类型替换：

|位置|编码|解码|
|:---|:---|:---|
|named 必需|`write_named_member(__e, name, &m::to(&self.f))`|`m::from(__m.value(slot, name)?)?`|
|named `optional`|同上（默认值仍按**字段类型**比较）|`__m.get(slot)`：`Some(p)` → `Decode::decode(Decoder::new(p))`，`None` → `Default`|
|tuple 非末位（auto）|`Encode::encode_auto(&m::to(&self.f), __e)`|`Decode::decode_auto(__d)`|
|tuple 末位（raw）|`Encode::encode(&m::to(&self.f))`|`Decode::decode`|
|tuple 的 `Layout::LEN`|`<m::Repr as Layout>::LEN` 参与求和|—|

公开 API 零新增：`write_named_member`、`Encoder::write_prefixed_bounded`、`Decoder::new`、
`Decoder::with_prefix` 都是现成的。字段类型不必实现 `Layout` —— 那正是孤儿规则下外部类型
（`SystemTime` / `Uuid` / `Decimal`）的常态，也是这条属性存在的唯一理由。

**代价**：`to()` 必然构造一个 `Repr` 值，`Repr = String` 这类转换会分配。记录在案，不做
零分配变体：真出现热点，再加「函数式转换器」作为第二个入口，属性语法不变。

**边界**：`with` 只作用于具名结构体的成员；`#[binary(tuple)]` 允许（它只换转换器、不换布局，
所以成员仍不许 `rename` / `optional` / `skip`）；union variant 不支持（variant 本来就不接受
成员属性）；`with` + `skip` 直接报错。

#### `borrow`：靠成员类型表达，不设 `#[binary(borrow)]`

`&'de str` / `&'de [u8]` 的 `Decode` 本来就是零拷贝，缺的是**声明它们的结构体**：derive 之前
见到任何 `<...>` 都拒绝，连 `struct Frame<'de>` 都写不出来。现在允许**恰好一个生命周期参数**
（无 type / const 参数、无 `where`），并把它绑到解码生命周期上：

```rust
impl<'de> Decode<'de> for Frame<'de>   // 借用的寿命 = 值的寿命
```

**为什么不加属性**：字段类型 `&'de str` 已经表达了「借用」，再加 `#[binary(borrow)]` 就是第二
个能和类型打架的开关。serde 需要它，是因为它把 `'de` 与结构体自己的 `'a` 分开、再补
`'de: 'a`；这里两者是同一个名字，没有可补的关系。所以 C#/C++ 的「默认借用」在 Rust 侧的
对应物是「把成员写成引用」，不是属性。

**`&'de [T]` 明确不做**：payload 起点由 1 或 4 字节前缀决定，无法保证等于 `align_of::<T>()`；
把 `&[u8]` 重解释成 `&[T]` 要么 UB，要么给出「取决于输入缓冲地址」的报错 —— 同一段字节在
两次运行里可能一次成功、一次失败，这不是可接受的解码语义。`&'de [u8]`（对齐 1）不受影响。
需要按元素宽度解释时，调用方拿到 `&'de [u8]` 后自己 `align_to`，判据和责任都在他手里。

**顺带修掉的生成 bug**：`optional` 的默认值比较原来写 `self.f != Default::default()`，对
`&[u8]` / `Vec<u8>` 是 E0283（`&[u8]: PartialEq<ByteStr>`、`Vec<u8>: PartialEq<ByteString>`
也存在），改成 `<Ty as Default>::default()`。这条一直潜伏到 `#[binary(optional)] body: &'de [u8]`
才被触发。

#### 验证

- 新增 `tests/borrow.rs`（8 例）：named / tuple / union 的借用成员、指针落在输入缓冲内
  （零拷贝的直接证据）、非法 UTF-8、截断载荷、非借用生命周期参数。
- `tests/derive.rs` +5 例：`with` 的 named（必需 / optional）、tuple（定长 `Repr` / 变长
  `Repr`）、转换器错误与 wire 错误分离。核心断言是 **wire 恒等**：`with` 成员与「同一字段写成
  `Repr` 类型」的镜像结构逐字节相同，`Layout::LEN` 也按 `Repr` 求和（16 / 0）。
- 4 条新 UI 快照（`with_skip`、`with_not_literal`、`two_lifetimes`、`where_clause`），两条
  泛型快照的消息改写为「至多一个生命周期参数」。共 27 条。
- 没有新增 `unsafe`：借用路径复用既有的 `take` / `read_prefix`，`with` 只调公开 API，因此
  Miri 与 s390x 不重跑（§19.16 已覆盖同一批站点）。
- 热路径零改动：不带 `with` 的成员，生成代码只有 `optional` 那一行从 `Default::default()`
  变成 `<Ty as Default>::default()`（同一份代码，补了个限定），其余逐字符不变。矩阵
  复测（四配置三轮最小，「各语言最优构建」）：tuple encode **1.12×**、named encode **2.13×**、
  tuple owned **0.86×**、named owned **0.88×**、decode tuple **1.47×**、decode named **1.36×**、
  i32 **1.00×** —— 与 §19.16 那一轮逐项在噪声内。矩阵只出时间，不出字节；「wire 没变」由
  `tests/wire.rs` 的 49 条字节断言 + 33 条 C# fixture 守住（本轮全绿）。

### 19.18 名字查找：derive 的 `match` 已是最快，不引入查找表（2026-11-09）

**结论**：`match` 在现实 schema 上就是最快的方案，比第二名快 1.3~1.8×，还省掉整张表。
derive 不改，也不做 `const fn` 预计算查找表。

这条回答的是 §5 留下的问题（C++ 的 packed 2×u64 要不要抄）。答案是否定的，而且原因不是
「Rust 的 `match` 也够快」——**C++ 侧 packed 之所以赢，是因为它的对数值是 `FindMember` 里
`std::string_view ==` 的线性循环（7.5 ns），不是 packed 本身快**：同一个 4 成员 schema，Rust 的
`match` 1.74 ns，packed 的移植版 3.19 ns，C++ 的 packed 2.2 ns。也就是说，把 C++ 那套搬进
Rust 会**变慢 1.8×**。

**装置**：`crates/binary-bench` 新增 `lookup` 组，单独跑，不进默认的五组：

```bash
cargo run --release -p binary-bench -- lookup        # 汇总表，seq + hit
cargo run --release -p binary-bench -- lookup full   # 完整矩阵，pipe 行
```

七个方案共用一个 `&[u8] -> Option<usize>` 形状（就是 derive 生成的 `find` 闭包形状）：
`match`（五个字面量 schema 手写，等于 derive 的输出）、`linear`（等长扫描，C++ `findmember`
的对位）、`longdata`（C# `GetLongData`：≤15 B 装 2×u64、≤11 条，超出回落到 `chain`）、
`chain`（`hash*33` 桶链）、`open`（FNV-1a 线性探测）、`perfect`（运行期搜种子）、
`sorted`（二分）。C++ 那套的 `unordered` / `simd` / `swiss` 没有搬：`unordered` 在 Rust 要
owned `String` 键（测到的是分配而不是查找），`simd` / `swiss` 在 C++ 侧本来就没赢过
`chain` / `open`。

case 集与 `binary-benchmarks/named-lookup` 用同一套生成器（同一 xorshift、同一 seed、同一 label），
另加四个字面量 schema（`real4` / `real8` / `real16` / `real32`）和一个等长压力用例
`flat16`（16 个 6 字节真字段名）——只有字面量才能让 `match` 存在，因为它的模式必须是常量。

测量规则是本仓库的（2000 次预热、7 轮、每轮翻倍到 50 ms、取最小值），构建是 fat LTO + CGU=1。
**与 C++ 装置有一处差异**：查询游标用比较回绕，不用 `i % len`，所以两边的绝对值不能相减，
只能在同一张表内部比较。

**codegen 事实**（`MatchReal4` 的反汇编）：先 `cmp` 长度 → 长度 2 时 `cmp byte [..], 'I'`；
长度 4 时按首字节分派（`'N'` → `Name`、`'L'` → `List`、`'I'` → `Item`）→ 剩余字节逐字节
`cmp`。没有 `bcmp` 调用、没有跳转表、没有表内存。§5 原先写的「先比长度、再内联 memcmp」
不准确，已在那一节订正。

主表（fat LTO + CGU=1，`seq` + `hit`，ns/lookup；`-` = 该 case 没有 `match`）：

|case|match|linear|longdata|chain|open|perfect|sorted|
|:---|---:|---:|---:|---:|---:|---:|---:|
|`real4`（`Id/Item/List/Name`）|**1.74**|5.04|3.19|5.20|5.56|8.69|9.75|
|`real8`|**2.62**|6.59|3.39|6.09|5.85|9.38|14.28|
|`real16`|**3.79**|10.58|5.97|5.93|6.11|9.85|19.84|
|`real32`|**3.99**|15.90|5.69|5.97|6.37|10.91|26.62|
|`flat16`（全 6 B 等长）|**3.49**|22.25|6.24|6.43|6.56|11.33|19.81|
|`n4`（随机 4 B）|-|6.45|**2.93**|5.42|6.07|9.34|9.55|
|`n12`|-|16.52|**5.08**|5.39|6.20|9.24|20.00|
|`n32`|-|47.25|**4.90**|5.09|5.99|9.52|26.17|
|`n64`|-|95.60|**5.17**|5.40|6.26|-|34.00|
|`len15`（4 成员）|-|6.44|**3.61**|8.22|10.66|22.93|9.53|
|`len16`|-|6.42|6.08|**6.35**|10.91|23.09|10.55|

上表是 LTO 口径。脚本入口（默认 `--release`）跑同一组 case 排序不变：`match` 是
1.91 / 3.72 / 3.65 / 4.04 / 3.39，除 `real8` 外与上表差 ≤ 10%；`real8` 的 3.72 对 2.62
是函数落位带来的（§19.15.5 同一类构建效应），不是算法差异，不改变结论。

读法：

- **`match` 在四个现实 schema 上全胜**，对手是 `longdata` / `chain`（1.3~1.8×）。表内存
  0 B 对 64~2048 B，所以这一列没有取舍，是纯赚。
- **`linear` 不能当作 `match` 的退化形态**：同样是 4 成员，`linear` 5.04 对 `match` 1.74。
  「成员一多 `match` 就会退化成线性扫描」这个担心在本轮范围内不成立——`real16` → `real32`
  是 3.79 → 3.99，`flat16`（长度分派失效的最坏形状）也只有 3.49。
- **等长名字不是问题**：`flat16` 全部 6 字节，长度 switch 只能给出一个分支，`match` 仍靠
  首字节＋尾部字节把成本压在 3.49 ns；反而 `longdata` 因为 16 > 11 回落到 `chain`（6.24）。
- **packed 的边界在 11 / 15**：`n11` → `n12` 是 3.91 → 5.08，`len15` → `len16` 是
  3.61 → 6.08（两条都是回落到 `chain`）。`match` 没有这两个边界，它不认识「装箱」。
- **`open` 怕长名字**：FNV-1a 逐字节，完整矩阵的 `len64` 上要 49 ns（`chain` 每 4 字节一轮，
  12 ns）；`perfect` 同样怕长名字，而且 `n64` 起搜不到种子（C++ 侧同样 `na`）。
- **`miss` 与 `rand` 免费**：完整矩阵里把一条查询换成未知名、或把查询序打乱，所有方案的变化
  ≤ 2%（默认 release 的 `lookup full` 全矩阵）——前向兼容的跳过路径没有额外成本。

**端到端上限**：同会话 `objects` 组 `named parse only (4 names)` = **11.30 ns**、
`decode named (raw)` = **102.60 ns**。查找的 1.74 ns 占 parse 的 15%、占完整解码的 **1.7%**。
即使把查找做成免费，端到端也只快 1.7%——这是本轮不动 derive 的定量依据，也是「先基准后修改」
该有的收尾：结论是**不修改**，并且这个「不修改」有数。

**未覆盖**：成员数 > 16 且名字全部等长的形状（再往上要手写 32 条字面量才测得到，而
`real16` → `real32` 与 `flat16` 都没有拐点，优先级不高）；`simd` / `swiss` / `unordered`
三个方案（C++ 侧已证明不占优）；`const fn` 建表的版本——本轮结论不需要它，但若将来出现
「没有 `match` 可用」的路径（例如运行期 schema / 动态名字），这几条要重测。

复现（仓库根执行；第二条是 §19.15 的那个 LTO 口径）：

```bash
cargo run --release -p binary-bench -- lookup
CARGO_PROFILE_RELEASE_LTO=fat CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
  CARGO_TARGET_DIR=target-lto cargo run --release -p binary-bench -- lookup
```

---

## 20. 当前状态与质量门（2026-11-09）

本节是全文档唯一**活的状态表**。§9 / §13 / §19.x 里的日期、计数与「尚未」字样都是当时的
记录；与本节冲突时一律以本节为准。

### 20.1 验证矩阵

|门|命令|结果|
|:---|:---|:---|
|全量测试（host）|`cargo test --all`|**157 passed / 0 failed**（lib 单元 1 + `tests/length.rs` 11 + 集成 141 + doc 4）+ 27 条 compile-fail 快照|
|`no_std + alloc`|`cargo test -p binary --no-default-features`|1 + 11 + doc(4) 通过；六个 `alloc` 容器无条件编译|
|格式 / lint / 文档|`cargo fmt --all -- --check`、`cargo clippy --all --all-targets -- -D warnings`（含 no-default）、`cargo doc -p binary --no-deps`|全绿、零警告|
|大端真实二进制|`./scripts/test-multiarch.sh s390x`|157 passed / 0 failed（153 + doc 4）+ ui(27)，`file` 确认真 `ELF 64-bit MSB`|
|四象限交叉|`./scripts/test-multiarch.sh --all`|7 个 target 各 157 passed / 0 failed，`file` 逐 target 核对 ELF 位宽与端序|
|shell 脚本|`./scripts/check-shell.sh`|`scripts/` 下 4 个 shell 脚本：`bash -n` / `shfmt -d` / `shellcheck -S warning` 零发现|
|UB 检查|`./scripts/check-miri.sh`（host + s390x 抽象机）|各 156 passed / 0 failed（152 + doc 4）；ui 按 `#[cfg(not(miri))]` 跳过；未检出 UB|
|依赖边界|`cargo tree -e normal -p binary`|只有 `binary → binary-derive`；`trybuild` 仅 dev-dependency|
|性能剖析（仅 C++ / Rust）|`./scripts/run-benchmark.sh profile` + C++ 侧同测量规则的 harness|两边原生 `u8` 字符串、hex 逐字节相同；数据与热点见 §19.15|

四象限由 `scripts/test-multiarch.sh --all` 一次跑完（本机 35 s，构建已缓存）：每个 target
只在自己的 sysroot 下链接、自己的 qemu-user 里执行，`binfmt_misc` 不参与；脚本再把
`file` 读回的 ELF 位宽 / 端序与声明比对，标错 target 不会静默通过。

| target  | Rust triple                     | bits | endian | tests |
| :------ | :------------------------------ | ---: | :----- | ----: |
| x86_64  | `x86_64-unknown-linux-gnu`      | 64   | little | 157   |
| i686    | `i686-unknown-linux-gnu`        | 32   | little | 157   |
| aarch64 | `aarch64-unknown-linux-gnu`     | 64   | little | 157   |
| armv7   | `armv7-unknown-linux-gnueabihf` | 32   | little | 157   |
| riscv64 | `riscv64gc-unknown-linux-gnu`   | 64   | little | 157   |
| powerpc | `powerpc-unknown-linux-gnu`     | 32   | big    | 157   |
| s390x   | `s390x-unknown-linux-gnu`       | 64   | big    | 157   |

qemu 跑真实 MSB ELF + libc 验**行为与端序**，Miri 解释 MIR 验 `unsafe` 有没有 **UB**，两者
互补但都不是 soundness 证明（§19.11 / §19.12）。`--no-default-features` 覆盖 `alloc` 层，
`std` feature 只 gate `HashMap` / `HashSet`；实际只有 `std` / `derive` 两个 feature（§17.5）。

§19.15.9 / §19.15.10 / §19.15.11 三轮只动了安全的编码热路径（一次保留 + 就地写、去掉值前缀
的子 `Encoder`、已知长度前缀改走一次性预留），没有新增 `unsafe`，当时未重跑 Miri / s390x。
§19.16 把 `unsafe` 重新搬进 `src/pod.rs`（两处 memcpy 站点，端序从常量改 `cfg!`），因此
Miri（host + BE 抽象机）与 `scripts/test-multiarch.sh s390x` 都已重跑并全绿。
§12.5 补 C++ 语料那一轮只加了 fixture 数据与断言（无 `unsafe`、无库代码改动），
三个检查仍整轮重跑，本表数字是那一轮的（+2 条 `cxx_fixtures`，ui 快照 +4 是 §19.17 留下的）。
之后 `binary-cxx` 修容器头宽、重生成 `cxx.tsv`（桶列删除，仍 22 条、仍 2 例断言），
仓库内只动了 tsv 与断言代码，用例数不变，故上表数字沿用。
§19.15.17 只改了 `Encoder::write` 的 owned 分支与 `write_prefixed_bytes_wide`（安全代码，
未新增 `unsafe`），wire 不变；测试、`--no-default-features`、clippy（含 no-default）、doc、
fmt 整轮重跑全绿。Miri（host + BE 抽象机）与 `scripts/test-multiarch.sh s390x` 也整轮重跑：
新增的 `test_bulk_write_backend_parity` 在三个环境里都过，未检出 UB。
§19.15.18 只加了测试（`tests/buffer_contents.rs` 一条，集成 137 → 138）和 benchmark harness
的一行 `black_box` 位置，库代码与 wire 都没动；上表随之从 148 补到 150（149 是上一轮
`test_bulk_write_backend_parity`，150 是本条）。Miri（host + BE）与 s390x 整轮重跑同样全绿。
交叉矩阵那一轮只动装置与脚本：新增 `scripts/test-multiarch.sh`、`scripts/check-shell.sh`
与 `.editorconfig`，独立的 `scripts/s390x-check.sh` 删除（`s390x` 只是矩阵里的一个目标，
`scripts/check-miri.sh` 也从 `miri-check.sh` 改名对齐 `check-*`）；库侧唯一改动是
`crates/binary-bench/src/harness.rs` 的分配计数器 `AtomicU64` → `AtomicUsize`（32 位
powerpc 没有 `AtomicU64`，而计数器每次测量前清零，不会溢出）。用例数、wire 都没变。

仓库状态：`binary-rust` 仍是零 commit（`git status` 全为 `??`），首次提交前需要一并审阅
`scripts/`、新增源码与测试 —— 这是发布动作，不是技术欠账，所以不占 §20.3。

### 20.2 已定型、不再重复讨论的决策

- **不做 wire 版本号，也不做版本协商**（§1）；兼容性由 `#[non_exhaustive]` 买单（§18.8.5）。
- **闭合 `Error` 枚举**：整体擦除成 `dyn Error` 已由 benchmark 否决（§18.8.7 / §19.3）。
- **benchmark 只报 raw 形式**（§19.7 / §19.15.2，用户裁定 2026-11-08）：C# / C++ 都没有
  auto 这个被测物，Rust 再报一条只是同一份代码多一次分支或多一个前缀，差值落在噪声里；
  auto 与 raw 的字节关系由 `tests/wire.rs` / `tests/named.rs` 钉住。
- **整块读写能力不再是公开开关，而是元素的内部钩子**（§19.16，用户裁定 2026-11-08）：
  `Layout::POD` 已删除，`Encode::encode_slice` / `Decode::decode_slice` 默认逐元素、只有
  本 crate 的定宽标量覆盖；外部类型因此永远不会被整块处理，想打开只能自己写 `unsafe`。
  `bool` / `char` 仍逐元素，密集布尔集合走独立 bitset 编码（§19.10 方案 A 的结论不变）。
- **容器 raw payload 不写元素个数**，map 条目是 `(K, V)` tuple flatten（§17.3 订正 / §19.14）。
- **`Box` / `Rc` / `Arc` wire 透明**，解码不保留共享身份；递归深度不受限制（§19.13）。
- 八个标准容器**零新增依赖**落地，`Cargo.toml` / `Cargo.lock` 未改（§19.14）。
- **≤16 字节压缩用定长 `u64` 搬移**，照抄 C#/C++ `Allocator::FinishAnchor`：带
  `(-length) & 7` 字节尾部余量判断，不用通用 `memmove`，也不采用无 slack 的阶梯搬移
  （§19.15.7，用户裁定 2026-11-08）。理由是 wire 逐字节一致、延迟分布集中，且 100 串
  混合长度实测下均值也更低；「`int64` 大多数情况更优」是已被推翻的旧说法。
- **一个 `Encoder`，一个后端**：`write_prefixed_bounded` 不建子 `Encoder`、不把上界当预留；
  超界在事后按 `actual` 判、错误即终止（§19.15.10，用户裁定 2026-11-08）。子 `Encoder`
  只在「拿 slice 长度当围栏」时看起来有用，代价是第二个后端判别、把估算变成预留、以及一个
  只为这层存在的 `BufferFull`；实测 borrowed 打平、owned 慢 4~6%，已按此取舍落地。
- **已知精确长度就一次预留**：`String` / `str` / `[u8]` 及其引用走 `write_prefixed_bytes`，
  不再用 `write_prefixed_bounded(len, …)`「借」一次预留（§19.15.11，2026-11-08）。后者多一次
  容量检查、一次 `as_mut_slice`、一次 `pos` 更新，6 字节字符串上贵 2.8 ns；tuple 有 4 个
  变长成员，这一项就是 tuple encode 对 C++ 的 1.75× 里的绝大部分。`Vec<T>` 例外（POD 容器
  两者等价），`write_prefixed` 的兜底路径也不动。
- **跨语言 benchmark 在字节层做，`string` 不参与**（§19.15.1，2026-11-08 用户裁定）：
  C# 用 `byte[]`、C++ `std::string` 当 bytes、Rust `&[u8]` / `Vec<u8>`，三边都不转码、不校验。
  三种语言的内存表示不同（UTF-16 / 8 位序列 / UTF-8），同一行被测的东西不一样。不做「三边
  都套 UTF-16 适配器」：那层转码不在任何一边的库热路径上，且 Rust 库外拿不到一次预留的入口
  （`write_prefixed_bytes` 是 `pub(crate)`），适配器会被 `write_prefixed_bounded` 的 3.6 ns
  入口税吃掉，偏差比现在更大。Rust ↔ C++ 双边行仍用 `String` / `std::string`。覆盖到哪见 §19.6。
- **跨语言比值取「各语言最优构建」**：Rust 默认 `--release` 对 C++ `-O3 -flto`（§19.15.11）。
  同配置 LTO/LTO 在这对语言之间测的是构建系统 —— C++ 不开 LTO 会留下跨 TU 的
  `Allocator::Append`（14.5 → 39.2），Rust 开 LTO 反而被函数落位拖慢（16.1 → 23.0）。
- **名字查找就是 derive 生成的 `match`，不引入查找表**（§19.18，2026-11-09）：现实 schema 上
  `match` 比 packed 2×u64 / `chain` / `open` 都快 1.3~1.8×，表内存 0 字节；查找只占
  `decode named` 的 1.7%，即使做成免费也不值得为它加代码。C++ 的 `longdata` 看着快，是因为
  它对的位是 `std::string_view ==` 的线性循环，不是 packed 本身。
- **named 成员顺序不是 wire 契约**：Rust / C++ 都按声明序写，C# 按名字序写只是为了 encode
  输出稳定、测试好对（两边 README 原话都是 "the format does not require any order: decoding
  looks members up by name"）。不做 derive 期按名字排序；跨语言对照 named 只比「解出同一组
  值」，不比字节。`tuple` / tuple object 的顺序仍是契约（§19.4，用户裁定 2026-11-08）。
- **`Error` 保持 24 字节，不为缩 `Result` 去装箱 payload**（§19.15.12，2026-11-08）。24 来自
  「16 字节 payload + 判别字节按 8 对齐」；把 `Cow` 装箱后实测仍是 24，要再降只能连
  `&'static str` 与 `Arc<dyn Error>` 一起装箱。成功路径只写判别字节，代价只在不被内联的边界上
  （≈0.2 ns/call），而热路径全内联，测不到这一项。
- **owned 后端拿 `Vec::len` 当水位线，水位线只在构造期提交**（§19.15.13，2026-11-08）：
  `with_capacity(n)` 先 `resize(min(n, 128), 0)`，`ensure_capacity` / `ensure_prefix_tail`
  保持原样。理由不是审美 —— 把增长逻辑放进 `ensure_capacity` 会让 borrowed 的
  `vec<i32> x1024` 从 80 ns 变成 788~2219 ns（循环向量化被打掉），那是 10~28× 的代价，
  换 3 ns 的 owned 收益不值得。wire 不受影响：`finish` / `as_bytes` 都按 `pos` 截断。
  「allocator 一个字节都不写」的 append-only 变体已整份实现并 A/B（+7.9% ~ +60%，§19.15.18），
  同样否决 —— 水位线保留，但只作为 Rust 的 `Vec` 记账，不是任何转换器的依赖。
- **`with` 用 `Repr` 类型作契约，`borrow` 不设属性**（§19.17，2026-11-08）。`#[binary(with =
  "m")]` 只做类型替换（`m::Repr` + `to` / `from`），wire 语义从 `Repr` 的 `Encode` / `Decode`
  继承，因此没有第二份长度契约可写错，也不需要新的公开 API；`&'de str` / `&'de [u8]` 的借用
  由**成员类型**表达，derive 只放开「恰好一个生命周期参数」。`&'de [T]` 拒绝：payload 起点由
  1/4 字节前缀决定，无法保证 `align_of::<T>()`，重解释只能 UB 或给出「取决于缓冲地址」的
  解码结果。

- **`repository` 指向 `afxres/binary-rust`，只有可发布的 crate 继承**（2026-11-08）：
  `[workspace.package].repository` 由 `binary` / `binary-derive` 用
  `repository.workspace = true` 消费；`binary-bench` 与 `binary-tests` 都是
  `publish = false`（后者本轮补齐），不继承。此前声明了却无人消费，nightly cargo 的
  `cargo::unused_workspace_package_fields`（默认 warn）把它报成死配置 —— 不被继承的键不会
  出现在任何发布的 manifest 里，留着只会让人以为已经配好。stable 不报这条，所以它从来不影响
  质量门，改的只是发布出去的元数据。
- **缓冲容量由调用方在构造期给上界，库不做增长启发式**（2026-11-08，用户裁定）：编码时
  payload 已在内存里、长度可预知，`Encoder::with_capacity` / `on_slice`（C++ / C# 的
  `Allocator` 构造重载同形）是唯一入口，写入路径没有 reserve。估算必须是上界 —— 少算
  1 字节反而比不估更慢（§19.15.15）；超出仍按需增长，只是要付一次 realloc。解码侧没有
  这个信息，所以 §19.15.14 的「扫元素个数预分配」按不做关闭。
- **owned 后端的大 payload 只写一遍**（§19.15.17，2026-11-08）：`write` 在
  `src.len() >= 256` 时把 payload 追加进 `Vec` 的未初始化余量（`extend_from_slice`），不再
  `resize` 清零后覆盖；`write_prefixed_bytes_wide` 拆成「4 字节头 + 批量 payload」。水位线、
  `finish` 截断与 wire 都不变，只是把 C++ / C# 一直就有的 `bounds` / `offset` 分离补上。
- **解码出拥有型 `String` 的分配与 UTF-8 校验是用法取舍，不是库的设计缺口**
  （2026-11-08，用户裁定）：`decode tuple` 的 94.1 ns 里 60 ns 是 4 次 `String` 分配加 4 次
  校验（§19.15.16），但库已经提供了 `&'de str`（零拷贝、仍校验）与 `&'de [u8]`（连校验也省）
  这两条入口，成员类型换成它们即可。解码器不替调用方决定「解成拥有型还是借用」，也不在
  `Vec::decode` 里猜元素个数（§19.15.14）。要动那条 benchmark 数字只能换被测类型，属
  §19.15.1 的口径问题，不是待办。

### 20.3 剩余（按优先级）

无。§19.15 的每条性能线索都已结案（含 §19.15.18 否决的 append-only allocator、§19.15.14
关闭的变长预分配），§20.2 的每条是定型决策而不是欠账；benchmark 口径问题记在 §19.6 /
§19.15.1，仓库动作（零 commit、首次提交前审阅 `scripts/`）记在 §20.1 末尾。新增待办时按
「库侧 / 口径 / 仓库」三类写进来，并在条目里给出定量依据。
