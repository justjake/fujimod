//! Python catalog types generated from the catalog crate's schemars documents.
use serde_json::{Map, Value};
use std::{collections::BTreeMap, fs, path::Path};

type Definitions = BTreeMap<String, Value>;

const RUST_SCHEMA_FILES: &[(&str, &str)] = &[
    ("soc.schema.json", "Soc"),
    ("part.schema.json", "Part"),
    ("body.schema.json", "Body"),
    ("firmware.schema.json", "Firmware"),
    ("resolved-selection.schema.json", "ResolvedSelection"),
    (
        "resolved-configuration.schema.json",
        "ResolvedConfiguration",
    ),
    ("emulator-run-configuration.schema.json", "RunConfiguration"),
    ("emulator-run-overlay.schema.json", "RunOverlay"),
];

fn type_name(value: &Value) -> Option<&str> {
    value.get("title").and_then(Value::as_str)
}

fn insert_definition(
    definitions: &mut Definitions,
    name: String,
    schema: Value,
) -> Result<(), String> {
    if let Some(previous) = definitions.get(&name) {
        let mut previous_shape = previous.clone();
        let mut schema_shape = schema.clone();
        for value in [&mut previous_shape, &mut schema_shape] {
            if let Some(object) = value.as_object_mut() {
                for key in [
                    "$schema",
                    "$id",
                    "$anchor",
                    "$comment",
                    "description",
                    "title",
                ] {
                    object.remove(key);
                }
            }
        }
        if previous_shape != schema_shape {
            return Err(format!("catalog schema defines {name} inconsistently"));
        }
    } else {
        definitions.insert(name, schema);
    }
    Ok(())
}

fn inline_name(owner: &str, ordinal: usize) -> String {
    format!("{owner}Variant{ordinal}")
}

fn materialize_inline(
    schema: &mut Value,
    owner: &str,
    ordinal: &mut usize,
    generated: &mut Definitions,
    top_level: bool,
) -> Result<(), String> {
    if !top_level
        && schema.get("$ref").is_none()
        && schema.get("type").and_then(Value::as_str) == Some("object")
        && (schema.get("properties").is_some()
            || schema.get("additionalProperties") == Some(&Value::Bool(false)))
    {
        let name = inline_name(owner, *ordinal);
        *ordinal += 1;
        let mut definition = schema.clone();
        materialize_inline(&mut definition, owner, ordinal, generated, true)?;
        generated.insert(name.clone(), definition);
        *schema = serde_json::json!({ "$ref": format!("#/$defs/{name}") });
        return Ok(());
    }
    match schema {
        Value::Object(object) => {
            for (key, value) in object.iter_mut() {
                if key == "$defs" || key == "definitions" {
                    continue;
                }
                materialize_inline(value, owner, ordinal, generated, false)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                materialize_inline(value, owner, ordinal, generated, false)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn definitions(value: &Value) -> Option<&Map<String, Value>> {
    value
        .get("$defs")
        .or_else(|| value.get("definitions"))
        .and_then(Value::as_object)
}

pub(crate) fn schemas(directory: &Path) -> Result<Definitions, String> {
    let mut result = Definitions::new();
    for (filename, expected_name) in RUST_SCHEMA_FILES {
        let path = directory.join(filename);
        let mut schema: Value = serde_json::from_slice(
            &fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?,
        )
        .map_err(|error| format!("{}: {error}", path.display()))?;
        let mut generated = Definitions::new();
        for (name, value) in definitions(&schema).into_iter().flatten() {
            if RUST_SCHEMA_FILES
                .iter()
                .any(|(_, root_name)| *root_name == name.as_str())
            {
                // A root's standalone schema may carry tighter document
                // constraints than the same Rust type nested in another root.
                // Its canonical schema is emitted from its own file below.
                continue;
            }
            let mut value = value.clone();
            let mut ordinal = 0;
            materialize_inline(&mut value, name, &mut ordinal, &mut generated, true)?;
            insert_definition(&mut result, name.clone(), value)?;
        }
        let name = type_name(&schema)
            .ok_or_else(|| format!("{}: schema has no title", path.display()))?
            .to_owned();
        if name != *expected_name {
            return Err(format!(
                "{}: expected Rust type {expected_name}, found {name}",
                path.display()
            ));
        }
        let expected_id = format!("https://fujimod.com/schemas/{filename}");
        if schema.get("$id").and_then(Value::as_str) != Some(expected_id.as_str()) {
            return Err(format!(
                "{}: schema $id must be {expected_id}",
                path.display()
            ));
        }
        let mut ordinal = 0;
        materialize_inline(&mut schema, &name, &mut ordinal, &mut generated, true)?;
        insert_definition(&mut result, name, schema)?;
        for (name, schema) in generated {
            insert_definition(&mut result, name, schema)?;
        }
    }
    validate_references(&result)?;
    Ok(result)
}

fn ref_name(reference: &str) -> Result<String, String> {
    let fragment = reference
        .strip_prefix("#/")
        .ok_or_else(|| format!("unsupported catalog schema reference {reference:?}"))?;
    let mut segments = fragment.split('/');
    let scope = segments
        .next()
        .ok_or_else(|| format!("invalid catalog schema reference {reference:?}"))?;
    let name = segments
        .next()
        .ok_or_else(|| format!("invalid catalog schema reference {reference:?}"))?;
    if !matches!(scope, "$defs" | "definitions") || segments.next().is_some() {
        return Err(format!(
            "unsupported catalog schema reference {reference:?}"
        ));
    }
    Ok(name.replace("~1", "/").replace("~0", "~"))
}

fn validate_references(definitions: &Definitions) -> Result<(), String> {
    fn visit(value: &Value, definitions: &Definitions) -> Result<(), String> {
        match value {
            Value::Object(object) => {
                if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                    let name = ref_name(reference)?;
                    if !definitions.contains_key(&name) {
                        return Err(format!("catalog schema references missing type {name}"));
                    }
                }
                for value in object.values() {
                    visit(value, definitions)?;
                }
            }
            Value::Array(values) => {
                for value in values {
                    visit(value, definitions)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    for schema in definitions.values() {
        visit(schema, definitions)?;
    }
    Ok(())
}

fn python_union(types: impl IntoIterator<Item = Result<String, String>>) -> Result<String, String> {
    let mut rendered = Vec::new();
    for ty in types {
        let ty = ty?;
        if !rendered.contains(&ty) {
            rendered.push(ty);
        }
    }
    match rendered.len() {
        0 => Ok("never".into()),
        1 => Ok(rendered.remove(0)),
        _ => Ok(rendered.join(" | ")),
    }
}

fn python_enum(values: &[Value]) -> Result<String, String> {
    let rendered = values
        .iter()
        .map(|value| match value {
            Value::String(text) => serde_json::to_string(text).map_err(|error| error.to_string()),
            Value::Number(number) => Ok(number.to_string()),
            Value::Bool(value) => Ok(if *value { "True" } else { "False" }.into()),
            Value::Null => Ok("None".into()),
            _ => Err("catalog schema enum values must be scalar".into()),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(format!("Literal[{}]", rendered.join(", ")))
}

fn python_string_literal(value: &str) -> String {
    let single_quotes = value.chars().filter(|character| *character == '\'').count();
    let double_quotes = value.chars().filter(|character| *character == '"').count();
    let quote = if double_quotes > single_quotes {
        '\''
    } else {
        '"'
    };
    let mut rendered = String::with_capacity(value.len() + 2);
    rendered.push(quote);
    for character in value.chars() {
        match character {
            '\\' => rendered.push_str("\\\\"),
            '\n' => rendered.push_str("\\n"),
            '\r' => rendered.push_str("\\r"),
            '\t' => rendered.push_str("\\t"),
            character if character == quote => {
                rendered.push('\\');
                rendered.push(character);
            }
            character => rendered.push(character),
        }
    }
    rendered.push(quote);
    rendered
}

fn python_union_branches(value: &str) -> Vec<String> {
    let characters: Vec<_> = value.chars().collect();
    let mut branches = Vec::new();
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;
    let mut square_depth = 0;
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == active_quote {
                quote = None;
            }
            index += 1;
            continue;
        }
        match character {
            '\'' | '"' => quote = Some(character),
            '[' => square_depth += 1,
            ']' => square_depth -= 1,
            '|' if square_depth == 0
                && index > 0
                && index + 1 < characters.len()
                && characters[index - 1] == ' '
                && characters[index + 1] == ' ' =>
            {
                branches.push(characters[start..index - 1].iter().collect());
                start = index + 2;
                index += 2;
                continue;
            }
            _ => {}
        }
        index += 1;
    }
    branches.push(characters[start..].iter().collect());
    branches
}

fn python_alias(name: &str, value: &str) -> String {
    let declaration = format!("type {name} = {value}");
    if declaration.len() <= 88 {
        return format!("{declaration}\n\n\n");
    }
    if value.starts_with("Literal[") && value.ends_with(']') {
        let values = &value[8..value.len() - 1];
        let mut entries = Vec::new();
        let mut start = 0;
        let mut quote = None;
        let mut escaped = false;
        for (index, character) in values.char_indices() {
            if let Some(active_quote) = quote {
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == active_quote {
                    quote = None;
                }
            } else if character == '\'' || character == '"' {
                quote = Some(character);
            } else if character == ',' {
                entries.push(values[start..index].trim());
                start = index + character.len_utf8();
            }
        }
        entries.push(values[start..].trim());
        let mut rendered = format!("type {name} = Literal[\n");
        for entry in entries {
            rendered.push_str(&format!("    {entry},\n"));
        }
        rendered.push_str("]\n\n\n");
        return rendered;
    }
    let branches = python_union_branches(value);
    if branches.len() > 1 {
        if branches.join(" | ").len() + 4 <= 88 {
            return format!("type {name} = (\n    {}\n)\n\n\n", branches.join(" | "));
        }
        let mut rendered = format!("type {name} = (\n    {}\n", branches[0]);
        for branch in &branches[1..] {
            rendered.push_str(&format!("    | {branch}\n"));
        }
        rendered.push_str(")\n\n\n");
        return rendered;
    }
    format!("{declaration}\n\n\n")
}

fn property_type(schema: &Value, python_string_refs: bool) -> Result<String, String> {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let name = ref_name(reference)?;
        return Ok(if python_string_refs {
            format!("\"{name}\"")
        } else {
            name
        });
    }
    if let Some(value) = schema.get("const") {
        return literal(value);
    }
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        return python_enum(values);
    }
    for key in ["oneOf", "anyOf"] {
        if let Some(values) = schema.get(key).and_then(Value::as_array) {
            if values.iter().all(|value| value.get("const").is_some()) {
                let literals: Vec<_> = values.iter().map(|value| value["const"].clone()).collect();
                return python_enum(&literals);
            }
            return python_union(
                values
                    .iter()
                    .map(|value| property_type(value, python_string_refs)),
            );
        }
    }
    if let Some(values) = schema.get("allOf").and_then(Value::as_array) {
        if values.len() > 1 {
            return Err("Python catalog types cannot express multi-branch allOf schemas".into());
        }
        return python_union(
            values
                .iter()
                .map(|value| property_type(value, python_string_refs)),
        );
    }
    if let Some(types) = schema.get("type").and_then(Value::as_array) {
        if types.is_empty() || types.iter().any(|value| !value.is_string()) {
            return Err("catalog schema type arrays must contain string names".into());
        }
        return python_union(
            types
                .iter()
                .map(|value| primitive(value.as_str().unwrap(), schema, python_string_refs)),
        );
    }
    if let Some(name) = schema.get("type").and_then(Value::as_str) {
        return primitive(name, schema, python_string_refs);
    }
    if schema.get("properties").is_some() || schema.get("additionalProperties").is_some() {
        return Err("nested object catalog schemas must have a named Rust type".into());
    }
    Err("catalog schema has no type, reference, literal or composition".into())
}

fn literal(value: &Value) -> Result<String, String> {
    match value {
        Value::String(text) => {
            let encoded = serde_json::to_string(text).map_err(|error| error.to_string())?;
            Ok(format!("Literal[{encoded}]"))
        }
        Value::Number(number) => Ok(format!("Literal[{number}]")),
        Value::Bool(value) => Ok(format!(
            "Literal[{}]",
            if *value { "True" } else { "False" }
        )),
        Value::Null => Ok("None".into()),
        _ => Err("catalog schema enum values must be scalar".into()),
    }
}

fn primitive(name: &str, schema: &Value, python_string_refs: bool) -> Result<String, String> {
    match name {
        "object" => {
            if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
                if properties.is_empty() {
                    return Ok("dict[str, Never]".into());
                }
                return Err(
                    "inline object catalog schemas must be materialized as named types".into(),
                );
            }
            if let Some(additional) = schema.get("additionalProperties") {
                if additional == &Value::Bool(false) {
                    return Ok("dict[str, Never]".into());
                }
                let item = property_type(additional, python_string_refs)?;
                return Ok(format!("dict[str, {item}]"));
            }
            Err("open catalog object has no typed additionalProperties schema".into())
        }
        "array" if schema.get("prefixItems").is_some() => {
            let members = schema["prefixItems"]
                .as_array()
                .ok_or("catalog tuple prefixItems is not a list")?
                .iter()
                .map(|value| property_type(value, python_string_refs))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(format!("tuple[{}]", members.join(", ")))
        }
        "array" => {
            let item = schema
                .get("items")
                .map(|value| property_type(value, python_string_refs))
                .unwrap_or_else(|| Err("catalog array schema has no item type".into()))?;
            Ok(format!("list[{item}]"))
        }
        "integer" | "number" => Ok("int".into()),
        "string" => Ok("str".into()),
        "boolean" => Ok("bool".into()),
        "null" => Ok("None".into()),
        other => Err(format!("unsupported catalog schema type {other:?}")),
    }
}

fn object_members(schema: &Value) -> Option<(&Map<String, Value>, Vec<String>)> {
    let properties = schema.get("properties")?.as_object()?;
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    Some((properties, required))
}

fn python_identifier(name: &str) -> bool {
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    let starts_valid = first == '_' || first.is_ascii_alphabetic();
    let rest_valid =
        characters.all(|character| character == '_' || character.is_ascii_alphanumeric());
    starts_valid
        && rest_valid
        && !matches!(
            name,
            "False"
                | "None"
                | "True"
                | "and"
                | "as"
                | "assert"
                | "async"
                | "await"
                | "break"
                | "class"
                | "continue"
                | "def"
                | "del"
                | "elif"
                | "else"
                | "except"
                | "finally"
                | "for"
                | "from"
                | "global"
                | "if"
                | "import"
                | "in"
                | "is"
                | "lambda"
                | "nonlocal"
                | "not"
                | "or"
                | "pass"
                | "raise"
                | "return"
                | "try"
                | "while"
                | "with"
                | "yield"
        )
}

fn python(definitions: &Definitions) -> Result<String, String> {
    let mut output = String::new();
    for (name, schema) in definitions {
        if let Some((properties, required)) = object_members(schema) {
            if properties.keys().all(|field| python_identifier(field)) {
                output.push_str(&format!("class {name}(TypedDict, total=False):\n"));
                for (field, property) in properties {
                    let ty = property_type(property, false)?;
                    let marker = if required.contains(field) {
                        "Required"
                    } else {
                        "NotRequired"
                    };
                    output.push_str(&format!("    {field}: {marker}[{ty}]\n"));
                }
            } else {
                output.push_str(&format!("{name} = TypedDict(\n    {name:?},\n    {{\n"));
                for (field, property) in properties {
                    // Functional TypedDict evaluates its annotations immediately. Keep the
                    // complete annotation as a forward-reference string so refs inside
                    // unions, optionals, lists, and maps do not depend on declaration order.
                    let ty = property_type(property, false)?;
                    let ty = python_string_literal(&ty);
                    let marker = if required.contains(field) {
                        "Required"
                    } else {
                        "NotRequired"
                    };
                    let annotation = format!("{marker}[{ty}]");
                    if format!("        {field:?}: {annotation},").len() > 88 {
                        output.push_str(&format!(
                            "        {field:?}: {marker}[\n            {ty}\n        ],\n"
                        ));
                    } else {
                        output.push_str(&format!("        {field:?}: {annotation},\n"));
                    }
                }
                output.push_str("    },\n    total=False,\n)");
            }
            if properties.keys().all(|field| python_identifier(field)) {
                output.push_str("\n\n");
            } else {
                output.push_str("\n\n\n");
            }
        } else {
            output.push_str(&python_alias(name, &property_type(schema, false)?));
        }
    }
    let never_import = if output.contains("dict[str, Never]") {
        "Never, "
    } else {
        ""
    };
    Ok(format!(
        "# Generated from the Rust catalog schemas.\nfrom __future__ import annotations\n\nfrom typing import Literal, {never_import}NotRequired, Required, TypedDict\n\n\n{}\n",
        output.trim_end()
    ))
}

/// Generate the Python catalog types from Rust-derived JSON Schema.
pub fn generate(schema_directory: &Path, python_path: &Path, check: bool) -> Result<(), String> {
    let definitions = schemas(schema_directory)?;
    let expected = [(python_path, python(&definitions)?)];
    for (path, content) in expected {
        if check {
            let actual =
                fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
            if actual != content {
                return Err(format!(
                    "{} is stale; run fujimod-bindgen --catalog-types",
                    path.display()
                ));
            }
        } else {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            fs::write(path, content).map_err(|error| format!("{}: {error}", path.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn generates_python_from_closed_schema_graphs() {
        let configuration = r##"{
            "$schema":"https://json-schema.org/draft/2020-12/schema",
            "$id":"https://fujimod.com/schemas/emulator-run-configuration.schema.json",
            "title":"RunConfiguration",
            "type":"object",
            "properties":{
                "$schema":{"type":"string"},
                "schema":{"const":1},
                "body_id":{"$ref":"#/$defs/Identity"},
                "firmware_id":{"$ref":"#/$defs/Identity"},
                "overlays":{"type":"array","items":{"$ref":"#/$defs/RunOverlay"}}
            },
            "required":["$schema","schema","body_id","overlays"],
            "additionalProperties":false,
            "$defs":{
                "Identity":{"type":"string","minLength":1},
                "RunOverlay":{"oneOf":[
                    {"type":"object","properties":{"kind":{"const":"display"}},"required":["kind"],"additionalProperties":false},
                    {"type":"object","properties":{"kind":{"const":"controls"}},"required":["kind"],"additionalProperties":false}
                ]}
            }
        }"##;
        let overlay = r##"{
            "$schema":"https://json-schema.org/draft/2020-12/schema",
            "$id":"https://fujimod.com/schemas/emulator-run-overlay.schema.json",
            "title":"RunOverlay",
            "oneOf":[
                {"type":"object","properties":{"kind":{"const":"display"}},"required":["kind"],"additionalProperties":false},
                {"type":"object","properties":{"kind":{"const":"controls"}},"required":["kind"],"additionalProperties":false}
            ]
        }"##;
        let directory = std::env::temp_dir().join(format!(
            "fujimod-bindgen-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        for (filename, name) in [
            ("soc.schema.json", "Soc"),
            ("part.schema.json", "Part"),
            ("body.schema.json", "Body"),
            ("firmware.schema.json", "Firmware"),
            ("resolved-selection.schema.json", "ResolvedSelection"),
            (
                "resolved-configuration.schema.json",
                "ResolvedConfiguration",
            ),
        ] {
            fs::write(
                directory.join(filename),
                format!("{{\"$id\":\"https://fujimod.com/schemas/{filename}\",\"title\":\"{name}\",\"type\":\"object\",\"properties\":{{\"id\":{{\"type\":\"string\"}}}},\"required\":[\"id\"],\"additionalProperties\":false}}"),
            )
            .unwrap();
        }
        fs::write(
            directory.join("emulator-run-configuration.schema.json"),
            configuration,
        )
        .unwrap();
        fs::write(directory.join("emulator-run-overlay.schema.json"), overlay).unwrap();
        let schemas = schemas(&directory).unwrap();
        let python = python(&schemas).unwrap();
        assert!(python.contains("\"$schema\": Required[\"str\"]"));
        assert!(python.contains("\"firmware_id\": NotRequired[\"Identity\"]"));
        assert!(python.contains("type RunOverlay = RunOverlayVariant0 | RunOverlayVariant1"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_untyped_and_open_schema_shapes() {
        for schema in [
            serde_json::json!({}),
            serde_json::json!({ "type": "object", "additionalProperties": true }),
            serde_json::json!({ "type": "array" }),
        ] {
            assert!(property_type(&schema, false).is_err());
        }
        assert_eq!(
            primitive(
                "object",
                &serde_json::json!({ "type": "object", "additionalProperties": false }),
                false,
            )
            .unwrap(),
            "dict[str, Never]"
        );
    }

    #[test]
    fn references_are_local_named_types_and_python_unions_use_bare_names() {
        assert_eq!(ref_name("#/$defs/RunStart").unwrap(), "RunStart");
        assert!(ref_name("https://example.com/schema.json#/$defs/RunStart").is_err());
        assert!(ref_name("#/$defs/RunStart/properties/kind").is_err());
        let definitions = BTreeMap::from([(
            "Example".into(),
            serde_json::json!({
                "oneOf": [
                    { "const": "rom" },
                    { "$ref": "#/$defs/RunStart" }
                ]
            }),
        )]);
        assert!(validate_references(&definitions).is_err());
        assert_eq!(
            property_type(&definitions["Example"], false).unwrap(),
            "Literal[\"rom\"] | RunStart"
        );
    }
}
