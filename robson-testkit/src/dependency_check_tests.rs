//! `dependency_check` against synthetic metadata (8d plan §5.4, §7.1): the
//! workspace's shape passes; a renamed, target-specific, build or optional
//! edge fails, as does one only the resolved graph shows; input that proves
//! nothing is refused.

use serde_json::{json, Value};

use crate::dependency_check::{check, Summary};

const ROBSOND: &str = "path+file:///w/robsond#2.5.0";
const PROJECTOR: &str = "path+file:///w/robson-projector#2.5.0";
const TESTKIT: &str = "path+file:///w/robson-testkit#2.5.0";
const EXEC: &str = "path+file:///w/robson-exec#2.5.0";

fn edge(name: &str, kind: Value) -> Value {
    json!({ "name": name, "kind": kind, "rename": null, "optional": false, "target": null })
}

fn package(name: &str, id: &str, dependencies: Vec<Value>) -> Value {
    json!({ "name": name, "id": id, "dependencies": dependencies })
}

fn resolved(pkg: &str, name: &str, kind: Value) -> Value {
    json!({ "name": name, "pkg": pkg, "dep_kinds": [{ "kind": kind, "target": null }] })
}

/// The workspace's shape: robsond and robson-projector reach the testkit by
/// dev edges only.
fn workspace() -> Value {
    json!({
        "version": 1,
        "workspace_members": [ROBSOND, PROJECTOR, TESTKIT, EXEC],
        "packages": [
            package("robsond", ROBSOND, vec![edge("robson-exec", Value::Null), edge("robson-testkit", json!("dev"))]),
            package("robson-projector", PROJECTOR, vec![edge("robson-testkit", json!("dev"))]),
            package("robson-testkit", TESTKIT, vec![edge("robson-exec", Value::Null), edge("robson-projector", Value::Null)]),
            package("robson-exec", EXEC, vec![]),
        ],
        "resolve": { "root": null, "nodes": [
            { "id": ROBSOND, "deps": [resolved(EXEC, "robson_exec", Value::Null), resolved(TESTKIT, "robson_testkit", json!("dev"))] },
            { "id": PROJECTOR, "deps": [resolved(TESTKIT, "robson_testkit", json!("dev"))] },
            { "id": TESTKIT, "deps": [resolved(EXEC, "robson_exec", Value::Null), resolved(PROJECTOR, "robson_projector", Value::Null)] },
            { "id": EXEC, "deps": [] },
        ]},
    })
}

fn deps(metadata: &mut Value, package: usize) -> &mut Vec<Value> {
    metadata["packages"][package]["dependencies"].as_array_mut().unwrap()
}

fn refused(metadata: &Value, expected: &str) {
    let problems = check(&metadata.to_string()).unwrap_err();
    assert!(
        problems.iter().any(|p| p.contains(expected)),
        "{expected:?} not in {problems:?}"
    );
}

#[test]
fn the_workspace_shape_passes() {
    let summary = check(&workspace().to_string()).unwrap();
    assert_eq!(summary, Summary {
        dev_dependents: vec!["robson-projector".into(), "robsond".into()]
    });
}

#[test]
fn a_renamed_normal_edge_is_refused() {
    let mut metadata = workspace();
    let mut renamed = edge("robson-testkit", Value::Null);
    renamed["rename"] = json!("kit");
    deps(&mut metadata, 3).push(renamed);
    refused(&metadata, "robson-exec reaches robson-testkit by a normal edge, renamed `kit`");
}

#[test]
fn a_target_specific_edge_is_refused() {
    let mut metadata = workspace();
    let mut unix = edge("robson-testkit", Value::Null);
    unix["target"] = json!("cfg(unix)");
    deps(&mut metadata, 0).push(unix);
    refused(
        &metadata,
        "robsond reaches robson-testkit by a normal edge, under target `cfg(unix)`",
    );
}

#[test]
fn a_build_edge_is_refused() {
    let mut metadata = workspace();
    deps(&mut metadata, 0).push(edge("robson-testkit", json!("build")));
    refused(&metadata, "robsond reaches robson-testkit by a build edge");
}

#[test]
fn an_optional_edge_no_feature_activates_is_refused() {
    let mut metadata = workspace();
    let mut optional = edge("robson-testkit", Value::Null);
    optional["optional"] = json!(true);
    deps(&mut metadata, 3).push(optional);
    refused(&metadata, "robson-exec reaches robson-testkit by a normal edge, optional");
}

#[test]
fn an_edge_only_the_resolved_graph_shows_is_refused() {
    let mut metadata = workspace();
    let node = &mut metadata["resolve"]["nodes"][0]["deps"][1]["dep_kinds"];
    node.as_array_mut().unwrap().push(json!({ "kind": null, "target": null }));
    refused(
        &metadata,
        &format!("resolved: {ROBSOND} reaches robson-testkit by a normal edge"),
    );
}

#[test]
fn input_that_proves_nothing_is_refused() {
    for (input, expected) in [
        ("", "empty input"),
        ("  \n", "empty input"),
        ("{", "not cargo metadata's JSON"),
        ("[]", "input is not cargo metadata"),
        ("{}", "no `version`"),
    ] {
        let problems = check(input).unwrap_err();
        assert!(problems.iter().any(|p| p.contains(expected)), "{input:?}: {problems:?}");
    }
    let mut version = workspace();
    version["version"] = json!(2);
    refused(&version, "version Some(2), not 1");
    for missing in ["packages", "workspace_members", "resolve"] {
        let mut metadata = workspace();
        metadata[missing] = Value::Null;
        refused(&metadata, &format!("no `{missing}`"));
    }
}

#[test]
fn metadata_that_is_not_this_workspace_is_refused() {
    let mut outside = workspace();
    outside["workspace_members"] = json!([ROBSOND, PROJECTOR, EXEC]);
    refused(&outside, "robson-testkit is not a workspace member");
    let mut no_dev = workspace();
    deps(&mut no_dev, 0).retain(|d| d["name"] != "robson-testkit");
    deps(&mut no_dev, 1).clear();
    refused(&no_dev, "no workspace package reaches robson-testkit by a dev edge");
}

/// P2-3 (8d-1b round 1): an incomplete graph proves nothing, whatever its
/// declared edges say.
#[test]
fn an_incomplete_graph_is_refused() {
    const GHOST: &str = "path+file:///w/ghost#0.1.0";
    let mut no_nodes = workspace();
    no_nodes["resolve"]["nodes"] = json!([]);
    refused(&no_nodes, "`resolve.nodes` is empty");

    let mut no_kinds = workspace();
    no_kinds["resolve"]["nodes"][0]["deps"][1]["dep_kinds"] = json!([]);
    refused(&no_kinds, "edge to robson-testkit has no `dep_kinds`");

    let mut ghost_member = workspace();
    ghost_member["workspace_members"].as_array_mut().unwrap().push(json!(GHOST));
    refused(&ghost_member, &format!("workspace member {GHOST} is not in `packages`"));

    // The member's node gone, and with it the one edge to it.
    let mut unresolved_member = workspace();
    let nodes = unresolved_member["resolve"]["nodes"].as_array_mut().unwrap();
    nodes.remove(1);
    nodes[1]["deps"].as_array_mut().unwrap().retain(|dep| dep["pkg"] != PROJECTOR);
    refused(
        &unresolved_member,
        &format!("workspace member {PROJECTOR} is not in `resolve.nodes`"),
    );

    let mut ghost_edge = workspace();
    let deps = ghost_edge["resolve"]["nodes"][3]["deps"].as_array_mut().unwrap();
    deps.push(resolved(GHOST, "ghost", Value::Null));
    refused(&ghost_edge, &format!("has an edge to {GHOST}, which is not in `packages`"));

    let mut unshown = workspace();
    unshown["resolve"]["nodes"][0]["deps"].as_array_mut().unwrap().remove(1);
    refused(
        &unshown,
        &format!("the declared dev edge from {ROBSOND} to robson-testkit is not in"),
    );
}

/// Round 2: a resolved edge must reach a resolved node. A package with no
/// node, even outside the workspace, leaves the graph dangling; a leaf node
/// outside the workspace (a path or registry dependency with no deps of its
/// own) is complete and passes.
#[test]
fn every_resolved_edge_reaches_a_node() {
    let mut dangling = workspace();
    dangling["workspace_members"] = json!([ROBSOND, PROJECTOR, TESTKIT]);
    dangling["resolve"]["nodes"].as_array_mut().unwrap().remove(3);
    refused(
        &dangling,
        &format!("{ROBSOND} has an edge to {EXEC}, which has no resolved node"),
    );

    const LEAF: &str = "registry+https://github.com/rust-lang/crates.io-index#leaf@1.0.0";
    let mut leaf = workspace();
    leaf["packages"].as_array_mut().unwrap().push(package("leaf", LEAF, vec![]));
    deps(&mut leaf, 2).push(edge("leaf", Value::Null));
    let nodes = leaf["resolve"]["nodes"].as_array_mut().unwrap();
    nodes[2]["deps"]
        .as_array_mut()
        .unwrap()
        .push(resolved(LEAF, "leaf", Value::Null));
    nodes.insert(0, json!({ "id": LEAF, "deps": [] }));
    let summary = check(&leaf.to_string()).unwrap();
    assert_eq!(summary.dev_dependents, ["robson-projector", "robsond"]);
}
