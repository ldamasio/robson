#!/bin/bash
# AI Governance Framework Validation Script
#
# Purpose: Validate that all required governance files exist and comply with policies
# Usage: bash .ai-agents/validate.sh OR make validate

set -e

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🔍 AI Governance Framework Validation"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Counter for errors
ERRORS=0

# ============================================
# 1. CHECK REQUIRED FILES
# ============================================
echo "📂 Checking required governance files..."
echo ""

REQUIRED_FILES=(
  ".ai-agents/ADR-0001-ai-governance.md"
  ".ai-agents/MODES.md"
  ".ai-agents/DECISION-MATRIX.md"
  ".ai-agents/AI-WORKFLOW.md"
  ".ai-agents/CONTEXT.md"
  ".cursorrules"
  "docs/requirements/README.md"
  "docs/specs/README.md"
  "docs/plan/README.md"
  ".gitignore"
)

for file in "${REQUIRED_FILES[@]}"; do
  if [ ! -f "$file" ]; then
    echo "   ❌ Missing: $file"
    ERRORS=$((ERRORS + 1))
  else
    echo "   ✅ Found: $file"
  fi
done

echo ""

# ============================================
# 2. CHECK OPTIONAL BUT RECOMMENDED FILES
# ============================================
echo "📋 Checking optional governance files..."
echo ""

OPTIONAL_FILES=(
  "docs/requirements/example-multi-timeframe-analysis.md"
  "docs/specs/example-multi-timeframe-analysis-spec.md"
  ".ai-agents/session-logs/.gitkeep"
)

for file in "${OPTIONAL_FILES[@]}"; do
  if [ ! -f "$file" ]; then
    echo "   ⚠️  Optional: $file (not found, but OK)"
  else
    echo "   ✅ Found: $file"
  fi
done

echo ""

# ============================================
# 3. ENFORCE ENGLISH-ONLY POLICY
# ============================================
echo "🌍 Checking English-only policy compliance..."
echo ""

# Portuguese keywords that should NOT appear in governance docs
PORTUGUESE_KEYWORDS=(
  "função"
  "tarefa"
  "especificação"
  "requisito"
  "implementação"
  "arquivo"
  "desenvolvedor"
  "usuário"
  "código"
  "projeto"
  "sistema"
  "aplicação"
  "serviço"
  "modelo"
  "classe"
  "método"
  "variável"
  "retorna"
  "execução"
  "configuração"
  "padrão"
  "exemplo"
  "descrição"
  "documentação"
)

VIOLATIONS=0

# Check in .ai-agents/ directory
for keyword in "${PORTUGUESE_KEYWORDS[@]}"; do
  MATCHES=$(grep -riw "$keyword" .ai-agents/ 2>/dev/null | grep -v "validate.sh" | wc -l)
  if [ "$MATCHES" -gt 0 ]; then
    echo "   ❌ Found '$keyword' in .ai-agents/ ($MATCHES occurrences)"
    VIOLATIONS=$((VIOLATIONS + MATCHES))
    ERRORS=$((ERRORS + 1))
  fi
done

# Check in docs/requirements/ directory
for keyword in "${PORTUGUESE_KEYWORDS[@]}"; do
  MATCHES=$(grep -riw "$keyword" docs/requirements/ 2>/dev/null | wc -l)
  if [ "$MATCHES" -gt 0 ]; then
    echo "   ❌ Found '$keyword' in docs/requirements/ ($MATCHES occurrences)"
    VIOLATIONS=$((VIOLATIONS + MATCHES))
    ERRORS=$((ERRORS + 1))
  fi
done

# Check in docs/specs/ directory
for keyword in "${PORTUGUESE_KEYWORDS[@]}"; do
  MATCHES=$(grep -riw "$keyword" docs/specs/ 2>/dev/null | wc -l)
  if [ "$MATCHES" -gt 0 ]; then
    echo "   ❌ Found '$keyword' in docs/specs/ ($MATCHES occurrences)"
    VIOLATIONS=$((VIOLATIONS + MATCHES))
    ERRORS=$((ERRORS + 1))
  fi
done

# Check in docs/plan/ directory
for keyword in "${PORTUGUESE_KEYWORDS[@]}"; do
  MATCHES=$(grep -riw "$keyword" docs/plan/ 2>/dev/null | wc -l)
  if [ "$MATCHES" -gt 0 ]; then
    echo "   ❌ Found '$keyword' in docs/plan/ ($MATCHES occurrences)"
    VIOLATIONS=$((VIOLATIONS + MATCHES))
    ERRORS=$((ERRORS + 1))
  fi
done

if [ "$VIOLATIONS" -eq 0 ]; then
  echo "   ✅ Language policy: PASSED (English-only)"
else
  echo ""
  echo "   ❌ Language policy: FAILED ($VIOLATIONS violations found)"
  echo "   💡 Tip: All governance documents must be in English"
  echo "   📖 See: docs/LANGUAGE-POLICY.md for rationale"
fi

echo ""

# ============================================
# 4. CHECK GITIGNORE FOR SESSION LOGS
# ============================================
echo "🚫 Checking .gitignore for session logs exclusion..."
echo ""

if grep -q ".ai-agents/session-logs/\*" .gitignore 2>/dev/null; then
  echo "   ✅ Session logs are gitignored"
elif grep -q ".ai-agents/session-logs/" .gitignore 2>/dev/null; then
  echo "   ✅ Session logs are gitignored"
else
  echo "   ❌ Session logs NOT in .gitignore"
  echo "   💡 Add: .ai-agents/session-logs/* to .gitignore"
  ERRORS=$((ERRORS + 1))
fi

echo ""

# ============================================
# 5. CHECK FOR BROKEN CROSS-REFERENCES
# ============================================
echo "🔗 Checking for broken cross-references..."
echo ""

# Check that .cursorrules references .ai-agents/ files
if grep -q ".ai-agents/MODES.md" .cursorrules 2>/dev/null; then
  echo "   ✅ .cursorrules references .ai-agents/MODES.md"
else
  echo "   ⚠️  .cursorrules doesn't reference MODES.md (recommended)"
fi

# Check that ADR-0001 references other governance files
if grep -q "MODES.md" .ai-agents/ADR-0001-ai-governance.md 2>/dev/null; then
  echo "   ✅ ADR-0001 references MODES.md"
else
  echo "   ❌ ADR-0001 missing reference to MODES.md"
  ERRORS=$((ERRORS + 1))
fi

echo ""

# ============================================
# 6. SUMMARY
# ============================================
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "📊 Validation Summary"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

if [ $ERRORS -eq 0 ]; then
  echo "   ✅ VALIDATION PASSED"
  echo ""
  echo "   All required files are present."
  echo "   English-only policy is enforced."
  echo "   Cross-references are valid."
  echo ""
  echo "   🎉 AI Governance Framework is production-ready!"
  echo ""
  exit 0
else
  echo "   ❌ VALIDATION FAILED"
  echo ""
  echo "   Errors found: $ERRORS"
  echo ""
  echo "   Please fix the issues above and re-run validation."
  echo ""
  exit 1
fi
