// @vitest-environment jsdom
import { describe, it, expect, beforeEach } from "vitest";
import { enforceDarkTheme, watchGisTheme } from "$lib/utils/gis-theme";

const DARK = "MFS4be-JaPV2b-Ia7Qfc";
const DARK_2 = "MFS4be-Ia7Qfc";
const LIGHT = "i5vt6e-Ia7Qfc";

// Stand-in for the stylesheet GIS injects into <head> when it renders a
// button. Only the filled_black surface matters for these tests.
function installGisStylesheet() {
  const style = document.createElement("style");
  style.textContent = `.${DARK}{background-color:#202124;color:#e8eaed}`;
  document.head.appendChild(style);
}

// Mirrors the markup GIS emits: the label id lives on a nested span, and the
// personalized variant ships the LIGHT theme classes - that is the bug.
function personalizedButton(themeClasses = LIGHT): string {
  return `
    <div>
      <div role="button" aria-labelledby="button-label" class="nsm7Bb-HzV7m-LgbsSe jVeSEe ${themeClasses} uaxL4e-RbRzK">
        <span id="button-label">Fazer login como Leandro</span>
      </div>
    </div>`;
}

function mount(markup: string): HTMLElement {
  const container = document.createElement("div");
  container.innerHTML = markup;
  document.body.appendChild(container);
  return container;
}

function button(container: HTMLElement): HTMLElement {
  return container.querySelector<HTMLElement>('[role="button"]')!;
}

// MutationObserver callbacks are delivered as microtasks.
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

beforeEach(() => {
  document.head.innerHTML = "";
  document.body.innerHTML = "";
});

describe("enforceDarkTheme", () => {
  it("darkens the personalized button, which GIS ships with light classes", () => {
    installGisStylesheet();
    const container = mount(personalizedButton());

    enforceDarkTheme(container);

    const cls = button(container).classList;
    expect(cls.contains(DARK)).toBe(true);
    expect(cls.contains(DARK_2)).toBe(true);
    expect(cls.contains(LIGHT)).toBe(false);
    // Shape, layout and variant classes must survive untouched.
    expect(cls.contains("uaxL4e-RbRzK")).toBe(true);
    expect(cls.contains("jVeSEe")).toBe(true);
    expect(cls.contains("nsm7Bb-HzV7m-LgbsSe")).toBe(true);
  });

  it("leaves the standard button alone: it already carries the dark classes", () => {
    installGisStylesheet();
    const container = mount(personalizedButton(`${DARK} ${DARK_2}`));
    const before = button(container).className;

    enforceDarkTheme(container);

    expect(button(container).className).toBe(before);
  });

  it("restores GIS's own classes when the dark ones no longer paint a dark surface", () => {
    // Stylesheet present, but filled_black now lives under another name.
    const style = document.createElement("style");
    style.textContent = `.${DARK}-v2{background-color:#202124}`;
    document.head.appendChild(style);
    const container = mount(personalizedButton());
    const before = button(container).className;

    enforceDarkTheme(container);

    // Byte-for-byte the markup GIS rendered - no stripped light classes, no
    // inert dark ones left behind.
    expect(button(container).className).toBe(before);
    expect(button(container).classList.contains(LIGHT)).toBe(true);
  });

  it("does not latch: a failed attempt still succeeds once the stylesheet lands", () => {
    const container = mount(personalizedButton());

    enforceDarkTheme(container);
    expect(button(container).classList.contains(DARK)).toBe(false);

    installGisStylesheet();
    enforceDarkTheme(container);

    expect(button(container).classList.contains(DARK)).toBe(true);
  });

  it("tolerates a container GIS has not rendered into yet", () => {
    installGisStylesheet();
    const container = mount("");

    expect(() => enforceDarkTheme(container)).not.toThrow();
  });
});

describe("watchGisTheme", () => {
  it("darkens the button GIS swaps in after the initial render", async () => {
    installGisStylesheet();
    const container = mount("");
    const unwatch = watchGisTheme(container);

    container.innerHTML = personalizedButton();
    await flush();

    expect(button(container).classList.contains(DARK)).toBe(true);
    unwatch();
  });

  it("darkens a button that arrived before the stylesheet, with no further button mutation", async () => {
    // The ordering hazard: enforcement runs first and correctly declines,
    // then the stylesheet lands and nothing touches the button again.
    const container = mount(personalizedButton());
    const unwatch = watchGisTheme(container);
    await flush();
    expect(button(container).classList.contains(DARK)).toBe(false);

    installGisStylesheet();
    await flush();

    expect(button(container).classList.contains(DARK)).toBe(true);
    unwatch();
  });

  it("retries when a stylesheet <link> finishes loading, not merely when it is inserted", async () => {
    const container = mount(personalizedButton());
    const unwatch = watchGisTheme(container);
    await flush();

    // Inserted, but its CSS is not available yet - the attempt it triggers
    // must fail and leave the button as GIS rendered it.
    const link = document.createElement("link");
    link.rel = "stylesheet";
    link.href = "https://accounts.google.com/gsi/style";
    document.head.appendChild(link);
    await flush();
    expect(button(container).classList.contains(DARK)).toBe(false);

    // Now the CSS is live and the link reports it.
    installGisStylesheet();
    link.dispatchEvent(new Event("load"));

    expect(button(container).classList.contains(DARK)).toBe(true);
    unwatch();
  });

  it("matches rel case-insensitively, as HTML defines it", async () => {
    const container = mount(personalizedButton());
    const unwatch = watchGisTheme(container);
    await flush();

    const link = document.createElement("link");
    link.setAttribute("rel", "StyleSheet");
    document.head.appendChild(link);
    await flush();

    installGisStylesheet();
    link.dispatchEvent(new Event("load"));

    expect(button(container).classList.contains(DARK)).toBe(true);
    unwatch();
  });

  it("tracks links from another realm, where instanceof HTMLLinkElement fails", async () => {
    // An iframe has its own constructors, so `instanceof HTMLLinkElement`
    // against the outer realm is false for its elements. That is the case a
    // regression back to instanceof would break.
    const frame = document.createElement("iframe");
    document.body.appendChild(frame);
    const inner = frame.contentDocument!;
    expect(inner.createElement("link") instanceof HTMLLinkElement).toBe(false);

    const container = inner.createElement("div");
    container.innerHTML = personalizedButton();
    inner.body.appendChild(container);

    const unwatch = watchGisTheme(container);
    await flush();
    expect(button(container).classList.contains(DARK)).toBe(false);

    const link = inner.createElement("link");
    link.rel = "stylesheet";
    inner.head.appendChild(link);
    await flush();
    expect(button(container).classList.contains(DARK)).toBe(false);

    const style = inner.createElement("style");
    style.textContent = `.${DARK}{background-color:#202124}`;
    inner.head.appendChild(style);
    link.dispatchEvent(new Event("load"));

    expect(button(container).classList.contains(DARK)).toBe(true);
    unwatch();
    frame.remove();
  });

  it("tracks each link once and drops it once it settles", async () => {
    const container = mount(personalizedButton());
    const unwatch = watchGisTheme(container);
    const link = document.createElement("link");
    link.rel = "stylesheet";

    // Churn: the same link inserted, removed and reinserted.
    document.head.appendChild(link);
    link.remove();
    document.head.appendChild(link);
    await flush();

    installGisStylesheet();
    link.dispatchEvent(new Event("load"));
    expect(button(container).classList.contains(DARK)).toBe(true);

    // Settled links are released, so a later event from one is a no-op.
    button(container).setAttribute(
      "class",
      `nsm7Bb-HzV7m-LgbsSe jVeSEe ${LIGHT}`,
    );
    link.dispatchEvent(new Event("load"));
    expect(button(container).classList.contains(DARK)).toBe(false);

    unwatch();
  });

  it("stops listening to stylesheet links once disposed", async () => {
    const container = mount(personalizedButton());
    const link = document.createElement("link");
    link.rel = "stylesheet";
    document.head.appendChild(link);
    const unwatch = watchGisTheme(container);
    await flush();

    unwatch();
    installGisStylesheet();
    link.dispatchEvent(new Event("load"));

    expect(button(container).classList.contains(DARK)).toBe(false);
  });

  it("stops enforcing once disposed", async () => {
    installGisStylesheet();
    const container = mount("");
    const unwatch = watchGisTheme(container);

    unwatch();
    container.innerHTML = personalizedButton();
    await flush();

    expect(button(container).classList.contains(DARK)).toBe(false);
  });

  it("is idempotent across the many callbacks the observers fire", async () => {
    installGisStylesheet();
    const container = mount(personalizedButton());
    const unwatch = watchGisTheme(container);
    const afterFirst = button(container).className;

    // Unrelated churn inside the container and in <head>.
    button(container).appendChild(document.createElement("span"));
    document.head.appendChild(document.createElement("style"));
    await flush();

    expect(button(container).className).toBe(afterFirst);
    unwatch();
  });
});
