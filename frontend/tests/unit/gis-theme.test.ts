// @vitest-environment jsdom
import { describe, it, expect, beforeEach } from "vitest";
import { enforceDarkTheme } from "$lib/utils/gis-theme";

const DARK = "MFS4be-JaPV2b-Ia7Qfc";
const DARK_2 = "MFS4be-Ia7Qfc";
const LIGHT = "i5vt6e-Ia7Qfc";

// Stand-in for the stylesheet GIS injects; `enforceDarkTheme` uses it to
// confirm the dark classes it is about to add still mean something.
function installGisStylesheet() {
  const style = document.createElement("style");
  style.textContent = `.nsm7Bb-HzV7m-LgbsSe.${DARK}{background-color:#202124;color:#e8eaed}`;
  document.head.appendChild(style);
}

function mount(buttonClasses: string): HTMLElement {
  const container = document.createElement("div");
  // Mirrors the markup GIS emits: the label id lives on a nested span.
  container.innerHTML = `
    <div>
      <div role="button" aria-labelledby="button-label" class="nsm7Bb-HzV7m-LgbsSe ${buttonClasses}">
        <span id="button-label">Fazer login como Leandro</span>
      </div>
    </div>`;
  document.body.appendChild(container);
  return container;
}

function button(container: HTMLElement): HTMLElement {
  return container.querySelector<HTMLElement>('[role="button"]')!;
}

beforeEach(() => {
  document.head.innerHTML = "";
  document.body.innerHTML = "";
});

describe("enforceDarkTheme", () => {
  it("darkens the personalized button, which GIS ships with light classes", () => {
    installGisStylesheet();
    const container = mount(`jVeSEe ${LIGHT} uaxL4e-RbRzK`);

    enforceDarkTheme(container);

    const cls = button(container).classList;
    expect(cls.contains(DARK)).toBe(true);
    expect(cls.contains(DARK_2)).toBe(true);
    expect(cls.contains(LIGHT)).toBe(false);
    // Shape and layout classes must survive untouched.
    expect(cls.contains("uaxL4e-RbRzK")).toBe(true);
    expect(cls.contains("jVeSEe")).toBe(true);
  });

  it("leaves the standard button alone: it already carries the dark classes", () => {
    installGisStylesheet();
    const container = mount(`TzA9Ye-LgbsSe ${DARK} ${DARK_2} uaxL4e-RbRzK`);
    const before = button(container).className;

    enforceDarkTheme(container);

    expect(button(container).className).toBe(before);
  });

  it("is idempotent across the repeated calls the MutationObserver makes", () => {
    installGisStylesheet();
    const container = mount(`jVeSEe ${LIGHT}`);

    enforceDarkTheme(container);
    const afterFirst = button(container).className;
    enforceDarkTheme(container);
    enforceDarkTheme(container);

    expect(button(container).className).toBe(afterFirst);
  });

  it("does nothing when Google renamed the dark classes, rather than stripping the light ones", () => {
    // No GIS stylesheet: the dark classes we would add are styled by nothing.
    const container = mount(`jVeSEe ${LIGHT}`);

    enforceDarkTheme(container);

    const cls = button(container).classList;
    expect(cls.contains(LIGHT)).toBe(true);
    expect(cls.contains(DARK)).toBe(false);
  });

  it("tolerates a container GIS has not rendered into yet", () => {
    installGisStylesheet();
    const container = document.createElement("div");
    document.body.appendChild(container);

    expect(() => enforceDarkTheme(container)).not.toThrow();
  });

  it("ignores cross-origin stylesheets it cannot read", () => {
    const hostile = document.createElement("style");
    document.head.appendChild(hostile);
    Object.defineProperty(hostile.sheet!, "cssRules", {
      get() {
        throw new DOMException("cross-origin", "SecurityError");
      },
    });
    installGisStylesheet();
    const container = mount(`jVeSEe ${LIGHT}`);

    expect(() => enforceDarkTheme(container)).not.toThrow();
    expect(button(container).classList.contains(DARK)).toBe(true);
  });
});
