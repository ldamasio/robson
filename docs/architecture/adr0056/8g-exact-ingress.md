# ADR-0056 step 8g: exact spot ingress

This slice implements original-token validation for the spot inputs listed below.
It does not implement cost selection, receipt age authority, sizing, refresh workers,
or production activation. The spot factory remains disabled and execution costs
remain unavailable until their reviewed later steps.

`robson_exec::balance_costs::exact::ExactDecimal` accepts at most 128 bytes of plain
signed decimal text. It removes only trailing fractional zeros and constructs the
96-bit coefficient / scale-at-most-28 value directly. It rejects exponent syntax,
numeric JSON tokens, nonfinite values, malformed signs, overflow and any nonzero
digit that would otherwise be discarded. The spot deserializer also rejects
negative amounts. Mathematical negative zero is zero. Domain validations such as
positive ticker price and positive lot step remain at their existing boundaries.

## Original-input inventory

| Raw input | Exact boundary | Existing consumer |
|---|---|---|
| Account `free`, `locked` strings | `spot/records.rs::SpotAccountBalance` | Spot account/cash/holding projection |
| Standard/tax/special maker/taker/buyer/seller strings and discount value | `spot/records.rs::SpotCommissionRates`, `SpotCommissionDiscount` | Typed commission reader; interpretation is a later slice |
| PRICE_FILTER, LOT_SIZE, MARKET_LOT_SIZE and NOTIONAL amounts | `spot/rules.rs` filter structs | Validated complete rules/cache; integer filter counts stay integers |
| Ticker `price` | `spot/rest.rs::TickerPrice` | Required positive reference price |
| Aggregate-trade `p` string | `spot/market_data.rs::decode_message` | Spot feed price; invalid input yields existing Malformed outcome |
| Candle OHLC and volume strings | `spot/ohlcv.rs::parse_spot_klines` | Spot chart candles; at most the existing 1,000-row request limit |

The existing Value-based rules/candle decoding preserves JSON strings; it never
converts a Number into an accepted decimal. The scalar parser itself allocates
nothing. This is not a new bound on transport body bytes or serde's allocations:
those existing transport/JSON paths are unchanged. The candle result is bounded
by 1,000 rows after JSON decoding. No queue, cache or detached task is introduced.

The shared futures candle parser and USD-M/global Decimal serde behavior are
unchanged. Differential fixtures verify equal ordinary candle results; a discarded
scale-29 digit is explicitly refused only by the spot parser. Spot order/trade and
placement evidence parsers are outside this slice and retain their behavior. This
is not a claim that every monetary ingress is exact. Injected override/gap/margin/
notional configuration and cost interpretation belong to the following terms slice;
the legacy daemon environment configuration is untouched.

## Failure and work bounds

| Condition | Result |
|---|---|
| Malformed, numeric, negative or inexact spot amount | Existing typed parse/Malformed failure; no rounded value published |
| Exact native maximum or harmless trailing zeros | Preserved exactly; later domain checks still apply |
| HTTP/feed unavailable | Existing connector failure/reconnect behavior, unchanged |
| Process stops | No added state, worker, persistence or recovery obligation |
| Database unavailable | No database operation in this slice |

Each scalar parse performs at most 128 bytes of validation and bounded coefficient
arithmetic. Parsing adds **zero HTTP and zero SQL**. Existing reader request counts
are unchanged; the ticker regression observes exactly one physical local request
for both accepted and refused answers. Existing connector wire tests cover the
other readers. There is no asynchronous state transition in the parser.

The tradeoff is deliberate strictness: a venue token that requires rounding or
uses a numeric JSON type now refuses at the spot boundary. This prevents lost fee,
cash or grid evidence before later risk calculations. It does not repair or alter
the legacy USD-M rounding contract. New property tests cover all native Decimal
coefficient/sign/scale combinations with exact trailing-zero normalization; field
matrix regressions verify every listed decimal boundary and shared-path preservation.
