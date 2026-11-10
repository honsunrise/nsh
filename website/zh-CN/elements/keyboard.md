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

## 聚焦视图上的硬件按键

获得键盘焦点的视图（[`focusable`](./index.md#键盘焦点)，接收按压的视图默认为 `true`）在聚焦期间接收窗口的按键：`.on_key_down(..)` 带一个 `KeyEvent`，以及 `.on_key_up(..)`。按键按下会通知获得焦点的视图和每个监听的祖先；处理函数不返回值，一个视图做什么，不会让其他视图收不到这次按下。

窗口保持激活时，按键释放通知会回到最初接收按下的视图。窗口失去激活会清除已按下键的跟踪；维护自身按键状态的应用也可以在窗口焦点变化时清理这些状态。

`.capture_keys(..)` 让一次按下不再产生文字、也不再触发平台默认行为：`KeyCapture::All` 接收所有按键，`KeyCapture::keys([..])` 接收列出的 `keymap` 组合键。获得焦点的视图和它启用的祖先都可以捕获，以挂载的值为准，因此被移除或停用的视图不再捕获按键。`KeyCapture::All` 包含 Tab，列出 Tab 会取走焦点遍历；只监听而不捕获的视图，把 Tab、Return、空格和滚动留给平台。被捕获的按下以 `captured` 为 true 到达通知，并且不产生文字。获得焦点的 `Input` 保留原生输入，它外层的捕获不会取走它的按键。

应用的 `keymap` 在视图之前：可用的快捷键先于捕获触发，`passThrough: true` 的快捷键报告按键后继续传递，视图和平台仍会收到它。快捷键之后是获得焦点的视图和它祖先的通知及其捕获规则，然后是文字和平台的默认行为。获得焦点且可按压的视图保留 Return 和空格用于激活；要取走它们，捕获中要列出这些键。

输入法正在组合时，未被捕获的按键会先交给输入法，再执行普通激活，因此 Return 可以提交组合文字。只监听已提交文字的视图也遵循这一规则。

同一个视图用 `.on_text_input(..)` 接收文字，用 `.on_composition_change(..)` 接收输入法的预编辑，用 `.ime_cursor_area(Bounds<Points>)` 放置候选窗口（见[视图事件](./index.md#按键滚轮和文本输入)）。

```rust
// 一块在聚焦时接收所有按键的画布。
view()
    .focusable(true)
    .capture_keys(KeyCapture::All)
    .on_key_down(move |e: &KeyEvent| {
        if e.key == Key::Named(NamedKey::Escape) {
            close();
        }
    })
```

> [!NOTE]
> 这些事件属于 Rust 视图。Linux 窗口以及以 Linux 桌面运行的无头测试驱动会投递它们；`Input` 保留自己的输入，终端习惯保留自己的按键处理。

## 平台说明

- **iOS**：让位以键盘自身的时长和 ease-in-ease-out 曲线动画。
- **Android**：窗口为键盘缩小时（会调整大小的 activity），内容已经在键盘上方结束，变量为 0。键盘与视图重叠时，与 iOS 一样让出空间。
- **桌面平台和 Mac Catalyst** 使用硬件键盘输入：变量为 0，视图保持原有布局。
- **Web**：变量是浏览器自己的（VirtualKeyboard API 的）。

焦点顺序、回车键和输入附件参见[表单与键盘](../docs/forms-and-keyboard.md)，其他环境变量参见[安全区](./safe-area.md)。
