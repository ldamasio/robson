# ADR-0056 step 8i-1: unqualified liability facts

The `robson_exec::balance_liability` module implements pure calculations over
supplied ledger values. It has no runtime caller, persistence or send authority.
The spot factory and legacy balance batch refusal remain unchanged.

`partition_facts` accepts recorded Q, effective B, X and consumed quantity.
It derives N=Q−B, D=N−X and either G=X−consumed or an explicit deficit with
excess=consumed−X. In a deficit there is no G. Inputs and all intermediate
results must be nonnegative where required and exactly representable. It
checks Q=B+D+X without rounding or saturation. The supplied fee-evidence enum
describes provisional/final B; it does not prove completeness or settlement.

X is not chosen here. Initial lot flooring, provisional Bhat=Q*f_L, final-fee
reduction of X, complete O(L), R1/R3, write-off envelopes and settlement remain
later work. No caller can use these facts as a certificate or send ticket.

`order_facts` compares a borrowed StoredOrder with an expected canonical
OrderKey and governed OrderOwner. It checks quantities and derives the exact
remaining amount. Cancel-renamed client IDs do not replace the canonical key.
Tenant authentication and evidence provenance remain the store/caller's job.
The expected values are themselves untrusted data.

`matches_protection_shape` checks SELL, exact STOP_LOSS, remaining quantity and
positive trigger at or above the target. **Shape is not protection.** The result
contains no lifecycle state; a terminal, partial or unknown record can have the
same geometry. Currentness, outstanding cancellations and complete sibling
evidence are required by the later R3 evaluator. Expiry reasons decide nothing.

Transition vocabulary contains Replace, Reprotect, Flatten and Exit. Strength
is Exit > Flatten > {Replace,Reprotect}; equal rank makes no merge choice.
Positive target values join by maximum. Neither operation writes a transition,
records progress, validates chart provenance or closes a leg.

| Operation, every outcome | Work |
|---|---|
| Partition | At most5 existing exact add/sub operations; fixed native values |
| Local order | One exact subtraction and fixed comparisons; identity comparison O(input bytes), no copied payload or retained borrow |
| Target/strength | Fixed comparisons, no arithmetic allocation |
| All three |0 SQL/internal SQL/RI/HTTP/await/locks/tasks/timers/persistent writes |

There are no caches, queues or listeners to retain memory. Database/feed/process
failure creates no effect here; callers receive explicit invalid-quantity,
identity, target or Unrepresentable errors. A deficit remains visible data.
This does not certify runtime availability, concurrent ownership or durable
recovery. Existing security/dependency and workspace gates still apply.

Tradeoff: these small facts can be checked independently, but intentionally
cannot run an owner. The full 8i composition remains blocked on accepted 8g/8h
interfaces, complete order-set materialization, transition/journal atomicity,
certified basis, event-delivery generation and complete measured budgets.
Admission35 and owner25/200 are not increased; no schema or factory activation
follows. All15 activation prerequisites and SD15/SD16/T6/operator-only decisions
remain separate.


## Executable quantity candidates

The first calculation derives executable quantity from recorded gross and
base commission on the persisted positive lot step. A provisional bound
uses the supplied persisted rate as a dimensionless fraction, with exact
multiplication and native decimal export. The final-fee calculation uses
the smaller of the prior executable quantity and the new exact lot floor.
If final commission falls, the prior quantity stays fixed and the excess
becomes dust. Both paths reuse the same partition and report a deficit
without a remainder when prior consumption exceeds executable quantity.

The quantity candidates take supplied values. The later owner must bind the
same immutable lot step and fee source to the durable leg and coherent order
evidence before persisting any change. These functions use the private
sizing arithmetic frame with at most five fixed imports and six charged
calls. They issue zero SQL and venue requests and hold no locks.
