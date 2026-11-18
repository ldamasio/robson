# Migration38 public execution: provenance and maintenance

Source checkpoint only, not PG/timing/catalog/CODE or activation acceptance. The operator-authorized successor changes whole52 aggregate40ms to40ms per command and3→54 operations under transaction locks,7→58 total. Global200ms/lock20ms/idle20ms/PG12 remain binding. Original migration38 bytes, version/type/description/checksum,136constraints,36/37 and upstream applied-history behavior remain unchanged. No fork, dependency, generic SQL parser/cache, advisory CRC copy or new public authority.

The production sequence is install1, public BEGIN1,52 fixed borrowed original DDL slices, original metadata INSERT1 inside that transaction, public COMMIT1, elapsed UPDATE1 after acknowledged COMMIT, exact old-settings restore1. Each new query explicitly uses persistent(false).58 counts executed SQL/application operations, not measured wire/protocol/cold-cache/RI/internal/housekeeping counts. Original outcomes and OwnedApply raw retirement remain; DelegatedApply includes elapsed return. Other migrations and historical repair stay upstream. Private synthetic cfg(test) contracts retain their old7operation source scope and prove nothing about production58.

The only compatibility SQL owned here is INSERT(version,description,successTRUE,original checksum,execution_time=-1) and the postcommit UPDATE(execution_time,version), copied from pinned sqlx-postgres0.8.6 src/migrate.rs:execute_migration/apply. installed_on keeps the upstream default; bindings/order/error mapping and elapsed BEGIN-through-COMMIT semantics are retained. No rewritten history or empty executed Migration trick. The public Migration::new checksum calculation is used ONLY on borrowed static originals during preflight, never executed. SQLx's source allocates one48-byte checksum Vec per temporary;52 temporary checks sequentially cover62406original command bytes (full identity covers64577bytes) with no retained SQL copy. The52-reference array stores832bytes of string references on64-bit; no runtime statement discovery.

Pinned upstream source hashes:

- sqlx-postgres0.8.6/src/migrate.rs SHA256 5e958e86890da58bbf7d7a9cc284256d1bb1e24ec58d58cdb47927ae3643508c
- sqlx-core0.8.6/src/migrate/migration.rs SHA256 24456994dbab5fa2421222ab7cddb4343d122d4ff38d6839f097a4d35c54f515
- Upstream Cargo license: MIT OR Apache-2.0; repository https://github.com/launchbadge/sqlx

Every SQLx bump requires review of these two SQL literals, metadata schema/default/bind order, error/timing/sentinel behavior, public transaction/drop/drain/run_direct/checksum APIs and applied-history compatibility tests. The app does not copy wire execution, transaction manager or lock-ID algorithm. Source range/byte proofs do not establish semantic equivalence; exact schema/trigger/legacy/timer/atomicity/retirement and real4+3journal coexistence on PG12/16 remain required. No source success closes the original pg04 or D1 RED.

Private cfg(test) stages Install/Begin/Ddl1..52/Metadata/Commit/Elapsed/Restore and BeforeCall/AfterReturn are app observations only. The shared Arc<Mutex<Option<Gate>>> has one armed slot plus at most one hook removed from it and awaiting release (transient total2, not all handles1). Arm-next is allowed only after observing current reached and before its release; an occupied slot refuses overwrite. The slot lock is released before await. This permits four fixed windows during the same borrowed apply, without a queue/payload/history/Ready/Known witness. Dropping observer/release fails closed. Actual four-window inventory remains unexecuted. ROOT's separately composed acceptance descendant lives under robsond/tests/bounded_migration; those files are absent in this private production child until composition. No standalone postgres-libtest compilation is claimed here.

The following MIT notice accompanies the borrowed metadata SQL:

Copyright (c) 2020 LaunchBadge, LLC

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
