# Release Policy and v2.6 Development Snapshots

Robson publishes technical increments under the existing review, CI and merge gates. Technical publication and permission to operate with capital are separate. See the [public roadmap](../ROADMAP.md) for implementation status and activation gates.

## Read version labels carefully

The semantic Git tag [`v2.5.0`](https://github.com/ldamasio/robson/tree/v2.5.0) identifies commit `5c7b39c59b112726516cbda042dcf323dc31d48c`. Historical GitHub metadata also includes a release titled `v2.5.0` whose actual tag is [`Latest`](https://github.com/ldamasio/robson/releases/tag/Latest), pointing to older code. A release title, a Git tag, a source commit and a deployed image are distinct identities. Check the exact tag and commit rather than inferring the contents from a title. The inventory used here (2026-11-15) found no GitHub Release attached to the semantic `v2.5.0` tag. This documentation does not rewrite that history.

The current Cargo and frontend package versions remain **2.5.0**. The internal `v3-*` architecture names remain historical planning identifiers. Neither is evidence that a Spot-first product release has shipped.

## Release tracks

| Track | Meaning and evidence |
| --- | --- |
| `2.5.x` | Compatible maintenance fixes for the existing product line, accepted through the applicable review and CI gates. No implied Spot activation. |
| `v2.6.0-alpha.N` | Development source snapshots of the Spot-first transition. Notes identify implemented components, incomplete paths, the exact source and scoped validation. `v2.6.0-alpha.1` is planned; this document does not create or publish it. |
| `v2.6.0-beta.N` | Integrated Spot feature scope under evaluation. Publish this label when the corresponding integration and validation criteria are demonstrated, stating the tested scope and remaining acceptance work. |
| `v2.6.0-rc.N` | A candidate for the final release with the corresponding release checks demonstrated, including compatibility and recovery/rollback evidence. Notes identify any remaining applicable gate; the label does not waive one. |
| `v2.6.0` | The integrated Spot-first product release after its applicable implementation, acceptance and release gates. Notes explicitly state activation status; operational activation remains a separate reserved operator action. |

Technical version increments and publication follow the standing authorization and existing gates; they do not require a new business decision for every source snapshot. A prerelease label does not bypass review, CI or merge requirements, and it does not authorize account cutover, capital allocation or trading.

## Development snapshot versus embedded versions

The first development source snapshot will identify its exact source commit while inherited Cargo and frontend package metadata still reads `2.5.0`. Release notes must disclose that distinction explicitly rather than presenting it as a fully versioned Spot runtime. Updating embedded versions or publishing binary/container artifacts is separate implementation work with its own validation; no alpha build or image is claimed by this documentation.

A development snapshot is not ready for real capital. Publication can document unfinished work transparently without claiming operational readiness. Operator activation, capital decisions and the venue's reserved risk/cutover decisions remain separate from technical releases.

## Required release-note transparency

- Identify the exact source tag/commit and, if artifacts are actually built, their immutable identities and matching source.
- Distinguish implemented components, integrated acceptance and operational observation; list incomplete paths and known limitations.
- Identify test and CI results that apply to that source, plus compatibility and rollback requirements.
- State whether operational activation has occurred. A published release alone is not authorization to operate.

Keep credentials, private paths and internal incident details out of public release notes. Publish evidence relevant to users and contributors; do not replace missing evidence with readiness claims.
