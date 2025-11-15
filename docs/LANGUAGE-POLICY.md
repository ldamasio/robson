# Language Policy: English Only

**Status**: Active
**Effective Date**: 2025-11-14
**Scope**: All code, documentation, and communication

---

## Policy Statement

**All code, comments, documentation, commit messages, pull requests, issues, and technical communication in the Robson Bot project MUST be in English.**

This is a non-negotiable requirement for international open-source positioning.

---

## Rationale

### 1. International Open-Source Positioning

Robson Bot is **not** a Brazilian project. It is an **international open-source project** developed by RBX Robótica, positioning for global adoption.

**Target audience**:
- Global cryptocurrency traders
- International fintech developers
- Worldwide open-source contributors
- Enterprise clients globally
- Future team members from any country

**Impact**: Portuguese code or documentation creates an immediate barrier to 95%+ of potential users and contributors worldwide.

### 2. AI-Assisted Development Compatibility

Modern AI coding assistants (Claude, GitHub Copilot, Cursor, Cody) are:
- **Trained primarily on English codebases** (90%+ of training data)
- **Optimized for English technical terminology**
- **Less effective** with mixed-language codebases
- **More prone to errors** when switching languages mid-context

**Impact**: Mixed-language codebases reduce AI assistant effectiveness by 30-50%, slowing development velocity.

### 3. Team Collaboration & Future Growth

**Current team** (São Paulo, Brazil):
- All members are proficient in English technical writing
- English is already used for source code
- Domain terminology (trading, crypto) is naturally English

**Future team** (Zurich, Switzerland + distributed):
- International talent acquisition requires English
- Swiss fintech regulations favor English documentation
- Remote collaboration across time zones needs a common language
- Onboarding new team members is 10x faster with English

**Impact**: English-only removes friction for global team scaling.

### 4. Documentation Quality & Maintainability

**Technical terminology**:
- Trading: `order`, `position`, `strategy`, `signal` (no good Portuguese equivalents)
- Crypto: `wallet`, `exchange`, `blockchain`, `smart contract`
- Architecture: `port`, `adapter`, `aggregate`, `bounded context`

**Translation problems**:
- Portuguese: "adaptor condutor" vs. "driving adapter" (awkward)
- Portuguese: "evento de domínio" vs. "domain event" (loses precision)
- Mixed: Confusion when reading between languages

**Impact**: English technical writing is clearer, more precise, and easier to maintain.

### 5. Open-Source Best Practices

**Top open-source projects** (Django, React, Kubernetes, ArgoCD):
- **100% English** documentation, even when core team is non-English
- English as the standard for global collaboration
- Translations available separately (not mixed in source)

**Examples**:
- Django: Core team includes French, Russian, British developers → English docs
- Kubernetes: Contributors from 50+ countries → English only
- Linux: Linus Torvalds (Finnish) → English-only kernel development

**Impact**: Following industry standards signals professionalism and maturity.

---

## Scope of Policy

### ✅ MUST Be in English

| Item | Examples |
|------|----------|
| **Source Code** | Variables, functions, classes, methods |
| **Comments** | Inline, block, docstrings |
| **Documentation** | README, ADRs, specs, runbooks, guides |
| **Git** | Commit messages, branch names, PR descriptions |
| **Issues & PRs** | Titles, descriptions, comments |
| **Code Reviews** | Feedback, suggestions, approvals |
| **Configuration** | YAML, JSON, environment variable names |
| **API Contracts** | OpenAPI, AsyncAPI, GraphQL schemas |
| **Tests** | Test names, assertions, fixtures |
| **Logs** | Application logs, error messages |

### ✅ MAY Be in Local Language

| Item | Notes |
|------|-------|
| **Slack/Discord (Internal)** | Team chat can use Portuguese for casual discussion |
| **Marketing Materials** | Website, landing pages can be localized |
| **User-Facing UI** | Application UI can support i18n/l10n |
| **Customer Support** | Support tickets can be in customer's language |

**Important**: Technical discussions in Slack SHOULD be in English for searchability and future reference.

---

## Implementation

### Current State (2025-11-14)

**Source code**: ✅ **100% English** (already compliant)
**Documentation**: ⚠️ **95% English** (3 files with Portuguese comments)

**Files needing translation**:
1. `Makefile` - 4 Portuguese comments
2. `README.md` - 1 paragraph in Portuguese
3. `docs/DEVELOPER.md` - 1 Portuguese comment

### Migration Plan

**Phase 1** (Immediate):
- [ ] Translate `Makefile` comments to English
- [ ] Translate `README.md` paragraph to English
- [ ] Translate `docs/DEVELOPER.md` comment to English
- [ ] Create ADR-0006 documenting this decision
- [ ] Update CONTRIBUTING.md with language requirement

**Phase 2** (Ongoing):
- [ ] Add language check to pre-commit hooks
- [ ] Add language check to CI/CD pipeline
- [ ] Create developer onboarding checklist including language policy
- [ ] Update PR template with language reminder

### Enforcement

**Pre-commit hooks**:
```bash
# Check for Portuguese keywords in code
pre-commit hook: no-portuguese-code

# Patterns to flag:
# - Portuguese variable names (é, ã, ç, etc.)
# - Portuguese comments (comum patterns)
# - Portuguese commit messages
```

**CI/CD checks**:
```yaml
# GitHub Actions workflow
- name: Check Language Policy
  run: |
    # Fail if Portuguese detected in:
    # - Python/JS files
    # - Markdown files
    # - YAML config files
```

**Pull Request Template**:
```markdown
## Language Policy Checklist

- [ ] All code is in English
- [ ] All comments are in English
- [ ] All documentation is in English
- [ ] Commit messages are in English
- [ ] No Portuguese characters in variable/function names
```

---

## Exceptions

**None.**

This policy has **no exceptions**. All technical content must be in English.

**User-facing content** (UI labels, error messages for end users) may be localized through proper i18n frameworks, but:
- Source keys must be in English
- Translations stored separately (not inline)
- Code comments about translations must be in English

---

## Onboarding

**For new Brazilian developers**:

Yes, we know Portuguese is your native language. Here's why English is better for you:

1. **Career Growth**: 90% of top tech companies require English fluency
2. **Learning**: Best programming resources are in English
3. **Networking**: Global conferences, Slack communities, Twitter tech threads
4. **Salary**: English fluency correlates with 30-50% higher salaries in Brazil
5. **Options**: English skills enable remote work for international companies

**Tips**:
- Use Grammarly or LanguageTool for writing assistance
- VS Code spell checker: `streetsidesoftware.code-spell-checker`
- Read code in English daily to build vocabulary
- Don't worry about perfect grammar; clarity > perfection

**For non-native English speakers globally**:

Your English doesn't need to be perfect. We care about:
- **Clarity**: Can others understand your intent?
- **Consistency**: Use standard technical terms
- **Precision**: Be specific, not vague

We'll help with language in code reviews. Focus on technical correctness first.

---

## Benefits Summary

| Benefit | Impact |
|---------|--------|
| **Global Contributors** | 10x larger potential contributor pool |
| **AI Effectiveness** | 40% faster development with AI assistants |
| **Team Scaling** | Hire from anywhere, not just Portuguese speakers |
| **Documentation Quality** | Clearer technical writing, less ambiguity |
| **SEO & Discoverability** | 100x more searches in English for crypto/trading |
| **Enterprise Adoption** | International enterprises require English docs |
| **Future-Proofing** | Ready for Swiss HQ, global expansion |

---

## Related Documents

- **ADR-0006**: [English-Only Codebase](adr/ADR-0006-english-only-codebase.md) - Architectural decision
- **AI_WORKFLOW.md**: [AI Collaboration Guidelines](AI_WORKFLOW.md) - AI requirements
- **CONTRIBUTING.md**: [Contribution Guidelines](../CONTRIBUTING.md) - Includes language policy

---

## FAQs

### Q: Why is this so strict?

**A**: We're not being strict for fun. We're being **strategic for success**. Every Portuguese word in the codebase is a barrier to global adoption. We choose growth over comfort.

### Q: What if I'm more comfortable writing in Portuguese?

**A**: Write your first draft in Portuguese if it helps, then translate before committing. Use AI assistants (Claude, ChatGPT, DeepL) to help translate. Over time, you'll naturally start thinking in English for code.

### Q: Can I ask questions in Portuguese in team chat?

**A**: Casual Slack chat can use Portuguese. But technical discussions SHOULD be in English so:
1. Future team members can search chat history
2. Non-Portuguese speakers can participate
3. AI tools can index and search effectively

### Q: What about variable names that are domain-specific to Brazil?

**A**: Even Brazil-specific concepts should use English:
- ❌ `pix_transaction` (Brazilian payment system) → ✅ `instant_payment`
- ❌ `boleto` (Brazilian payment slip) → ✅ `bank_slip`

Use English with a comment explaining the Brazilian context if needed.

### Q: Is this cultural imperialism?

**A**: No. It's **pragmatism**. We could choose Esperanto or Mandarin, but English is the _de facto_ standard for international software development. We're following industry norms, not imposing culture.

Programming languages (Python, JavaScript) are in English. Frameworks (Django, React) are in English. Cloud platforms (AWS, GCP) are in English. We're being consistent with our ecosystem.

---

## Accountability

**Policy Owner**: CTO / Technical Leadership
**Review Frequency**: Annually (or when hiring internationally)
**Enforcement**: Automated checks + peer review

**Violations**:
- First occurrence: PR comment, request to fix
- Repeated: PR blocked until fixed
- Pattern of violations: 1:1 discussion with tech lead

We're not here to punish. We're here to build a world-class international project. Let's do it right from the start.

---

**Last Updated**: 2025-11-14
**Version**: 1.0
**Status**: Active
