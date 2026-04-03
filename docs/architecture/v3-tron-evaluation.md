# ROBSON v3 — TRON/TRC-20 EVALUATION & DECISION

**Date**: 2026-04-03  
**Status**: DECIDED — DEFER product integration, PURSUE funding  
**Classification**: Strategic / Regulatory

---

## Decision Summary

| Aspect | Decision |
|--------|----------|
| **Product integration in v3** | NO |
| **Architecture TRON-readiness** | YES (PaymentRail abstraction) |
| **$1B AI fund application** | YES (as funding, not product dependency) |
| **Zero Hash integration** | DEFER |
| **Timeline for reconsideration** | Q1 2027 at earliest |

---

## Context: TRON Ecosystem (2026)

### Relevant Developments

1. **TRON DAO $1B AI Agent Fund** (announced March 2026): Targets AI agent infrastructure, identity, stablecoin payments, and agentic finance. Specifically relevant to Robson's architecture as an AI risk management agent.

2. **TRC-20 USDT**: Sub-second confirmation, near-zero fees. Most-used stablecoin rail globally for transfers and remittances. Dominant chain for USDT by transaction volume.

3. **Zero Hash Integration** (announced 31 March 2026, Geneva): Enterprise-regulated access to TRX and USDT TRC-20 for custody, trading, liquidity, and settlement. Positioned as compliance-ready for European/Swiss-adjacent jurisdictions.

4. **Allora Integration**: On-chain AI predictions live on TRON. Demonstrates TRON's push into AI-finance convergence.

5. **Technical stack**: Solidity + TVM (EVM-compatible), TronWeb (JavaScript SDK), TronBox (development framework). Familiar tooling for any EVM developer.

---

## Regulatory Analysis: Zug/Baar/Switzerland

### Applicable Regulations

| Regulation | Status | Impact on Robson |
|-----------|--------|-----------------|
| **DLT Act (2021)** | Mature, operational | If Robson integrates TRON, tokens would be classifiable under existing DLT framework. No regulatory gap — but classification triggers compliance obligations. |
| **Stablecoin Regulation (2026)** | Active enforcement | 100% backing requirement + licensing for issuers. For USERS/integrators: depends on activity. Settlement through stablecoins may require FinTech or banking license depending on volume and third-party involvement. |
| **Crypto-AEOI** | Mandatory since Jan 2026 | Automatic exchange of information for crypto holdings. Already applies to Binance account. Adding TRON wallet would add reporting obligations. |
| **FINMA Licensing** | Depends on activity | Key question: Does operating stablecoin settlement rails transform Robson from "own-account risk tool" to "financial intermediary"? |

### FINMA Classification Risk

**Current classification (without TRON)**: Private risk management tool for operator's own account. No third-party assets, no custody, no advisory services. Falls OUTSIDE FINMA licensing requirements.

**Classification with TRON integration**: Depends on the role:

| TRON Role | FINMA Classification | License Required? |
|-----------|---------------------|-------------------|
| Treasury management (holding USDT for own account) | Still own-account | NO, but Crypto-AEOI reporting required |
| Settlement between exchanges (moving USDT between venues) | Possibly money transmission | POSSIBLY — requires legal opinion |
| Accepting TRON from others (fund management) | Financial intermediary | YES — FinTech license minimum |
| Stablecoin micro-transactions (paying for services) | Payment activity | POSSIBLY — depends on volume |
| Interacting with TRON smart contracts (DeFi) | Complex — contract-dependent | REQUIRES legal analysis per contract |

**Key risk**: Any interpretation that Robson is facilitating stablecoin transfers for others (even accidentally, via a future multi-user feature) could trigger FinTech licensing requirements. The licensing process costs CHF 30K+ and takes 6-12 months.

### Zero Hash Regulatory Status

Zero Hash positions itself as regulated infrastructure. However:
- Swiss regulatory approval: NOT confirmed as of April 2026
- Geneva announcement (March 2026) suggests intent but not completion
- Relying on Zero Hash's compliance does not protect Robson from its own FINMA obligations
- If Zero Hash does not obtain Swiss-compatible coverage, the integration path is blocked

---

## Technical Analysis

### Integration Effort

| Component | Effort | Complexity |
|-----------|--------|------------|
| TronWeb SDK integration | S | Low (npm package, well-documented) |
| Wallet management (key generation, signing) | M | Medium (key custody is a security-critical path) |
| Transaction monitoring (confirmations, finality) | M | Medium (different model from traditional exchanges) |
| Smart contract interaction (if needed) | L | High (Solidity development, audit requirements) |
| Zero Hash API integration | M | Medium (REST API, but terms/SLA unknown) |

**Total estimated effort**: 3-6 weeks of focused development.

### Architecture Impact

Adding TRON to Robson would require:

1. **New adapter**: `TronPaymentRail` implementing `PaymentRail` trait
2. **Key management**: Secure storage for TRON private keys (more sensitive than API keys — private keys control funds directly)
3. **Transaction monitoring**: Background process watching TRON chain for confirmations
4. **New failure modes**: Chain congestion, smart contract bugs, bridge failures
5. **New security surface**: Private key compromise = total loss of funds on TRON

---

## Decision Rationale

### Why NO for v3 Product Integration

1. **Regulatory risk exceeds benefit**: Adding TRON potentially transforms Robson's FINMA classification. The legal analysis required (CHF 5-10K for stablecoin-specific opinion) delays v3 launch with uncertain outcome. The benefit (stablecoin settlement) does not address a problem the operator currently has — Binance handles all settlement.

2. **Engineering distraction**: 3-6 weeks of TRON development is 3-6 weeks not spent on the Risk Engine, EventLog replay, and control surface — the components that determine whether v3 succeeds.

3. **Key custody risk**: Managing TRON private keys adds a catastrophic failure mode (key compromise = total fund loss) that does not exist in the current architecture (API keys can be rotated, have limited permissions, and are exchange-side custodied).

4. **Zero Hash dependency**: Building on Zero Hash before they have confirmed Swiss regulatory coverage creates a dependency on a timeline Robson does not control.

5. **No operator need**: The operator trades on Binance. Settlement is handled by Binance. There is no settlement problem in v3.

### Why YES for Architecture Readiness

The `PaymentRail` trait costs zero development time to define and creates optionality:

```rust
#[async_trait]
pub trait PaymentRail: Send + Sync {
    /// Transfer funds to a destination
    async fn transfer(
        &self,
        amount: Decimal,
        currency: &str,
        destination: &str,
    ) -> Result<TransferReceipt>;
    
    /// Check balance on this rail
    async fn balance(&self, currency: &str) -> Result<Decimal>;
    
    /// Check transfer status
    async fn status(&self, transfer_id: &str) -> Result<TransferStatus>;
    
    /// Rail identifier
    fn rail_id(&self) -> &str;
}
```

Future implementations:
- `BinanceSettlement`: Existing exchange transfers
- `TronTrc20Rail`: TRON USDT transfers (if/when adopted)
- `BankTransferRail`: Traditional wire (if ever needed)

### Why PURSUE $1B Fund as Funding

The TRON DAO $1B AI Agent Fund targets exactly Robson's category:
- AI agent infrastructure
- Financial AI applications
- Agentic commerce

Applying for a grant requires:
- Application materials describing Robson's architecture
- Mention of TRON-readiness (PaymentRail abstraction)
- No product integration required for application

**Risk**: Low. Application is a business activity, not an engineering dependency.  
**Potential upside**: Funding for development, infrastructure, and potential future TRON integration if regulatory conditions are met.  
**Action item**: Prepare grant application by Q3 2026.

---

## Trigger Conditions for Reconsideration

Reconsider TRON product integration when ALL of the following are true:

| # | Condition | How to Verify | Current Status |
|---|-----------|--------------|----------------|
| 1 | FINMA issues clear guidance on stablecoin integration for own-account trading tools | Monitor FINMA publications, consult legal counsel | NOT MET — guidance pending |
| 2 | Zero Hash obtains Swiss-compatible regulatory approval | Zero Hash announcement or FINMA register | NOT MET — Geneva announcement only |
| 3 | Operator has concrete use case for stablecoin settlement | Operator identifies a workflow that TRON improves | NOT MET — Binance handles settlement |
| 4 | v3 core system is stable and operational for >3 months | System running with zero critical incidents | NOT MET — v3 not yet launched |

**Earliest possible reconsideration**: Q1 2027 (assuming v3 launches Q3 2026 and runs stable for 3 months).

---

## Failure Mode Analysis

### If TRON Had Been Adopted and Then Failed

| Scenario | Impact | Mitigation (if adopted) | Actual Impact (not adopted) |
|----------|--------|------------------------|---------------------------|
| FINMA restricts TRC-20 stablecoins | Must remove integration, potentially face regulatory action | PaymentRail abstraction allows swap to another rail. But legal exposure from the operating period remains. | ZERO — not integrated |
| Zero Hash changes terms/shuts down | Lose regulated bridge to TRON | Must find alternative bridge or operate directly on-chain (higher regulatory risk) | ZERO — not integrated |
| TRON network congestion/outage | Settlement delays, stuck funds | Fallback to Binance settlement (PaymentRail abstraction) | ZERO — not integrated |
| Private key compromise | Total loss of TRON-custodied funds | Insurance? Multi-sig? Cold storage? All add complexity. | ZERO — not integrated |

### Opportunity Cost of Not Adopting

| Missed Opportunity | Likelihood | Impact |
|-------------------|-----------|--------|
| Early access to $1B fund via product integration | Low (fund accepts applications without product integration) | Low — pursuing fund separately |
| Competitive advantage in AI agent payments | Medium (if market moves to TRON for agent-to-agent payments) | Low for v3 — single operator, no agent-to-agent payments |
| Lower settlement costs vs bank wires | Low (operator uses Binance, not bank wires) | ZERO for v3 |

---

## Summary

TRON/TRC-20 is a strategically interesting ecosystem with real momentum in AI + finance convergence. However, for Robson v3 — a single-operator risk management tool operating from Baar/Zug — the regulatory risk, engineering distraction, and key custody complexity outweigh the benefits. The architecture is designed to accommodate TRON integration when conditions are met (PaymentRail abstraction), and the $1B AI fund is being pursued independently as a funding opportunity.

The correct engineering decision is to ship v3 with a working Risk Engine, reliable EventLog, and responsive control surface — then evaluate TRON integration from a position of operational stability rather than architectural aspiration.
