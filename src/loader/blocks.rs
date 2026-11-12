//! Hash-verified, connection-scoped carrier blocks. No dynamic vanilla state IDs.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BlockDefinition {
    pub id: String,
    pub name: String,
    pub model: String,
    pub carrier: String,
    pub drop_item: String,
}

pub fn parse(entries: &[Value]) -> Result<Vec<BlockDefinition>, String> {
    if entries.len() > 128 { return Err("Loader block count exceeds 128".into()); }
    let mut ids = BTreeSet::new();
    let mut carriers = BTreeSet::new();
    entries.iter().map(|entry| {
        let block: BlockDefinition = serde_json::from_value(entry.clone()).map_err(|e| e.to_string())?;
        for id in [&block.id, &block.model, &block.carrier, &block.drop_item] {
            super::resource_id(id)?;
        }
        if block.name.is_empty() || block.name.len() > 128 {
            return Err("Loader block name must be 1..128 bytes".into());
        }
        if !matches!(block.carrier.as_str(),
            "minecraft:gray_glazed_terracotta" |
            "minecraft:pink_glazed_terracotta" |
            "minecraft:blue_glazed_terracotta")
        {
            return Err("Unsupported Loader block carrier".into());
        }
        if !ids.insert(block.id.clone()) || !carriers.insert(block.carrier.clone()) {
            return Err("Duplicate Loader block/carrier".into());
        }
        Ok(block)
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn block_contract_rejects_ambiguous_carriers_and_unknown_fields() {
        let entry = serde_json::json!({"id":"computer:monitor","name":"Monitor","model":"computer:block/monitor",
            "carrier":"minecraft:gray_glazed_terracotta","drop_item":"computer:monitor"});
        assert_eq!(parse(std::slice::from_ref(&entry)).unwrap()[0].id, "computer:monitor");
        assert!(parse(&[entry.clone(), entry.clone()]).is_err());
        for carrier in ["minecraft:pink_glazed_terracotta", "minecraft:blue_glazed_terracotta"] {
            let mut valid = entry.clone();
            valid["carrier"] = serde_json::json!(carrier);
            assert!(parse(&[valid]).is_ok());
        }
        let mut old = entry.clone();
        old["carrier"] = serde_json::json!("minecraft:black_glazed_terracotta");
        assert!(parse(&[old]).is_err());
        let mut invalid = entry;
        invalid["states"] = serde_json::json!([]);
        assert!(parse(&[invalid]).is_err());
    }
}
