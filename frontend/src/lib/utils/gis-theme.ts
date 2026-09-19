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
const XHTML_NAMESPACE = "http://www.w3.org/1999/xhtml";

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
  // Element -> "stop listening to it", so a link is only ever tracked once
  // while it is in the document, and is dropped when it leaves.
  const trackedLinks = new Map<Element, () => void>();

  // Deliberately not `instanceof HTMLLinkElement`: that is per realm, so a link
  // belonging to another document would not match. Tag and rel are checked the
  // way HTML defines them - rel keywords are ASCII case-insensitive, and an
  // SVG-namespace <link> is not a stylesheet link.
  function isStylesheetLink(node: Node): node is Element {
    const element = node as Element;
    if (element.nodeType !== 1) return false;
    if (
      element.localName !== "link" ||
      element.namespaceURI !== XHTML_NAMESPACE
    ) {
      return false;
    }
    const rel = element.getAttribute("rel") ?? "";
    return rel.toLowerCase().split(/\s+/).includes("stylesheet");
  }

  function untrackLink(node: Node) {
    const stopListening = trackedLinks.get(node as Element);
    if (!stopListening) return;
    stopListening();
    trackedLinks.delete(node as Element);
  }

  // A <link rel="stylesheet"> is in the DOM well before its CSS has loaded, so
  // its insertion alone is not a readiness signal: we have to wait for its load
  // event too, or an attempt that ran in between would be the last one.
  //
  // We keep listening for as long as the link is in the document rather than
  // releasing after the first event: a link that fails can be retried by
  // pointing its href somewhere else, and that fetch reports on the same
  // element. Removal is what bounds the map.
  function trackStylesheetLink(node: Node) {
    if (!isStylesheetLink(node) || trackedLinks.has(node)) return;
    const link = node;
    link.addEventListener("load", apply);
    link.addEventListener("error", apply);
    trackedLinks.set(link, () => {
      link.removeEventListener("load", apply);
      link.removeEventListener("error", apply);
    });
  }

  const buttonObserver = new MutationObserver(apply);
  buttonObserver.observe(container, { childList: true, subtree: true });

  const stylesheetObserver = new MutationObserver((records) => {
    for (const record of records) {
      // `rel` can turn a link we ignored into one we care about (preload
      // promoted to stylesheet), and `href` can point it at a new fetch.
      if (record.type === "attributes") {
        if (isStylesheetLink(record.target)) trackStylesheetLink(record.target);
        else untrackLink(record.target);
        continue;
      }
      for (const node of Array.from(record.addedNodes))
        trackStylesheetLink(node);
      for (const node of Array.from(record.removedNodes)) untrackLink(node);
    }
    apply();
  });
  stylesheetObserver.observe(doc.head, {
    childList: true,
    subtree: true,
    attributes: true,
    attributeFilter: ["rel", "href"],
  });

  // Links already in <head> may still be in flight when we start watching.
  for (const link of Array.from(doc.head.children)) trackStylesheetLink(link);

  apply();

  return () => {
    buttonObserver.disconnect();
    stylesheetObserver.disconnect();
    for (const stopListening of trackedLinks.values()) stopListening();
    trackedLinks.clear();
  };
}
