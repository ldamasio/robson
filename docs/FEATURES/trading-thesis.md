# Trading Thesis Feature - v1 Chat Interface

**Status**: Implemented (Backend Complete, Pending Import Path Fix)

**Created**: 2025-01-17

## Overview

Trading Thesis is a structured market hypothesis feature for Robson v1 Chat. It allows users to document and track their market observations without executing trades.

## Core Philosophy

**Thesis = "I think X might happen" (observation)**
**Strategy = "Execute order when Y occurs" (automation)**

A thesis can exist alone forever as a learning diary. Not every thesis should become a strategy.

## The 4 Required Elements

Every Trading Thesis must contain:

1. **Market Context**: What is happening NOW?
2. **Rationale**: WHY might this opportunity exist?
3. **Expected Trigger**: WHAT needs to happen to confirm?
4. **Invalidation**: WHAT proves the thesis is wrong?

## Thesis Lifecycle

```
DRAFT → ACTIVE → VALIDATED (trigger occurred)
               ↓
              REJECTED (invalidation occurred)

DRAFT → ACTIVE → EXPIRED (time-based)
               ↓
             CONVERTED (became executable strategy)
```

## API Endpoints

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/thesis/templates/` | Get pre-defined thesis templates |
| POST | `/api/thesis/create/` | Create new thesis |
| GET | `/api/thesis/` | List user's theses |
| GET | `/api/thesis/summary/` | Get thesis statistics |
| GET | `/api/thesis/{id}/` | Get specific thesis |
| POST | `/api/thesis/{id}/status/` | Update thesis status |

## Files Created

### Domain Layer (Hexagonal Architecture)
- `apps/backend/core/domain/thesis.py`
  - `TradingThesis` dataclass (frozen, immutable)
  - `ThesisStatus` enum
  - `ThesisTemplate` for common patterns
  - Pre-defined templates: breakout, mean_reversion, trend_following

### Django Models
- `apps/backend/monolith/api/models/thesis.py`
  - `TradingThesisModel` with validation
  - State transition methods
  - Domain entity conversion

### API Views
- `apps/backend/monolith/api/views/thesis_views.py`
  - CRUD operations for theses
  - Status updates
  - Summary statistics

### Database
- `api/migrations/0031_add_trading_thesis_model.py`

### Chat System
- Updated `apps/backend/core/domain/conversation.py`
  - Enhanced system prompt for thesis coaching behavior
- Updated `apps/backend/core/application/use_cases/chat_with_robson.py`
  - Added `THESIS` intent type
  - Thesis detection logic

## Chat Behavior

When user mentions market observations, AI acts as "thinking coach":

1. Suggests structuring as Trading Thesis
2. Guides through 4 elements with questions
3. Validates clarity and coherence
4. Offers to save to Thesis Journal
5. Asks if observational or for trading
6. NEVER pushes to execute

## Sample Chat Flow

```
User: I think BTC will break out soon.

Robson: Would you like me to help you structure this as a Trading Thesis?

User: Sure.

Robson: Let's capture your hypothesis. I need:
1. Market Context: What timeframe? What's happening now?
2. Rationale: Why might this work?
3. Trigger: What confirms it?
4. Invalidation: What proves you're wrong?

[User provides details]

Robson: Great! I've saved this to your Thesis Journal as ACTIVE.
I'll monitor for the trigger conditions. This is observational -
not every thesis needs a trade.
```

## Known Issues

### Import Path Configuration

The thesis and chat views are temporarily disabled in URL configuration due to Python path issues with `core.domain` imports.

**Affected Files**:
- `apps/backend/monolith/api/urls/__init__.py` (THESIS_VIEWS_AVAILABLE = False)
- Chat views also affected

**Root Cause**: The `core` module import path configuration happens after URL imports.

**Workaround**: Views are implemented and ready. Import path needs to be fixed in Django initialization.

## Next Steps

1. Fix Python path configuration for `core.domain` imports
2. Enable thesis views in URL configuration
3. Run migrations: `python manage.py migrate`
4. Add frontend UI components (optional)
5. Test chat flow end-to-end

## Design Principles

- KISS: Keep It Simple, Stupid
- Zero execution coupling
- Optional conversion to strategy
- Chat-first creation (no forms)
- Natural language over code
- Learning diary over trading bot

## References

- ADR-0007: Trading Thesis vs Strategy
- docs/AGENTS.md: AI assistant context
- docs/architecture/TRANSACTION-HIERARCHY.md: How thesis relates to operations
