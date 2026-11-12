---
title: 键盘
description: keyboard-inset-height 环境变量、Keyboard.dismiss 和键盘高度：让内容保持在软键盘之上，并随键盘移动。
order: 120
group: primitives
demo: el-keyboard
---

# 键盘

`env(keyboard-inset-height)` 是软键盘覆盖在界面上的高度，键盘隐藏时为 0。把它用作视图的底部内边距或外边距，可以让内容保持在键盘之上：键盘显示或隐藏时，视图按键盘高度让出空间，变化在原生侧随键盘一起动画。它用于滚动视图之外的内容，例如聊天界面底部的消息栏；[滚动视图](./scroll.md)或 [List](./list.md) 中的输入框会在原生侧滚动到可见位置。与它配套的还有隐藏键盘的 `Keyboard.dismiss()`，以及用于自定义布局的键盘高度。

```rust tabs
use nsh_core::prelude::*;

fn chat() -> impl IntoElement {
    let draft = signal(String::new());
    view()
        .flex_1()
        .bg(white())
        .pb_keyboard()
        .child(scroll().flex_1().child(text("Messages…").p_4()))
        .child(
            view()
                .flex_row()
                .gap_2()
                .p_2()
                .border_t_1()
                .border_color(rgb(0xe5e5e5))
                .child(
                    input()
                        .value(draft)
                        .on_change(move |v: &str| draft.set(v.to_owned()))
                        .placeholder("Message")
                        .flex_1()
                        .p_2(),
                ),
        )
}
```

```tsx tabs
import { mount, Scroll, signal, Text, View } from "nsh";
import { Input } from "nsh/base";

function App() {
  const draft = signal("");
  return (
    <View style={{ flex: 1, backgroundColor: "#ffffff", paddingBottom: "env(keyboard-inset-height)" }}>
      <Scroll style={{ flex: 1 }} keyboardDismissMode="interactive">
        <Text style={{ padding: 16, paddingTop: 48 }}>Messages…</Text>
      </Scroll>
      <View style={{ flexDirection: "row", gap: 8, padding: 8, borderTopWidth: 1, borderTopColor: "#e5e5e5" }}>
        <Input
          value={draft()}
          onChange={({ value }) => draft.set(value)}
          placeholder="Message"
          style={{ flex: 1, fontSize: 16, padding: 8 }}
        />
      </View>
    </View>
  );
}

mount(App);
```

完整的键盘高度适用于延伸到屏幕底部的视图，界面的内容通常如此。如果它下方还有东西，例如标签栏，就用一个长度减去它：`calc(env(keyboard-inset-height) + -49px)` 让视图保持在 49 pt 的标签栏之上，`calc(env(keyboard-inset-height) + 12px)` 在键盘上方增加间距。内边距至少为 0。

## 内边距或外边距

| 样式 | 让出空间的方式 |
|---|---|
| `paddingBottom: "env(keyboard-inset-height)"`（Tailwind `pb-[env(keyboard-inset-height)]`，Rust `.pb_keyboard()`） | 上方的内容缩小；背景仍延伸到屏幕底部。 |
| `marginBottom: "env(keyboard-inset-height)"`（Tailwind `mb-[env(keyboard-inset-height)]`，Rust `.mb_keyboard()`） | 整个视图（包括背景）在键盘顶部结束。 |

键盘高度只与同一声明中的长度相加，因此视图用 `calc()` 把它和自己的间距组合起来。在 Rust 中，`.pb_keyboard_plus(px(..))` 和 `.mb_keyboard_plus(px(..))` 加上一个长度。

```rust tabs
use nsh_core::prelude::*;

fn sign_in() -> impl IntoElement {
    view()
        .mb_keyboard_plus(px(-49.0))
        .flex_1()
        .p_4()
        .gap_3()
        .child(input().placeholder("E-mail").keyboard_type(KeyboardType::Email))
}
```

```tsx tabs
<View class="flex-1 gap-3 p-4 mb-[calc(env(keyboard-inset-height)+-49px)]">
  <Input placeholder="E-mail" keyboardType="email-address" class="rounded-lg border border-neutral-300 px-3 py-2" />
</View>
```

这里视图下方 49 pt 的标签栏已经遮住了一部分键盘高度。要关闭让位，就在样式中去掉这个变量，例如根据一个信号加条件。

## 隐藏键盘

`nsh` 的 `Keyboard.dismiss()` 让获得焦点的输入框失去焦点，从而隐藏键盘。没有输入框获得焦点时，它不做任何改变。Rust 组件调用 `dismiss_keyboard()`。

```rust tabs
use nsh_base::button;
use nsh_core::prelude::*;

fn done() -> impl IntoElement {
    button()
        .on_press(|| dismiss_keyboard())
        .rounded_lg()
        .bg(rgb(0x2563eb))
        .px_4()
        .py_3()
        .child(text("Done").text_color(white()))
}
```

```tsx tabs
import { Text, Keyboard } from "nsh";
import { Button } from "nsh/base";

function Done() {
  return (
    <Button onPress={() => Keyboard.dismiss()} class="rounded-lg bg-blue-600 px-4 py-3 active:bg-blue-700">
      <Text class="text-center font-semibold text-white">Done</Text>
    </Button>
  );
}
```

> [!TIP]
> 要在用户拖动滚动视图时隐藏键盘，请在[滚动视图](./scroll.md)或 [List](./list.md) 上设置 `keyboardDismissMode="on-drag"` 或 `"interactive"`。

## 键盘高度

| TSX | Rust | 返回 |
|---|---|---|
| `keyboardFrame()` | `keyboard_height()`、`keyboard_duration()` | 软键盘覆盖在 NSH 界面上的高度，单位为点（隐藏时为 0），以及其显示或隐藏动画的时长，单位为毫秒。 |

两者都是响应式的：读取它们的 effect、JSX 表达式或样式闭包会在键盘显示、隐藏或改变高度时重新运行。

```rust tabs
use nsh_core::prelude::*;

fn message_bar() -> impl IntoElement {
    view()
        .flex_row()
        .p_2()
        .pb(move || {
            // The home indicator's inset only matters while the keyboard is hidden.
            let home = if keyboard_height() > Points::ZERO {
                Points::ZERO
            } else {
                safe_area().bottom
            };
            px(8.0) + home
        })
        .child(input().placeholder("Message").flex_1())
}
```

```tsx tabs
import { View, keyboardFrame, safeArea } from "nsh";
import { Input } from "nsh/base";

function MessageBar() {
  const insets = safeArea();
  return (
    <View
      style={{
        flexDirection: "row",
        padding: 8,
        // The home indicator's inset only matters while the keyboard is hidden.
        paddingBottom: 8 + (keyboardFrame().height > 0 ? 0 : insets().bottom),
      }}
    >
      <Input placeholder="Message" style={{ flex: 1 }} />
    </View>
  );
}
```

高度是键盘与 NSH 界面重叠的部分。当窗口本身为键盘缩小时（会调整大小的 Android activity），或 NSH 嵌入在结束于键盘上方的视图中时，高度为 0。

> [!NOTE]
> 要让内容保持在键盘之上，`env(keyboard-inset-height)` 是更简单的方式：它在原生侧随键盘动画，不需要运行代码。

## 自定义视图的硬件按键

获得焦点的视图会收到其平台报告的真实硬件按键转换：TSX 中的 `onKeyDown` / `onKeyUp`，或 Rust 中的 `on_key_down` / `on_key_up`。
这适用于自行绘制的画布或嵌入式交互界面。命名快捷键使用 `on_key` / `on_key_with`，文本编辑使用 `Input`，终端会话使用 `Terminal`。

`KeyEvent` 只报告输入源实际提供的数据：

| 字段 | 含义 |
|---|---|
| `key` | 输入源的逻辑键：命名键用 DOM 的拼写（`"Enter"`、`"ArrowLeft"`），可打印键用其实际报告的字符，保留大小写与当前布局，不会强行折算到另一种布局。输入源无法命名时为 `"Unidentified"`。 |
| `text` | 按住修饰键时该键会产生的字符预览（`"A"`、`"?"`），绝不是输入法提交的文本；没有时为 `null`。 |
| `physical` | 输入源报告了物理身份时的键身份：原生数字键码或 DOM 的 `code`；否则为 `null`。 |
| `modifiers` | 本次转换之后仍按住的修饰键。 |
| `repeat` | 按住键产生的自动重复。 |
| `captured` | 本次转换匹配已挂载的捕获规则，平台会消费它。 |

两个监听函数都不返回值。聚焦视图上的一个平台事件会沿同一原生窗口内的已启用监听祖先各传播一次，`onKeyDown` 与 `onKeyUp` 都是如此。
输入源报告的真实释放会到达聚焦视图，即使最初的按下发生在别处（与 DOM 一致）；窗口失去焦点会清除按住状态，不会伪造释放事件。

匹配使用规范的快捷键按键序列：原生键码或 DOM 的 `code` 会把键放回 US 布局；只报告字符的输入源会把 Shift 产生的标点
还原为 US 布局上的键并带上 Shift（`?` 即 `shift+/`），这仅用于匹配；大写字母仍是其小写键，修饰键以输入源为准（Caps Lock 不是 Shift）。
事件中的 `key` 与 `text` 仍保持输入源报告的内容。

TSX 的 `captureKeys`（`{"none"}`、`{"all"}`，或 `{["mod+k", "escape"]}` 这样的数组），以及 Rust 的
`capture_keys(KeyCapture::All)` / `capture_keys(KeyCapture::keys(...))`，会在输入到达前挂载消费规则。规则只解析一次，成为规范的快捷键
按键序列（即 `nsh/keymap` 的语法；`mod` 在 Apple 平台上是 Command，其他平台是 Control），`KeyEvent.captured` 报告本次转换的决定。
规则支持响应式值；保留视图再次渲染时省略规则，会恢复 `"none"`。可用的应用快捷键保持优先级，获得焦点的原生输入框保留自己的按键，
捕获也无法越过操作系统根本不投递的快捷键。被捕获的 Tab 留在当前视图；仅被观察的 Tab 按正常规则切换焦点。

```tsx
import { signal, Text, View } from "nsh";

function Pad() {
  const log = signal("");
  return (
    <View
      focusable
      captureKeys={["mod+k", "shift+tab"]}
      onKeyDown={(e) => log.set(`${e.key} ${e.captured} ${e.repeat}`)}
      onKeyUp={(e) => log.set(`up ${e.key}`)}
    >
      <Text>{log()}</Text>
    </View>
  );
}
```

Linux 桌面模式的无界面驱动也会执行这一 Rust API；Rust UI 集可以向 TSX 暴露应用专用组件及事件。
不同输入源的能力不同：只报告击键的旧式终端不会产生释放、重复或物理键码，GPUI 的逻辑事件也没有物理键码。
各平台输入源报告的字段见 `docs/dev/tooling.md` 及各平台的开发文档。

## 平台说明

- **iOS**：让位以键盘自身的时长和 ease-in-ease-out 曲线动画。
- **Android**：窗口为键盘缩小时（会调整大小的 activity），内容已经在键盘上方结束，变量为 0。键盘与视图重叠时，与 iOS 一样让出空间。
- **桌面平台和 Mac Catalyst** 使用硬件键盘输入：变量为 0，视图保持原有布局。
- **Web**：变量是浏览器自己的（VirtualKeyboard API 的）。

焦点顺序、回车键和输入附件参见[表单与键盘](../docs/forms-and-keyboard.md)，其他环境变量参见[安全区](./safe-area.md)。
