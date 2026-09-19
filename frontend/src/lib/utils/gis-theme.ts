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
//
// These are obfuscated, undocumented class names, so treat them as a best
// effort: see `enforceDarkTheme` for how we avoid making things worse if
// Google renames them.
const GIS_DARK_CLASSES = ["MFS4be-JaPV2b-Ia7Qfc", "MFS4be-Ia7Qfc"];
const GIS_LIGHT_CLASSES = ["i5vt6e-Ia7Qfc", "i5vt6e-to915-Ia7Qfc"];

// GIS ships its stylesheet inside the same injected <style> block, so if the
// dark classes we are about to add are not styled by anything on the page,
// Google has renamed them. Stripping the light classes then would leave an
// unstyled button - worse than the white pill we set out to fix - so in that
// case we leave the GIS rendering exactly as it is.
function darkClassesAreStyled(doc: Document): boolean {
  for (const sheet of Array.from(doc.styleSheets)) {
    let rules: CSSRuleList;
    try {
      rules = sheet.cssRules;
    } catch {
      continue; // cross-origin stylesheet, not ours to inspect
    }
    for (const rule of Array.from(rules)) {
      if (rule.cssText?.includes(GIS_DARK_CLASSES[0])) return true;
    }
  }
  return false;
}

/**
 * Force GIS's dark (filled_black) theme onto whichever button variant it
 * rendered inside `container`. Safe to call repeatedly: it is a no-op once the
 * button already carries the dark classes, which is the normal case for the
 * standard (non-personalized) button.
 */
export function enforceDarkTheme(container: HTMLElement): void {
  const button = container.querySelector<HTMLElement>(
    '[role="button"][aria-labelledby="button-label"]',
  );
  if (!button || button.classList.contains(GIS_DARK_CLASSES[0])) return;
  if (!darkClassesAreStyled(container.ownerDocument)) return;
  button.classList.remove(...GIS_LIGHT_CLASSES);
  button.classList.add(...GIS_DARK_CLASSES);
}
