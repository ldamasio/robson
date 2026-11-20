//! Independent source identities; rendered semantics come only from original
//! DO38.
use std::collections::BTreeMap;

use anyhow::{bail, ensure, Result};
use serde_json::Value;
use sqlx::migrate::{Migration, MigrationType};

pub(super) const ORIGINAL: &str =
    include_str!("../../../migrations/20240101000038_request_budget.sql");
pub(super) const CHECKSUM: &str = "dbbe4861d2a8ca80bb838172c322e9bb284d3508fe72ba1be4917a9f08220c69e37d3a26da859ad3a040c34f665d7aaf";
pub(super) fn source_hash() -> Result<String> {
    let m = Migration::new(
        20240101000038,
        "reference only".into(),
        MigrationType::Simple,
        ORIGINAL.into(),
        false,
    );
    let actual: String = m.checksum.iter().map(|b| format!("{b:02x}")).collect();
    ensure!(actual == CHECKSUM, "original source checksum drift");
    Ok(actual)
}
/// Run ORIGINAL as the reference inside the caller's open transaction. The
/// fixture's sentinel statement_timeout must not bound this oracle, so it is
/// lifted transaction-locally and restored before the caller's capture.
pub(super) async fn run_original(c: &mut sqlx::PgConnection) -> Result<()> {
    let statement: String = sqlx::query_scalar("SELECT current_setting('statement_timeout')")
        .persistent(false)
        .fetch_one(&mut *c)
        .await?;
    sqlx::query("SELECT set_config('statement_timeout','0',true)")
        .persistent(false)
        .execute(&mut *c)
        .await?;
    sqlx::raw_sql(ORIGINAL).execute(&mut *c).await?;
    sqlx::query("SELECT set_config('statement_timeout',$1,true)")
        .bind(&statement)
        .persistent(false)
        .execute(&mut *c)
        .await?;
    Ok(())
}
#[path = "catalog_contract_data.rs"]
mod data;
use data::{KEYS, SPECS};

pub(super) fn maps(v: &Value) -> Result<(BTreeMap<&str, &Value>, BTreeMap<&str, &Value>)> {
    if v["overflow"] == true {
        ensure!(
            v["reason"] == "catalog count/duplicate/byte bound",
            "catalog overflow reason invalid"
        );
        bail!("catalog count/duplicate/byte bound");
    }
    ensure!(v["overflow"] == false, "catalog overflow flag missing/invalid");
    let records = v["records"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("catalog records missing"))?;
    ensure!(records.len() <= 4096, "catalog row bound");
    let (mut semantic, mut identity) = (BTreeMap::new(), BTreeMap::new());
    for r in records {
        ensure!(serde_json::to_vec(r)?.len() <= 65536, "catalog descriptor byte bound");
        let key = r["key"].as_str().ok_or_else(|| anyhow::anyhow!("catalog key missing"))?;
        ensure!(
            r["sem"].is_object() && r.get("identity").is_some(),
            "catalog semantic/identity descriptor missing"
        );
        ensure!(semantic.insert(key, &r["sem"]).is_none(), "duplicate catalog key");
        identity.insert(key, &r["identity"]);
    }
    Ok((semantic, identity))
}
fn names(v: &Value) -> Result<Vec<&str>> {
    v.as_array()
        .ok_or_else(|| anyhow::anyhow!("key array missing"))?
        .iter()
        .map(|v| v.as_str().ok_or_else(|| anyhow::anyhow!("key name missing")))
        .collect()
}
pub(super) fn declared(v: &Value) -> Result<()> {
    let (m, _) = maps(v)?;
    ensure!(SPECS.len() == 136 && KEYS.len() == 29, "source contract cardinality");
    for &(table, name, kind, valid) in SPECS {
        let key = format!("constraint:public.{table}/{name}");
        let c = m
            .get(key.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing declared constraint {key}"))?;
        let deferred = name == "budget_clock_charge_fk";
        ensure!(c["type"] == kind && c["validated"] == valid, "constraint type/validation {key}");
        ensure!(
            c["deferrable"] == deferred && c["deferred"] == deferred,
            "constraint deferral {key}"
        );
        ensure!(c["definition"].is_string(), "constraint definition missing {key}");
        if kind == "c" {
            ensure!(c["expression"].is_string(), "CHECK expression missing {key}");
        }
    }
    let roles = [
        ("RI_FKey_check_ins", 5, false),
        ("RI_FKey_check_upd", 17, false),
        ("RI_FKey_noaction_del", 9, true),
        ("RI_FKey_noaction_upd", 17, true),
    ];
    let mut ri = 0;
    for &(id, keys, parent, parent_keys) in KEYS {
        let lookup = format!("constraint:public.{id}");
        let c = m
            .get(lookup.as_str())
            .ok_or_else(|| anyhow::anyhow!("source key missing {id}"))?;
        ensure!(
            names(&c["key_names"])? == keys.split(',').collect::<Vec<_>>(),
            "ordered key {id}"
        );
        if parent.is_empty() {
            continue;
        }
        ensure!(c["parent_table"] == format!("public.{parent}"), "FK relation {id}");
        ensure!(
            names(&c["parent_keys"])? == parent_keys.split(',').collect::<Vec<_>>(),
            "FK order {id}"
        );
        ensure!(c["match"] == "s" && c["update"] == "a" && c["delete"] == "a", "FK actions {id}");
        let table = id.split('/').next().ok_or_else(|| anyhow::anyhow!("source FK id"))?;
        let prefix = format!("ri:public.{id}/");
        let actual: Vec<_> = m.iter().filter(|(k, _)| k.starts_with(&prefix)).collect();
        ensure!(actual.len() == 4, "RI role count {id}");
        for (function, flags, on_parent) in roles {
            let relation = if on_parent { parent } else { table };
            let other = if on_parent { table } else { parent };
            let key = format!("{prefix}{function}/{relation}");
            let t = m.get(key.as_str()).ok_or_else(|| anyhow::anyhow!("missing RI role {key}"))?;
            ensure!(t["function"] == format!("pg_catalog.{function}()"), "RI function {key}");
            ensure!(
                t["constraint"] == format!("constraint:public.{id}") && t["binding_ok"] == true,
                "RI FK link {key}"
            );
            ensure!(
                t["table"] == format!("public.{relation}")
                    && t["other_table"] == format!("public.{other}"),
                "RI directions {key}"
            );
            ensure!(t["index"] == c["index"] && !c["index"].is_null(), "RI supporting index {key}");
            ensure!(
                t["type"] == flags && t["internal"] == true && t["enabled"] == "O",
                "RI flags {key}"
            );
            ensure!(
                t["deferrable"] == c["deferrable"] && t["deferred"] == c["deferred"],
                "RI deferral {key}"
            );
            ensure!(
                t["arguments"] == "" && t["update_columns"] == "" && t["when"].is_null(),
                "RI condition {key}"
            );
            ensure!(
                t["old_table"].is_null() && t["new_table"].is_null(),
                "RI transition table {key}"
            );
            ri += 1;
        }
    }
    ensure!(ri == 60, "native RI total");
    for (position, name, kind) in [
        (1, "kind", "text"),
        (2, "boot_uuid", "uuid"),
        (3, "offset_seconds", "bigint"),
        (4, "offset_nanoseconds", "integer"),
    ] {
        let key = format!("attribute:public.budget_clock_domain_v38/{name}");
        let a = m
            .get(key.as_str())
            .ok_or_else(|| anyhow::anyhow!("composite attribute {key}"))?;
        ensure!(a["number"] == position && a["type"] == kind, "composite type/order {key}");
    }
    ensure!(
        m.contains_key("constraint:public.recovery_incidents/recovery_incidents_reason_check"),
        "old reason CHECK removed"
    );
    if !m.contains_key("type:public.budget_clock_domain_v38") {
        bail!("standalone composite missing");
    }
    Ok(())
}
