//! Complete schema-derived declarations for catalog graphs exposed by the SDK.
use serde_json::Value;
use std::{collections::BTreeSet, fs, path::Path};

const ROOTS: &[&str] = &[
    "LensStopResponse",
    "LensClockMode",
    "LensMotorCounter",
    "HomeSwitchSide",
    "FirmwareLayouts",
    "PartAttachment",
    "BodyProfile",
    "SocProfile",
    "StabilizerCommand",
    "PhysicalControls",
    "PhysicalControl",
    "ControlIcon",
    "BootCombination",
    "BootHold",
];

fn name(value: &str) -> String {
    if ROOTS.contains(&value) {
        value.into()
    } else {
        format!("Catalog_{value}")
    }
}

fn render(schema: &Value, references: &mut BTreeSet<String>) -> Result<String, String> {
    if schema == &Value::Bool(false) {
        return Ok("never".into());
    }
    if schema == &Value::Bool(true) || schema == &serde_json::json!({}) {
        return Ok("unknown".into());
    }
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let target = reference
            .strip_prefix("#/$defs/")
            .ok_or_else(|| format!("Unsupported catalog reference {reference}"))?;
        references.insert(target.into());
        return Ok(name(target));
    }
    if let Some(value) = schema.get("const") {
        return Ok(value.to_string());
    }
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        return Ok(values
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join(" | "));
    }
    for (keyword, separator) in [("anyOf", " | "), ("oneOf", " | "), ("allOf", " & ")] {
        if let Some(branches) = schema.get(keyword).and_then(Value::as_array) {
            return Ok(format!(
                "({})",
                branches
                    .iter()
                    .map(|s| render(s, references))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(separator)
            ));
        }
    }
    if let Some(types) = schema.get("type").and_then(Value::as_array) {
        let mut branches = Vec::new();
        for ty in types {
            let mut branch = schema.clone();
            branch["type"] = ty.clone();
            branches.push(render(&branch, references)?);
        }
        return Ok(format!("({})", branches.join(" | ")));
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("null") => Ok("null".into()),
        Some("boolean") => Ok("boolean".into()),
        Some("integer" | "number") => Ok("number".into()),
        Some("string") => Ok("string".into()),
        Some("array") => {
            if let Some(items) = schema.get("prefixItems").and_then(Value::as_array) {
                return Ok(format!(
                    "[{}]",
                    items
                        .iter()
                        .map(|s| render(s, references))
                        .collect::<Result<Vec<_>, _>>()?
                        .join(", ")
                ));
            }
            let item = schema
                .get("items")
                .ok_or("Catalog array has no item schema")?;
            Ok(format!("Array<{}>", render(item, references)?))
        }
        Some("object") => {
            let required = schema.get("required").and_then(Value::as_array);
            let mut fields = Vec::new();
            if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
                for (key, value) in properties {
                    let optional =
                        if required.is_some_and(|r| r.contains(&Value::String(key.clone()))) {
                            ""
                        } else {
                            "?"
                        };
                    fields.push(format!(
                        "{}{}: {}",
                        serde_json::to_string(key).unwrap(),
                        optional,
                        render(value, references)?
                    ));
                }
            }
            let object = format!("{{ {} }}", fields.join("; "));
            match schema.get("additionalProperties") {
                Some(Value::Bool(false)) | None if !fields.is_empty() => Ok(object),
                Some(Value::Bool(false)) => Ok("Record<string, never>".into()),
                extra => {
                    let value = match extra {
                        Some(s) => render(s, references)?,
                        None => "unknown".into(),
                    };
                    let map = format!("Record<string, {value}>");
                    Ok(if fields.is_empty() {
                        map
                    } else {
                        format!("({object} & {map})")
                    })
                }
            }
        }
        _ => Err(format!("Unsupported catalog TypeScript schema: {schema}")),
    }
}

pub fn generate(directory: &Path, output: &Path) -> Result<(), String> {
    let definitions = super::catalog_types::schemas(directory)?;
    let mut pending: BTreeSet<String> = ROOTS.iter().map(|s| s.to_string()).collect();
    let mut emitted = BTreeSet::new();
    let mut source = String::from("// Generated from Rust catalog schemas.\n");
    while let Some(key) = pending.pop_first() {
        if !emitted.insert(key.clone()) {
            continue;
        }
        let schema = definitions
            .get(&key)
            .ok_or_else(|| format!("Missing catalog definition {key}"))?;
        let value = render(schema, &mut pending)?;
        source.push_str(&format!("export type {} = {value};\n", name(&key)));
    }
    source.push_str("export type BootMode = keyof NonNullable<PhysicalControls[\"boot\"]>;\n");
    fs::write(output, source).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn renders_tagged_unions_optional_fields_and_transitive_references() {
        let mut refs = BTreeSet::new();
        let schema = serde_json::json!({"oneOf":[{"type":"object","required":["kind","motor"],"properties":{
            "kind":{"const":"counter"},"motor":{"$ref":"#/$defs/LensMotorCounter"},"label":{"type":["string","null"]}},"additionalProperties":false},{"const":null}]});
        assert_eq!(
            render(&schema, &mut refs).unwrap(),
            "({ \"kind\": \"counter\"; \"label\"?: (string | null); \"motor\": LensMotorCounter } | null)"
        );
        assert_eq!(refs, BTreeSet::from(["LensMotorCounter".into()]));
    }
}
