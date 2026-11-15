# Release Policy and v2.6 Development Snapshots

Robson publishes technical increments under the existing review, CI and merge gates. Technical publication and permission to operate with capital are separate. See the [public roadmap](../ROADMAP.md) for implementation status and activation gates.

## Read version labels carefully

The verified historical product tag [`v2.5.0`](https://github.com/ldamasio/robson/tree/v2.5.0) identifies commit `5c7b39c59b112726516cbda042dcf323dc31d48c`: [PR #59](https://github.com/ldamasio/robson/pull/59), merged on 2026-05-09, covering I3 reconciliation of Robson-Active positions missing on the exchange. At that source, the Rust subworkspace was `v3/Cargo.toml` with version `2.0.0-alpha`; the product tag did not imply manifests named `2.5.0`.

GitHub release ID `319957573`, titled `v2.5.0`, was incorrectly associated with the literal [`Latest` tag](https://github.com/ldamasio/robson/tree/Latest) at older 2023 source. The corrected association uses the existing semantic `v2.5.0` tag; **neither tag was moved**. The [complete original release notes and their hash](releases/archive/v2.5.0-original-github-release.md) are preserved as a public audit archive. The reassociation was completed and verified on 2026-11-15 at 19:31:17 UTC: [release ID `319957573`](https://github.com/ldamasio/robson/releases/tag/v2.5.0) now resolves to the semantic tag and May source, with the original 2026-05-09 publication timestamp preserved. The original notes are also attached to that release as an audit asset; their historical claims are not current acceptance evidence. Metadata correction does not rebuild, retest or newly accept historical production behavior.

The changelog now separates the verified May baseline from later development that had been grouped under an incorrect 2026-08-23 `2.5.0` heading. Original later entries remain recorded as unreleased development; they are not backdated into the May tag.

## Current transition identity

This development candidate consistently uses **`2.6.0-alpha.1`** in workspace metadata, all local Cargo.lock packages and the frontend package. It is not a published prerelease or a built binary. There is no exception allowing a new `v2.6` tag to identify source whose packages still claim `2.5.0`.

The transition progresses through `2.6.0-alpha.N` → `2.6.0-beta.N` → `2.6.0-rc.N` → `2.6.0`; `2.5.x` is reserved for compatible maintenance. Historical internal `v3-*` architecture names are retained and are not product release numbers.

## Release tracks

| Track | Meaning and evidence |
| --- | --- |
| `2.5.x` | Compatible maintenance fixes for the existing product line, accepted through the applicable review and CI gates. No implied Spot activation. |
| `v2.6.0-alpha.N` | Development source snapshots of the Spot-first transition. Notes identify implemented components, incomplete paths, the exact source and scoped validation. `v2.6.0-alpha.1` is planned; this document does not create or publish it. |
| `v2.6.0-beta.N` | Integrated Spot feature scope under evaluation. Publish this label when the corresponding integration and validation criteria are demonstrated, stating the tested scope and remaining acceptance work. |
| `v2.6.0-rc.N` | A candidate for the final release with the corresponding release checks demonstrated, including compatibility and recovery/rollback evidence. Notes identify any remaining applicable gate; the label does not waive one. |
| `v2.6.0` | The integrated Spot-first product release after its applicable implementation, acceptance and release gates. Notes explicitly state activation status; operational activation remains a separate reserved operator action. |

Technical version increments and publication follow the standing authorization and existing gates; they do not require a new business decision for every source snapshot. A prerelease label does not bypass review, CI or merge requirements, and it does not authorize account cutover, capital allocation or trading.

## Validate identity before publication

Run the stdlib-only checker (Python 3.11+) without building or resolving dependencies:

```bash
python3 scripts/check-release-identity.py
python3 scripts/check-release-identity.py --tag v2.6.0-alpha.1
python3 -m unittest discover -s scripts/tests -p test_release_identity.py
```

It checks strict [SemVer 2.0.0](https://semver.org/spec/v2.0.0.html), workspace/local lock/frontend agreement and, when supplied, the exact `v`-prefixed tag. Numeric identifiers cannot have leading zeroes: `2.5.014` is invalid while `2.5.14` is valid. The pnpm lock records dependency resolution but has no project-version field; it is not rewritten for this version change.

The `Release Identity` workflow validates pull requests, main and version tags with read-only repository permissions. It publishes no release or image and triggers no deployment itself; existing promotion workflows are unchanged. A version label identifies source, not a tested artifact or permission to use capital. Publish only the reviewed candidate's exact source, not an earlier head.

A development snapshot is not ready for real capital. Operational activation, capital decisions and reserved venue-risk/cutover decisions remain separate from technical releases.

## Required release-note transparency

- Identify the exact source tag/commit and, if artifacts are actually built, their immutable identities and matching source.
- Distinguish implemented components, integrated acceptance and operational observation; list incomplete paths and known limitations.
- Identify test and CI results that apply to that source, plus compatibility and rollback requirements.
- State whether operational activation has occurred. A published release alone is not authorization to operate.

Keep credentials, private paths and internal incident details out of public release notes. Publish evidence relevant to users and contributors; do not replace missing evidence with readiness claims.
