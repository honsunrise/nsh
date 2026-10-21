import { signal, Text, untracked, View } from "nsh";
import { Button, DockArea, type DockAreaLayout, type DockAreaRef } from "nsh/base";
import { platform, windowSize } from "nsh/device";
import { hint } from "../ui";

// `nsh/base`'s `DockArea` owns the workspace's behavior: tab drags into
// another group, to a group's edge (a new split) or along a tab bar, all
// run natively; the splits' and docks' handles; selection, closing and
// what may move. The layout is the app's data: the area reports every edit
// as the whole new layout and the app passes it back. Each panel's content
// is the app's too, rendered once per key, so it keeps its state wherever
// the panel moves. The look is the gallery's theme classes.

/** The panels, by key: their titles. */
const TITLES: Record<string, string> = {
  files: "Files",
  readme: "README.md",
  main: "main.rs",
  preview: "Preview",
  terminal: "Terminal",
};
const title = (panel: string) => TITLES[panel] ?? panel;

/** The docks' names, by placement. */
const DOCKS: Record<string, string> = { left: "Files", bottom: "Terminal" };

type DockNode = DockAreaLayout["center"];
type DockRegion = NonNullable<DockAreaLayout["left"]>;

/** The workspace at first: two editor groups side by side, the files on
 * the left and a terminal under them. */
function initial(): DockAreaLayout {
  return {
    center: {
      type: "split",
      axis: "horizontal",
      children: [
        { type: "tabs", panels: ["readme", "main"], activeIndex: 1 },
        { type: "tabs", panels: ["preview"] },
      ],
      sizes: [300, null],
    },
    left: { layout: { type: "tabs", panels: ["files"] }, size: 200 },
    bottom: { layout: { type: "tabs", panels: ["terminal"] }, size: 160 },
  };
}

/** A layout as a line of text: `row(...)` / `column(...)` for splits,
 * `[a, b]` for tab groups. */
function describe(node: DockNode): string {
  if (node.type === "tabs") return `[${node.panels.map(title).join(", ")}]`;
  return `${node.axis === "horizontal" ? "row" : "column"}(${node.children.map(describe).join(" ")})`;
}

function describeDock(name: string, dock: DockRegion | null | undefined): string {
  if (!dock) return `${name}: none`;
  return `${name}: ${describe(dock.layout)} ${Math.round(dock.size ?? 0)} pt, ${dock.open === false ? "closed" : "open"}`;
}

/** The size of the center's first slot, when the center is a split. */
function firstSlot(node: DockNode): string {
  if (node.type !== "split") return "—";
  const size = node.sizes?.[0];
  return size == null ? "auto" : `${Math.round(size)} pt`;
}

/** An editor: a count of its edits, which survives every move. */
function Editor(props: { panel: string }) {
  const edits = signal(0);
  return (
    <View class="flex-1 gap-3 p-4">
      <View class="flex-row items-center gap-3">
        <Button
          onPress={() => edits.set(edits() + 1)}
          accessibilityLabel={`Edit ${title(props.panel)}`}
          class="rounded-md bg-primary px-3 py-1.5 active:opacity-70"
        >
          <Text class="text-[13px] font-medium text-primary-foreground">Edit</Text>
        </Button>
        <Text class="text-sm text-foreground">{`${title(props.panel)}: ${edits()} ${edits() === 1 ? "edit" : "edits"}`}</Text>
      </View>
      <Text class="font-mono text-xs text-muted-foreground">{props.panel === "main" ? "fn main() { hopper::run(); }" : "# Hopper, a dockable workspace"}</Text>
    </View>
  );
}

/** What a panel shows, by key. */
function Panel(props: { panel: string }) {
  switch (props.panel) {
    case "readme":
    case "main":
      return <Editor panel={props.panel} />;
    case "files":
      return (
        <View class="flex-1 gap-1 p-3">
          {["src/", "src/app.rs", "src/lib.rs", "Cargo.toml"].map((line) => (
            <Text class="font-mono text-xs text-foreground">{line}</Text>
          ))}
        </View>
      );
    case "preview":
      return (
        <View class="flex-1 items-center justify-center p-4">
          <Text class="text-sm text-muted-foreground">Live preview</Text>
        </View>
      );
    default:
      return (
        <View class="flex-1 gap-1 p-3">
          <Text class="font-mono text-xs text-foreground">$ cargo run</Text>
          <Text class="font-mono text-xs text-muted-foreground">Finished in 1.92s</Text>
        </View>
      );
  }
}

/**
 * Dock: a `DockArea` workspace, editor tabs in a center split, files in a
 * left dock and a terminal in a bottom dock; tabs dragged between groups
 * and to their edges, closed, the docks toggled and resized. Narrower than
 * 600 pt it shows one group at a time and presents a dock at a time, as a
 * drawer or a sheet over the editors, leaving the layout's `open` as it is.
 */
export function Dock() {
  const layout = signal<DockAreaLayout>(initial());
  const locked = signal(false);
  const last = signal("—");
  // The docks that show (`onDockOpenChange`): open ones in the full
  // presentation, none at first at compact width, where the area is less
  // than 600 wide (a phone's window).
  const wide = untracked(windowSize).width >= 600;
  const shown = signal<Record<string, boolean>>({ left: wide, bottom: wide });
  let area!: DockAreaRef;
  const pointer = platform.idiom === "desktop";

  const tool = (label: string, name: string, onPress: () => void, on: () => boolean) => (
    <Button
      onPress={onPress}
      selected={on()}
      accessibilityLabel={label}
      class="rounded-md border border-border px-3 py-1.5 active:opacity-70 selected:bg-muted"
    >
      <Text class="text-[13px] font-medium text-foreground">{name}</Text>
    </Button>
  );

  return (
    <View class="flex-1 gap-3 bg-background p-4">
      <View class="flex-row flex-wrap items-center gap-2">
        {tool("Toggle files", "Files", () => area.toggleDock("left"), () => shown().left)}
        {tool("Toggle terminal", "Terminal", () => area.toggleDock("bottom"), () => shown().bottom)}
        {tool("Lock the layout", "Lock", () => locked.set(!locked()), locked)}
        {tool("Reset the layout", "Reset", () => layout.set(initial()), () => false)}
        <Text class="flex-1 text-right text-xs text-muted-foreground">
          {hint("Hold a tab to reorder it", "Drag a tab onto another group, or to a group's edge to split it")}
        </Text>
      </View>
      {/* At most 700 wide: the tests drag tabs by the places this gives
          them, whatever the window's size (one 1000 wide or more). */}
      <View class="w-full max-w-[700px] flex-1 overflow-hidden rounded-lg border border-border">
        <DockArea
          ref={area}
          class="flex-1"
          layout={layout()}
          onLayoutChange={(next) => layout.set(next)}
          onPanelRemove={(panel) => last.set(`Closed ${title(panel)}`)}
          onPanelActiveChange={(panel, active) => active && last.set(`Showing ${title(panel)}`)}
          onDockOpenChange={(placement, open) => shown.set({ ...shown(), [placement]: open })}
          locked={locked()}
          groupClass="flex-1 bg-background"
          tabBarClass="flex-row border-b border-border bg-muted"
          tabClass="group"
          contentClass="flex-1"
          handleAccessibilityLabel="Resize"
          handleClass="basis-px bg-border group-hover:bg-muted-foreground group-active:bg-primary"
          dockClass="bg-muted"
          dropIndicatorClass="rounded-md border-2 border-primary bg-primary/10"
          drawerClass="border-r border-border bg-background"
          sheetClass="rounded-t-xl bg-background"
          backdropClass="bg-black/30"
          renderTab={(tab) => (
            // A fixed size: the tests drag tabs by their places.
            <View
              testID={`tab-${tab.panel}`}
              class="h-9 w-28 flex-row items-center gap-1 pl-3 pr-1 group-selected:bg-background"
            >
              <Text numberOfLines={1} class="flex-1 text-[13px] text-muted-foreground group-selected:text-foreground">
                {title(tab.panel)}
              </Text>
              <If when={tab.closable && (pointer || tab.active)}>
                <Button
                  onPress={() => area.closePanel(tab.panel)}
                  accessibilityLabel={`Close ${title(tab.panel)}`}
                  class="size-6 items-center justify-center rounded-sm hover:bg-muted active:opacity-70"
                >
                  <Text class="text-xs text-muted-foreground">×</Text>
                </Button>
              </If>
            </View>
          )}
          renderDock={(dock) => (
            <View class="h-8 flex-row items-center border-b border-border pl-3 pr-1">
              <Text class="flex-1 text-xs font-semibold text-muted-foreground">{DOCKS[dock.placement] ?? dock.placement}</Text>
              <Button
                // Toggled: at compact width that dismisses the drawer, in
                // the full presentation it closes the open dock.
                onPress={() => area.toggleDock(dock.placement)}
                accessibilityLabel={`Hide ${DOCKS[dock.placement] ?? dock.placement}`}
                class="rounded-sm px-2 py-0.5 hover:bg-background active:opacity-70"
              >
                <Text class="text-xs text-muted-foreground">Hide</Text>
              </Button>
            </View>
          )}
          renderDragPreview={(tab) => (
            <View class="rounded-md border border-border bg-background px-3 py-1.5 shadow-md">
              <Text class="text-[13px] text-foreground">{title(tab.panel)}</Text>
            </View>
          )}
          renderPanel={(panel) => <Panel panel={panel.id} />}
          renderEmpty={() => (
            <View class="flex-1 items-center justify-center">
              <Text class="text-sm text-muted-foreground">Every editor is closed</Text>
            </View>
          )}
        />
      </View>
      <View class="gap-0.5">
        <Text class="font-mono text-xs text-foreground">{`Center: ${describe(layout().center)}`}</Text>
        <Text class="font-mono text-xs text-muted-foreground">{`First slot: ${firstSlot(layout().center)}`}</Text>
        <Text class="font-mono text-xs text-muted-foreground">{describeDock("Left", layout().left)}</Text>
        <Text class="font-mono text-xs text-muted-foreground">{describeDock("Bottom", layout().bottom)}</Text>
        <Text class="font-mono text-xs text-muted-foreground">{`Showing docks: ${["left", "bottom"].filter((p) => shown()[p]).map((p) => DOCKS[p]).join(", ") || "none"}`}</Text>
        <Text class="font-mono text-xs text-muted-foreground">{`Last event: ${last()}`}</Text>
      </View>
    </View>
  );
}
