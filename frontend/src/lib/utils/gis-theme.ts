// Google Identity Services honours the `theme` we pass to `renderButton` on
// its standard button, but not on the "personalized" variant it swaps in when
// a Google session already exists ("Fazer login como <nome>", with avatar and
// e-mail): GIS builds that one from a different internal template and it comes
// back carrying the default light theme classes instead of the filled_black
// ones we asked for, so it rendered as a white pill on our dark login card.
//
// We re-apply Google's own filled_black classes rather than hand-rolling
// colours, so the hover/active states and the dimmer second line (the e-mail,
// otherwise #5f6368 on a dark surface) all stay consistent with the theme we
// requested, and the G logo keeps Google's mandated treatment.
const GIS_DARK_CLASSES = ["MFS4be-JaPV2b-Ia7Qfc", "MFS4be-Ia7Qfc"];
const GIS_LIGHT_CLASSES = ["i5vt6e-Ia7Qfc", "i5vt6e-to915-Ia7Qfc"];

// GIS's markup is undocumented and its class names are obfuscated, so we never
// assume the classes above still mean anything. We apply them and then ask the
// browser what it actually painted: if the button did not become a dark
// surface, the classes no longer carry filled_black and we put the button back
// exactly as GIS rendered it. Stripping the light classes without that check
// is how a Google rename would turn the white pill into an unstyled button.
//
// Deliberately a "is it dark" test rather than an exact hex, so Google can
// tweak the shade of filled_black without disabling the fix.
function isDarkSurface(color: string): boolean {
  const match = /^rgba?\(([^)]+)\)$/.exec(color.trim());
  if (!match) return false;
  const parts = match[1].split(",").map((p) => Number.parseFloat(p));
  const [r, g, b, a = 1] = parts;
  if (![r, g, b].every(Number.isFinite) || a < 0.9) return false;
  // Rec. 601 luma; filled_black's #202124 lands at ~33.
  return 0.299 * r + 0.587 * g + 0.114 * b < 96;
}

/**
 * Force GIS's dark (filled_black) theme onto whichever button variant it
 * rendered inside `container`.
 *
 * Safe to call repeatedly: it is a no-op once the button already carries the
 * dark classes, which is the normal case for the standard button. If applying
 * them does not actually produce a dark surface - Google renamed something, or
 * the GIS stylesheet has not landed yet - the button is restored untouched, so
 * a later call can try again.
 */
export function enforceDarkTheme(container: HTMLElement): void {
  const button = container.querySelector<HTMLElement>(
    '[role="button"][aria-labelledby="button-label"]',
  );
  if (!button || button.classList.contains(GIS_DARK_CLASSES[0])) return;

  const original = button.getAttribute("class") ?? "";
  button.classList.remove(...GIS_LIGHT_CLASSES);
  button.classList.add(...GIS_DARK_CLASSES);

  const view = container.ownerDocument.defaultView;
  const painted = view?.getComputedStyle(button).backgroundColor ?? "";
  if (!isDarkSurface(painted)) button.setAttribute("class", original);
}

/**
 * Keep the GIS button dark for as long as it is on screen, and return a
 * disposer.
 *
 * Two things can happen after we first enforce the theme, in either order, so
 * we watch for both: GIS re-renders the button (it does that once it resolves
 * the signed-in account and swaps in the personalized variant), and the
 * stylesheet those theme classes depend on becomes available. Watching only the
 * container would leave the button light forever whenever the stylesheet lands
 * after the last render.
 */
export function watchGisTheme(container: HTMLElement): () => void {
  const doc = container.ownerDocument;
  const apply = () => enforceDarkTheme(container);
  const linkListeners: Array<() => void> = [];

  // A <link rel="stylesheet"> is in the DOM well before its CSS has loaded, so
  // its insertion alone is not a readiness signal: we have to wait for its load
  // event too, or an attempt that ran in between would be the last one.
  function trackStylesheetLink(node: Node) {
    if (!(node instanceof HTMLLinkElement)) return;
    if (!node.rel.split(/\s+/).includes("stylesheet")) return;
    node.addEventListener("load", apply);
    linkListeners.push(() => node.removeEventListener("load", apply));
  }

  const buttonObserver = new MutationObserver(apply);
  buttonObserver.observe(container, { childList: true, subtree: true });

  const stylesheetObserver = new MutationObserver((records) => {
    for (const record of records) {
      for (const node of Array.from(record.addedNodes))
        trackStylesheetLink(node);
    }
    apply();
  });
  stylesheetObserver.observe(doc.head, { childList: true });

  // Links already in <head> may still be in flight when we start watching.
  for (const link of Array.from(doc.head.querySelectorAll("link"))) {
    trackStylesheetLink(link);
  }

  apply();

  return () => {
    buttonObserver.disconnect();
    stylesheetObserver.disconnect();
    for (const remove of linkListeners) remove();
    linkListeners.length = 0;
  };
}
