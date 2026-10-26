//! Python's statically visible names for the schema-driven payload classes.
use super::*;
use std::fmt::Write;

fn field_type(ty: &unions::ValueType) -> String {
    match ty {
        unions::ValueType::Integer { .. } => "int".into(),
        unions::ValueType::Named { name } => name.clone(),
        unions::ValueType::Bytes
        | unions::ValueType::Array {
            bits: 8,
            signed: false,
            ..
        } => "bytes".into(),
        unions::ValueType::Array { length, .. } => {
            format!("tuple[{}]", vec!["int"; *length].join(", "))
        }
    }
}

impl Api {
    pub fn write_python_types(&self, out: &Path) -> Result<(), String> {
        fs::create_dir_all(out).map_err(|e| e.to_string())?;
        let mut text = String::from(
            "# Generated from the Rust binding declarations.\nfrom enum import Enum\nfrom collections.abc import Buffer\nfrom fujimod._variants import Sum\nfrom fujimod._bridge import Handle\n",
        );
        for quantity in &self.service.quantities {
            writeln!(text, "from _fujimod_js import Js{}", quantity.name).unwrap();
        }
        for union in self.service.unions.iter().filter(|union| union.input) {
            writeln!(text, "from _fujimod_js import Js{}", union.wasm_name()).unwrap();
        }
        text.push('\n');
        writeln!(text, "class Codec(Enum):").unwrap();
        if self.codec.variants.is_empty() {
            text.push_str("    pass\n");
        }
        for name in &self.codec.variants {
            writeln!(text, "    {name} = ...").unwrap();
        }
        text.push('\n');
        for choice in &self.service.choices {
            if !choice.result {
                writeln!(text, "class {}(Enum):", choice.name).unwrap();
                for name in &choice.variants {
                    writeln!(text, "    {name} = ...").unwrap();
                }
            } else {
                writeln!(text, "class {}(Sum):", choice.name).unwrap();
                for name in &choice.variants {
                    writeln!(
                        text,
                        "    class {name}({}):\n        def __init__(self) -> None: ...",
                        choice.name
                    )
                    .unwrap();
                }
                writeln!(text, "    class Unknown({}):\n        raw_status: int\n        def __init__(self, raw_status: int) -> None: ...", choice.name).unwrap();
            }
            text.push('\n');
        }
        for quantity in &self.service.quantities {
            writeln!(
                text,
                "class {}(Handle[Js{}]):",
                quantity.name, quantity.name
            )
            .unwrap();
            let args = quantity
                .parameters
                .iter()
                .map(|p| {
                    format!(
                        "{}: {}",
                        p.name,
                        if p.input == "bytes" { "Buffer" } else { "int" }
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(text, "    def __init__(self, {args}) -> None: ...").unwrap();
            writeln!(
                text,
                "    @property\n    def {}(self) -> {}: ...\n",
                quantity.getter,
                if quantity.getter == "data" {
                    "bytes"
                } else {
                    "int"
                }
            )
            .unwrap();
        }
        for union in &self.service.unions {
            let extra = if union.input {
                format!(", Handle[Js{}]", union.wasm_name())
            } else {
                String::new()
            };
            writeln!(text, "class {}(Sum{extra}):", union.name).unwrap();
            for variant in &union.variants {
                writeln!(text, "    class {}({}):", variant.name, union.name).unwrap();
                for field in &variant.fields {
                    writeln!(
                        text,
                        "        @property\n        def {}(self) -> {}: ...",
                        field.name,
                        field_type(&field.ty)
                    )
                    .unwrap();
                }
                let args = variant
                    .fields
                    .iter()
                    .map(|field| format!(", {}: {}", field.name, field_type(&field.ty)))
                    .collect::<String>();
                writeln!(text, "        def __init__(self{args}) -> None: ...").unwrap();
            }
            text.push('\n');
        }
        fs::write(out.join("_fujimod_service_values.pyi"), text).map_err(|e| e.to_string())
    }
}
