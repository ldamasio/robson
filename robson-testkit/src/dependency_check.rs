//! Proof that only dev-dependency edges reach `robson-testkit` (ADR-0056
//! step 8, 8d plan §5.4), read from `cargo metadata --format-version 1`.
//!
//! Every package's **declared** dependencies are read, so an optional edge no
//! feature activates and an edge under any target (`[target.'cfg(..)'.
//! dependencies]`) count; the dependency is matched by the package it names,
//! never by its alias (`kit = { package = "robson-testkit" }`). Any edge that
//! is not `dev` (a normal or a build edge, optional or not) is a violation.
//! The **resolved** graph is read too: an edge to `robson-testkit`'s package
//! id with any kind other than `dev` is a violation.
//!
//! **Input that proves nothing is refused:** empty input, input that is not
//! JSON, a `version` other than 1, no `packages`, `workspace_members` or
//! `resolve`, an incomplete graph (a workspace member missing from
//! `packages` or from `resolve.nodes`, no nodes, a node or an edge naming a
//! package `packages` does not list, an edge to a package with no resolved
//! node, an edge to `robson-testkit` with no
//! `dep_kinds`, a declared dev edge the resolved graph does not show),
//! `robson-testkit` not a workspace member, or no workspace package reaching
//! it by a `dev` edge (the real workspace has two, `robsond` and
//! `robson-projector`). So a failed or truncated `cargo metadata` can never
//! pass as "no violation".

use std::collections::BTreeSet;

use serde_json::Value;

/// The crate only dev-dependency edges may reach.
pub const TESTKIT: &str = "robson-testkit";

/// What a passing check saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    /// Workspace packages that reach `robson-testkit` by a `dev` edge.
    pub dev_dependents: Vec<String>,
}

fn field<'a>(value: &'a Value, name: &str) -> Result<&'a Value, String> {
    value.get(name).filter(|v| !v.is_null()).ok_or_else(|| format!("no `{name}`"))
}

fn array<'a>(value: &'a Value, name: &str) -> Result<&'a Vec<Value>, String> {
    field(value, name)?
        .as_array()
        .ok_or_else(|| format!("`{name}` is not an array"))
}

fn text<'a>(value: &'a Value, name: &str) -> Result<&'a str, String> {
    field(value, name)?.as_str().ok_or_else(|| format!("`{name}` is not a string"))
}

/// An edge's kind: `null` is a normal edge.
fn kind(value: &Value) -> Result<&str, String> {
    match value.get("kind") {
        None | Some(Value::Null) => Ok("normal"),
        Some(Value::String(kind)) => Ok(kind),
        Some(other) => Err(format!("an edge kind {other}")),
    }
}

/// Check `metadata`, the output of `cargo metadata --format-version 1`.
///
/// # Errors
/// Every violation, or why the input proves nothing.
pub fn check(metadata: &str) -> Result<Summary, Vec<String>> {
    if metadata.trim().is_empty() {
        return Err(vec!["empty input: cargo metadata printed nothing".to_string()]);
    }
    let metadata: Value = serde_json::from_str(metadata)
        .map_err(|error| vec![format!("input is not cargo metadata's JSON: {error}")])?;
    shape(&metadata).map_err(|why| vec![format!("input is not cargo metadata: {why}")])?;
    references(&metadata).map_err(|why| vec![format!("incomplete metadata: {why}")])?;
    let mut violations = Vec::new();
    let mut dev_dependents = Vec::new();
    let mut declared_dev = BTreeSet::new();
    let members = array(&metadata, "workspace_members").map_err(|e| vec![e])?;
    let member = |id: &str| members.iter().any(|m| m.as_str() == Some(id));
    let mut testkit = None;
    for package in array(&metadata, "packages").map_err(|e| vec![e])? {
        let (name, id) = match (text(package, "name"), text(package, "id")) {
            (Ok(name), Ok(id)) => (name, id),
            _ => return Err(vec!["a package without a name or an id".to_string()]),
        };
        if name == TESTKIT && member(id) {
            testkit = Some(id);
        }
        let dependencies =
            array(package, "dependencies").map_err(|e| vec![format!("{name}: {e}")])?;
        for dependency in dependencies {
            if text(dependency, "name").map_err(|e| vec![format!("{name}: {e}")])? != TESTKIT {
                continue;
            }
            match kind(dependency).map_err(|e| vec![format!("{name}: {e}")])? {
                "dev" if member(id) => {
                    dev_dependents.push(name.to_string());
                    declared_dev.insert(id);
                },
                "dev" => {},
                kind => violations.push(describe(name, kind, dependency)),
            }
        }
    }
    let Some(testkit) = testkit else {
        return Err(vec![format!(
            "{TESTKIT} is not a workspace member: not this workspace"
        )]);
    };
    let resolved_dev = resolved(&metadata, testkit, &mut violations)
        .map_err(|why| vec![format!("incomplete metadata: {why}")])?;
    if let Some(missing) = declared_dev.iter().find(|id| !resolved_dev.contains(**id)) {
        return Err(vec![format!(
            "incomplete metadata: the declared dev edge from {missing} to {TESTKIT} is not in the \
             resolved graph"
        )]);
    }
    if !violations.is_empty() {
        return Err(violations);
    }
    if dev_dependents.is_empty() {
        return Err(vec![format!(
            "no workspace package reaches {TESTKIT} by a dev edge: not this workspace's metadata"
        )]);
    }
    dev_dependents.sort();
    dev_dependents.dedup();
    Ok(Summary { dev_dependents })
}

/// The fields every `cargo metadata --format-version 1` output has.
fn shape(metadata: &Value) -> Result<(), String> {
    match field(metadata, "version")?.as_u64() {
        Some(1) => {},
        other => return Err(format!("version {other:?}, not 1")),
    }
    array(metadata, "packages")?;
    array(metadata, "workspace_members")?;
    array(field(metadata, "resolve")?, "nodes")?;
    Ok(())
}

fn describe(package: &str, kind: &str, dependency: &Value) -> String {
    let mut how = format!("{package} reaches {TESTKIT} by a {kind} edge");
    if let Some(alias) = dependency.get("rename").and_then(Value::as_str) {
        how.push_str(&format!(", renamed `{alias}`"));
    }
    if let Some(target) = dependency.get("target").and_then(Value::as_str) {
        how.push_str(&format!(", under target `{target}`"));
    }
    if dependency.get("optional").and_then(Value::as_bool) == Some(true) {
        how.push_str(", optional");
    }
    how
}

/// The graph's references hold together: every workspace member is a listed
/// package and a resolved node; there are nodes; every node names a listed
/// package, and every edge a listed package that has a node.
fn references(metadata: &Value) -> Result<(), String> {
    let mut packages = BTreeSet::new();
    for package in array(metadata, "packages")? {
        packages.insert(text(package, "id")?);
    }
    let nodes = array(field(metadata, "resolve")?, "nodes")?;
    if nodes.is_empty() {
        return Err("`resolve.nodes` is empty".to_string());
    }
    let mut resolved = BTreeSet::new();
    for node in nodes {
        let id = text(node, "id")?;
        if !packages.contains(id) {
            return Err(format!("resolved node {id} is not in `packages`"));
        }
        resolved.insert(id);
    }
    // Every node is known before any edge is read, so nodes may come in
    // any order.
    for node in nodes {
        let id = text(node, "id")?;
        for dep in array(node, "deps")? {
            let pkg = text(dep, "pkg")?;
            if !packages.contains(pkg) {
                return Err(format!("{id} has an edge to {pkg}, which is not in `packages`"));
            }
            if !resolved.contains(pkg) {
                return Err(format!("{id} has an edge to {pkg}, which has no resolved node"));
            }
        }
    }
    for member in array(metadata, "workspace_members")? {
        let member = member.as_str().ok_or("a workspace member that is not a string")?;
        if !packages.contains(member) {
            return Err(format!("workspace member {member} is not in `packages`"));
        }
        if !resolved.contains(member) {
            return Err(format!("workspace member {member} is not in `resolve.nodes`"));
        }
    }
    Ok(())
}

/// Every resolved edge to `testkit`'s package id must be `dev` only, and
/// say so: an edge with no `dep_kinds` proves nothing. Returns the packages
/// with a resolved dev edge to it.
fn resolved<'a>(
    metadata: &'a Value,
    testkit: &str,
    violations: &mut Vec<String>,
) -> Result<BTreeSet<&'a str>, String> {
    let mut dev = BTreeSet::new();
    for node in array(field(metadata, "resolve")?, "nodes")? {
        let from = text(node, "id")?;
        for dep in array(node, "deps")? {
            if text(dep, "pkg")? != testkit {
                continue;
            }
            let dep_kinds = array(dep, "dep_kinds")?;
            if dep_kinds.is_empty() {
                return Err(format!("{from}'s edge to {TESTKIT} has no `dep_kinds`"));
            }
            for dep_kind in dep_kinds {
                match kind(dep_kind)? {
                    "dev" => {
                        dev.insert(from);
                    },
                    kind => violations.push(format!(
                        "resolved: {from} reaches {TESTKIT} by a {kind} edge{}",
                        dep_kind
                            .get("target")
                            .and_then(Value::as_str)
                            .map(|t| format!(" under target `{t}`"))
                            .unwrap_or_default()
                    )),
                }
            }
        }
    }
    Ok(dev)
}
