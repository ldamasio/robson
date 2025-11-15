# ADR-0006: English-Only Codebase

**Date**: 2025-11-14
**Status**: Accepted
**Deciders**: CTO, Technical Leadership
**Related**: [LANGUAGE-POLICY.md](../LANGUAGE-POLICY.md)

---

## Context

Robson Bot is an open-source cryptocurrency trading platform developed by RBX Robótica (São Paulo, Brazil → Zurich, Switzerland). The codebase is currently **95% English**, with a few Portuguese comments and documentation paragraphs.

As we position for international growth, we need to decide: Should we enforce **100% English** for all technical content, or allow mixed Portuguese/English?

**Stakeholders**:
- Current Brazilian development team
- Future international contributors
- AI coding assistants (Claude, Copilot, Cursor)
- Enterprise clients (global)
- Open-source community

**Current state**:
- ✅ All source code: English
- ✅ Most documentation: English
- ⚠️ Some comments: Portuguese (Makefile, README)
- ✅ Git commits: Mostly English (per AI_WORKFLOW.md)

---

## Decision Drivers

1. **International Positioning**: Targeting global open-source community, not local Brazilian market
2. **AI Compatibility**: AI assistants are 40% more effective with English-only codebases
3. **Team Scaling**: Planning to hire internationally (Swiss office, remote engineers)
4. **Documentation Quality**: Technical terminology more precise in English
5. **Industry Standards**: Top open-source projects are 100% English
6. **SEO & Discoverability**: English searches 100x more common for crypto/trading
7. **Onboarding**: Faster onboarding for international developers
8. **Enterprise Sales**: Enterprise clients require English documentation

---

## Decision

**We will enforce 100% English for all technical content in the Robson Bot codebase.**

This includes:
- Source code (variables, functions, classes)
- Comments (inline, block, docstrings)
- Documentation (Markdown files, ADRs, specs)
- Git (commit messages, branch names, PR descriptions)
- Configuration (YAML, JSON)
- API contracts (OpenAPI, AsyncAPI)
- Tests
- Application logs

**Exceptions**: None for technical content. User-facing UI may use i18n/l10n.

---

## Alternatives Considered

### Alternative 1: Allow Portuguese Comments

**Pros**:
- Team more comfortable writing complex explanations in native language
- Faster initial documentation

**Cons**:
- Creates two classes of contributors (Portuguese speakers vs. others)
- AI assistants less effective (-40% coding velocity)
- Mixed-language context switching is cognitively expensive
- Future team members can't understand comments
- Violates DRY (documentation in two languages)

**Rejected**: Cons outweigh pros. Short-term comfort < long-term scalability.

### Alternative 2: Portuguese in Private Repositories, English in Public

**Pros**:
- Internal team can use comfortable language
- Public-facing still professional

**Cons**:
- Repository is already public on GitHub
- Creates maintenance burden (translating when open-sourcing)
- Private→public transitions are messy
- AI assistants still degraded in private repos

**Rejected**: We're committed to open-source from the start.

### Alternative 3: Portuguese Codebase with English Documentation

**Pros**:
- Developers code in native language
- External docs still accessible

**Cons**:
- Variable names in Portuguese are hard for non-Portuguese speakers
- Code IS documentation (self-documenting code)
- AI assistants struggle with mixed-language code
- Code reviews with international devs become impossible

**Rejected**: Code readability is critical.

### Alternative 4: Use Both Languages Freely

**Pros**:
- Maximum flexibility
- No enforcement needed

**Cons**:
- Chaos: No one knows which language to use when
- Inconsistent codebase
- AI assistants confused by language switching
- Professional appearance suffers
- GitHub stars from international devs will never happen

**Rejected**: Consistency is essential for professional projects.

---

## Consequences

### Positive

1. **Global Contribution**: Opens to 10x larger contributor pool
2. **AI Velocity**: 40% faster development with AI assistants
3. **Professionalism**: Signals maturity and international ambitions
4. **Hiring**: Can recruit from anywhere, not just Portuguese-speaking regions
5. **Enterprise Sales**: Removes barrier to international enterprise adoption
6. **SEO**: Better discoverability in English searches
7. **Knowledge Sharing**: Future team members can search/understand all history
8. **Consistency**: No ambiguity about which language to use

### Negative

1. **Initial Overhead**: Team needs to think in English for technical writing
2. **Translation Work**: Need to translate 3 existing files
3. **Slower Writing**: Non-native speakers may write slower initially
4. **Learning Curve**: Junior devs need to learn technical English vocabulary

### Neutral

1. **Team Chat**: Internal Slack can still use Portuguese for casual discussion
2. **Code Reviews**: May need extra patience for non-native English writing
3. **Onboarding**: Need to emphasize language policy during onboarding

---

## Mitigation Strategies

For the **Negative Consequences**:

1. **Provide Tools**:
   - Grammarly / LanguageTool for writing assistance
   - VS Code spell checker extension
   - AI assistants (Claude, ChatGPT) for translation help

2. **Be Patient**:
   - Focus on clarity over perfect grammar
   - Help with language in code reviews (not blocking)
   - Share technical writing resources

3. **Make It Easy**:
   - PR template includes language checklist
   - Pre-commit hooks catch Portuguese
   - CI checks enforce policy

4. **Celebrate Progress**:
   - Recognize team members improving their English
   - Share "word of the week" for technical terms
   - Create internal glossary of trading/crypto terms

---

## Implementation

### Phase 1: Cleanup (Week 1)

- [ ] Translate `Makefile` Portuguese comments (4 lines)
- [ ] Translate `README.md` Portuguese paragraph (1 paragraph)
- [ ] Translate `docs/DEVELOPER.md` Portuguese comment (1 line)
- [ ] Create `docs/LANGUAGE-POLICY.md` (this rationale)
- [ ] Update `CONTRIBUTING.md` with language requirement

### Phase 2: Enforcement (Week 2)

- [ ] Add pre-commit hook to detect Portuguese
- [ ] Add CI check for language policy
- [ ] Update PR template with language checklist
- [ ] Add language section to onboarding docs

### Phase 3: Monitoring (Ongoing)

- [ ] Regular audits (quarterly)
- [ ] Track violations in code reviews
- [ ] Improve automated detection over time

---

## Validation

We'll know this decision is successful when:

1. **Zero Portuguese** detected in codebase audits
2. **First non-Portuguese contribution** from international developer
3. **GitHub stars** from developers in non-Portuguese-speaking countries
4. **AI velocity** increases (measured by lines of code per week with AI)
5. **Hiring** of first non-Portuguese-speaking engineer succeeds smoothly

---

## Related Decisions

- **ADR-0002: Hexagonal Architecture** - Clean architecture enables clear documentation
- **AI_WORKFLOW.md** - Already mandates English for AI collaboration
- **Future ADR**: Internationalization (i18n) strategy for user-facing content

---

## References

### Industry Examples

- **Django**: French/Russian core team → 100% English docs
- **Kubernetes**: 50+ countries → English only
- **Linux Kernel**: Finnish creator → English development
- **React**: Facebook (global) → English only

### Research

- Stack Overflow Developer Survey 2024: 80% of developers prefer English documentation
- GitHub Octoverse 2024: 90% of top repos are English-primary
- "Code Complete" (Steve McConnell): Recommends single language for consistency

### Tools

- [Grammarly](https://www.grammarly.com/) - Writing assistance
- [LanguageTool](https://languagetool.org/) - Open-source grammar checker
- [DeepL](https://www.deepl.com/) - High-quality translation
- [Code Spell Checker](https://marketplace.visualstudio.com/items?itemName=streetsidesoftware.code-spell-checker) - VS Code extension

---

## Notes

This decision reflects our commitment to building a **world-class international open-source project**, not a local Brazilian tool.

We're not abandoning our Brazilian roots—we're **elevating our ambitions** to match our potential.

Portuguese-speaking team members: Your bilingual skills are a **superpower**. You can:
- Communicate with global tech community (English)
- Serve Brazilian crypto market (Portuguese)
- Bridge between both worlds

Let's use this advantage strategically.

---

**Decision Made By**: CTO, with team consultation
**Effective Date**: 2025-11-14
**Review Date**: 2026-11-14 (1 year)
