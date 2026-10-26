//! Build-time language views of Rust value declarations. No USB implementation
//! or copied validation bounds belong here.
use serde::Serialize;
use std::{collections::BTreeMap, fs, path::Path};
mod catalog_types;
mod catalog_typescript;
mod unions;
use unions::Union;

pub use catalog_types::generate as generate_catalog_types;
pub use catalog_typescript::generate as generate_catalog_typescript;

use syn::{Fields, FnArg, ImplItem, Item, ReturnType, Type, Visibility};

#[derive(Clone, Debug, Serialize)]
pub struct Choice {
    pub name: String,
    pub variants: Vec<String>,
    pub result: bool,
    pub kind_name: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct QuantityParameter {
    pub name: String,
    pub input: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct Quantity {
    pub name: String,
    pub parameters: Vec<QuantityParameter>,
    pub getter: String,
    #[serde(skip_serializing)]
    getter_type: String,
    pub fallible: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct Values {
    pub unions: Vec<Union>,
    pub choices: Vec<Choice>,
    pub quantities: Vec<Quantity>,
}
#[derive(Clone, Debug)]
pub struct Api {
    pub service: Values,
    pub codec: Choice,
}
fn named(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(path) if path.qself.is_none() => path.path.get_ident().map(ToString::to_string),
        _ => None,
    }
}
fn input(ty: &Type) -> Option<String> {
    if let Some(name) = named(ty) {
        return match name.as_str() {
            "u8" | "u16" | "u32" | "i8" | "i16" | "i32" | "Address" => Some(name),
            _ => None,
        };
    }
    if let Type::Reference(reference) = ty
        && let Type::Slice(slice) = reference.elem.as_ref()
        && named(&slice.elem).as_deref() == Some("u8")
    {
        return Some("bytes".into());
    }
    None
}
fn scalar(ty: &Type) -> bool {
    matches!(
        named(ty).as_deref(),
        Some("u8" | "u16" | "u32" | "i8" | "i16" | "i32")
    )
}
fn getter_type(ty: &Type) -> Option<String> {
    match named(ty).as_deref() {
        Some("u8" | "u16" | "u32" | "i8" | "i16" | "i32" | "Address") => named(ty),
        _ => None,
    }
}
fn argument(arg: &FnArg) -> Option<(String, String)> {
    let FnArg::Typed(arg) = arg else {
        return None;
    };
    let syn::Pat::Ident(pattern) = arg.pat.as_ref() else {
        return None;
    };
    Some((pattern.ident.to_string(), input(&arg.ty)?))
}
fn choice(item: &syn::ItemEnum) -> Option<Choice> {
    let mut variants = Vec::new();
    let mut result = false;
    for variant in &item.variants {
        match &variant.fields {
            Fields::Unit => variants.push(variant.ident.to_string()),
            Fields::Unnamed(fields)
                if variant.ident == "Unknown"
                    && fields.unnamed.len() == 1
                    && named(&fields.unnamed[0].ty).as_deref() == Some("u8") =>
            {
                result = true;
            }
            _ => return None,
        }
    }
    Some(Choice {
        name: item.ident.to_string(),
        variants,
        result,
        kind_name: result.then(|| kind_name(&item.ident.to_string())),
    })
}
impl Api {
    pub fn read(root: &Path) -> Result<Self, String> {
        let mut sources = Vec::new();
        let file = root.join("crates/usb-app80-types/src/catalog.rs");
        sources.push((
            file.clone(),
            fs::read_to_string(&file).map_err(|e| e.to_string())?,
        ));
        let directory = file.with_extension("");
        let mut paths: Vec<_> = fs::read_dir(directory)
            .map_err(|e| e.to_string())?
            .map(|entry| entry.map(|e| e.path()))
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        paths.sort();
        for path in paths {
            if path.extension().is_some_and(|e| e == "rs")
                && !path.file_stem().unwrap().to_string_lossy().contains("test")
            {
                sources.push((
                    path.clone(),
                    fs::read_to_string(path).map_err(|e| e.to_string())?,
                ));
            }
        }
        let mut api = Self::from_sources(sources.iter().map(|(_, text)| text.as_str()))?;
        let catalog = fs::read_to_string(root.join("crates/catalog-service/src/lib.rs"))
            .map_err(|e| e.to_string())?;
        api.codec = syn::parse_file(&catalog)
            .map_err(|e| e.to_string())?
            .items
            .iter()
            .find_map(|item| {
                if let Item::Enum(item) = item
                    && item.ident == "Codec"
                {
                    return choice(item);
                }
                None
            })
            .ok_or("missing plain Codec enum")?;
        Ok(api)
    }
    pub fn from_sources<'a>(sources: impl IntoIterator<Item = &'a str>) -> Result<Self, String> {
        let mut choices = BTreeMap::new();
        let mut payloads = BTreeMap::new();
        let mut quantities = BTreeMap::new();
        let mut structs = std::collections::BTreeSet::new();
        let mut impls = Vec::new();
        let mut public_names = std::collections::BTreeSet::new();
        for source in sources {
            for item in syn::parse_file(source).map_err(|e| e.to_string())?.items {
                match item {
                    Item::Enum(item) if matches!(item.vis, Visibility::Public(_)) => {
                        if !public_names.insert(item.ident.to_string()) {
                            return Err(format!("duplicate public type {}", item.ident));
                        }
                        if let Some(value) = choice(&item)
                            && item.ident != "Command"
                            && item.ident != "ReplyValue"
                        {
                            choices.insert(value.name.clone(), value);
                        } else if item.ident != "CommandError" {
                            let union = Union::parse(&item)?;
                            payloads.insert(union.name.clone(), union);
                        }
                    }
                    Item::Struct(item) if matches!(item.vis, Visibility::Public(_)) => {
                        if !public_names.insert(item.ident.to_string()) {
                            return Err(format!("duplicate public type {}", item.ident));
                        }
                        structs.insert(item.ident.to_string());
                    }
                    Item::Impl(item) if item.trait_.is_none() => impls.push(item),
                    _ => (),
                }
            }
        }
        let mut methods_by_type = BTreeMap::<String, Vec<syn::ImplItemFn>>::new();
        for item in impls {
            let Some(name) = named(&item.self_ty) else {
                continue;
            };
            if !structs.contains(&name) {
                continue;
            }
            for method in item.items {
                if let ImplItem::Fn(method) = method {
                    methods_by_type
                        .entry(name.clone())
                        .or_default()
                        .push(method);
                }
            }
        }
        for (name, methods) in methods_by_type {
            let Some(new) = methods
                .iter()
                .find(|m| m.sig.ident == "new" && matches!(m.vis, Visibility::Public(_)))
            else {
                continue;
            };
            let Some(mut parameters) = new
                .sig
                .inputs
                .iter()
                .map(argument)
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            if parameters.is_empty()
                || (parameters.len() > 1
                    && new.sig.inputs.iter().any(|arg| {
                        let FnArg::Typed(arg) = arg else {
                            return true;
                        };
                        !scalar(&arg.ty)
                    }))
            {
                continue;
            }
            let single_input = (parameters.len() == 1).then(|| parameters[0].1.as_str());
            let getter = if single_input == Some("bytes") {
                "data"
            } else if single_input == Some("Address") {
                "address"
            } else {
                "value"
            };
            let method = methods
                .iter()
                .find(|m| m.sig.ident == getter && matches!(m.vis, Visibility::Public(_)))
                .ok_or_else(|| format!("{name}: checked value needs public {getter} getter"))?;
            if !matches!(method.sig.output, ReturnType::Type(..)) {
                return Err(format!("{name}: getter has no result"));
            }
            let ReturnType::Type(_, result_type) = &method.sig.output else {
                unreachable!()
            };
            let getter_type = if single_input == Some("bytes") {
                "bytes".into()
            } else {
                getter_type(result_type).ok_or_else(|| {
                    format!("{name}: getter must return a supported integer value")
                })?
            };
            let fallible = match &new.sig.output {
                ReturnType::Type(_, ty) => match ty.as_ref() {
                    Type::Path(path)
                        if path
                            .path
                            .segments
                            .last()
                            .is_some_and(|s| s.ident == "Result") =>
                    {
                        true
                    }
                    ty if named(ty).is_some_and(|n| n == "Self" || n == name) => false,
                    _ => {
                        return Err(format!(
                            "{name}: constructor must return Self or Result<Self, CommandError>"
                        ));
                    }
                },
                _ => {
                    return Err(format!(
                        "{name}: constructor must return Self or Result<Self, CommandError>"
                    ));
                }
            };
            if parameters.len() == 1 {
                let input = parameters[0].1.clone();
                parameters[0].0 = if input == "bytes" {
                    "data".into()
                } else {
                    "value".into()
                };
            }
            quantities.insert(
                name.clone(),
                Quantity {
                    name,
                    parameters: parameters
                        .into_iter()
                        .map(|(name, input)| QuantityParameter { name, input })
                        .collect(),
                    getter: getter.into(),
                    getter_type,
                    fallible,
                },
            );
        }
        let mut service = Values {
            unions: payloads.into_values().collect(),
            choices: choices.into_values().collect(),
            quantities: quantities.into_values().collect(),
        };
        unions::classify(&mut service)?;
        Ok(Self {
            service,
            codec: Choice {
                name: "Codec".into(),
                variants: vec![],
                result: false,
                kind_name: None,
            },
        })
    }
    pub fn write(&self, out: &Path) -> Result<(), String> {
        fs::create_dir_all(out).map_err(|e| e.to_string())?;
        fs::write(
            out.join("service_schema.json"),
            serde_json::to_string_pretty(&self.service).unwrap(),
        )
        .map_err(|e| e.to_string())?;
        let files = [
            ("service_values_python.rs", self.python()),
            ("service_values_javascript.rs", self.javascript()),
            (
                "definition_codec_python.rs",
                python_choice(&self.codec, "fujimod.catalog.service", "pub"),
            ),
            (
                "definition_codec_javascript.rs",
                javascript_codec(&self.codec),
            ),
            ("service-values.js", self.js_exports(false)),
            ("service-values.d.ts", self.js_exports(true)),
        ];
        for (name, contents) in files {
            fs::write(out.join(name), contents).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    pub fn write_web(&self, out: &Path) -> Result<(), String> {
        fs::create_dir_all(out).map_err(|e| e.to_string())?;
        for (name, declaration) in [("service-values.js", false), ("service-values.d.ts", true)] {
            fs::write(out.join(name), self.js_exports(declaration)).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    pub fn build(root: &Path, out: &Path) {
        println!(
            "cargo:rerun-if-changed={}",
            root.join("crates/usb-app80/src").display()
        );
        println!(
            "cargo:rerun-if-changed={}",
            root.join("crates/catalog-service/src/lib.rs").display()
        );
        Self::read(root)
            .and_then(|api| api.write(out))
            .unwrap_or_else(|e| panic!("binding generation failed: {e}"));
    }
    fn python(&self) -> String {
        let mut out = String::from("// Generated from Rust declarations.\nuse super::*;\n");
        for c in &self.service.choices {
            out += &python_choice(c, "fujimod.usb.app80", "pub(super)");
        }
        for q in &self.service.quantities {
            out += &python_quantity(q);
        }
        for union in &self.service.unions {
            out += &union.python(&self.service);
        }
        out += "pub(super) fn register(module:&Bound<'_,PyModule>)->PyResult<()>{\n";
        for name in self
            .service
            .choices
            .iter()
            .map(|c| &c.name)
            .chain(self.service.quantities.iter().map(|q| &q.name))
            .chain(self.service.unions.iter().map(|u| &u.name))
        {
            out += &format!("module.add_class::<{name}>()?;\n");
        }
        out += "Ok(())}\n";
        out
    }
    fn javascript(&self) -> String {
        let mut out = String::from("// Generated from Rust declarations.\nuse super::*;\n");
        let mut types = String::from("declare const __serviceType: unique symbol;\n");
        for c in &self.service.choices {
            let name = if c.result {
                kind_name(&c.name)
            } else {
                c.name.clone()
            };
            let variants = c
                .variants
                .iter()
                .map(String::as_str)
                .chain(c.result.then_some("Unknown"))
                .collect::<Vec<_>>()
                .join(",");
            out +=
                &format!("#[wasm_bindgen] #[derive(Clone,Copy)] pub enum {name} {{{variants}}}\n");
            out += &format!(
                "impl From<core::{}> for {name} {{fn from(value:core::{})->Self {{match value {{",
                c.name, c.name
            );
            for v in &c.variants {
                out += &format!("core::{}::{v}=>Self::{v},", c.name);
            }
            if c.result {
                out += &format!("core::{}::Unknown(_)=>Self::Unknown,", c.name);
            }
            out += "}}}\n";
            if c.result {
                out += &format!(
                    "impl {name} {{pub(super) fn value(value:core::{})->JsValue {{match value {{",
                    c.name
                );
                for v in &c.variants {
                    out += &format!("core::{}::{v}=>record(Self::{v} as u32,&[]),", c.name);
                }
                out += &format!(
                    "core::{}::Unknown(raw)=>record(Self::Unknown as u32,&[(\"rawStatus\",raw.into())]),",
                    c.name
                );
                out += "}}}\n";
                types += &format!(
                    "export type {} = {};\n",
                    c.name,
                    c.variants
                        .iter()
                        .map(|v| format!("Readonly<{{kind:{name}.{v}}}>"))
                        .chain(std::iter::once(format!(
                            "Readonly<{{kind:{name}.Unknown;rawStatus:number}}>"
                        )))
                        .collect::<Vec<_>>()
                        .join(" | ")
                );
            } else if self
                .service
                .unions
                .iter()
                .filter(|u| u.input)
                .flat_map(|u| &u.variants)
                .flat_map(|v| &v.fields)
                .any(|f| matches!(&f.ty,unions::ValueType::Named{name} if *name==c.name))
            {
                out += &format!(
                    "impl {name} {{pub(super) fn parse(value:JsValue)->Result<core::{name},JsValue>{{let value=integer(value,u32::MAX)?;"
                );
                for v in &c.variants {
                    out +=
                        &format!("if value==Self::{v} as u32 {{return Ok(core::{name}::{v});}}\n");
                }
                out += "Err(fail(ErrorKind::InvalidArgument,\"unknown enum choice\"))}}\n";
            }
        }
        for q in &self.service.quantities {
            types += &format!(
                "export interface {} {{readonly [__serviceType]:\"{}\"}}\n",
                q.name, q.name
            );
            out += &javascript_quantity(q);
        }
        for union in &self.service.unions {
            if union.input {
                types += &format!(
                    "export interface {} {{readonly [__serviceType]:\"{}\"}}\n",
                    union.wasm_name(),
                    union.name
                );
            }
            out += &union.javascript(&self.service);
        }
        out += &format!(
            "#[wasm_bindgen(typescript_custom_section)] const GENERATED_VALUE_TYPES:&str=r#\"{types}\"#;\n"
        );
        out
    }
    fn js_exports(&self, declaration: bool) -> String {
        let mut out = String::from(
            "// Generated from Rust declarations.\nimport * as native from './fujimod.js';\n",
        );
        let names: Vec<_> = self
            .service
            .choices
            .iter()
            .map(|c| {
                if c.result {
                    kind_name(&c.name)
                } else {
                    c.name.clone()
                }
            })
            .chain(self.service.quantities.iter().map(|q| q.name.clone()))
            .collect();
        if declaration {
            out += "export declare const serviceValues:Readonly<{\n";
            for n in &names {
                out += &format!("{n}:typeof native.{n};\n");
            }
            for u in &self.service.unions {
                let native = u.wasm_name();
                if u.input {
                    out += &format!("{}:Readonly<{{", u.name);
                    for v in &u.variants {
                        out += &format!("{}:typeof native.{native}.{};", v.factory, v.factory);
                    }
                    out += "}>;\n";
                } else {
                    let name = &u.kind_export;
                    out += &format!("{name}:typeof native.{};\n", u.kind_name);
                }
            }
            out += "}>;\nexport declare const definitionValues:Readonly<{Codec:typeof native.ServiceCodec}>;\nexport declare const valueSchemas:Readonly<{service:string;definitions:string}>;\n";
            for c in &self.service.choices {
                out += &format!("export type {}=native.{};\n", c.name, c.name);
            }
            for q in &self.service.quantities {
                out += &format!("export type {}=native.{};\n", q.name, q.name);
            }
            for u in &self.service.unions {
                out += &format!("export type {}=native.{};\n", u.wasm_name(), u.wasm_name());
            }
        } else {
            out += "export const serviceValues=Object.freeze({\n";
            for n in &names {
                out += &format!("{n}:native.{n},\n");
            }
            for u in &self.service.unions {
                if u.input {
                    out += &format!("{}:Object.freeze({{", u.name);
                    for v in &u.variants {
                        out += &format!("{}:native.{}.{},", v.factory, u.wasm_name(), v.factory);
                    }
                    out += "}),\n";
                } else {
                    let name = &u.kind_export;
                    out += &format!("{name}:native.{},\n", u.kind_name);
                }
            }
            out +=
                "});\nexport const definitionValues=Object.freeze({Codec:native.ServiceCodec});\n";
            let service = serde_json::to_string(&self.service).unwrap();
            let catalog = serde_json::to_string(&Values {
                unions: vec![],
                choices: vec![self.codec.clone()],
                quantities: vec![],
            })
            .unwrap();
            out += &format!(
                "export const valueSchemas=Object.freeze({{service:{},definitions:{}}});\n",
                serde_json::to_string(&service).unwrap(),
                serde_json::to_string(&catalog).unwrap()
            );
        }
        out
    }
}
fn kind_name(name: &str) -> String {
    if name == "WordWriteResult" {
        "WordWriteKind".into()
    } else {
        format!("{name}Kind")
    }
}
fn python_choice(c: &Choice, module: &str, visibility: &str) -> String {
    let n = &c.name;
    let mut out = if c.result {
        format!(
            "#[pyclass(frozen,from_py_object,module=\"{module}\")] #[derive(Clone)] {visibility} enum {n} {{"
        )
    } else {
        format!(
            "#[pyclass(eq,from_py_object,module=\"{module}\")] #[derive(Clone,Copy,Debug,PartialEq)] {visibility} enum {n} {{"
        )
    };
    for v in &c.variants {
        out += &format!("{v}{},", if c.result { "{}" } else { "" });
    }
    if c.result {
        out += "Unknown{raw_status:u8},";
    }
    out += "}\n";
    out +=
        &format!("impl From<core::{n}> for {n} {{fn from(value:core::{n})->Self {{match value {{");
    for v in &c.variants {
        out += &format!(
            "core::{n}::{v}=>Self::{v}{},",
            if c.result { "{}" } else { "" }
        );
    }
    if c.result {
        out += &format!("core::{n}::Unknown(raw_status)=>Self::Unknown{{raw_status}},");
    }
    out += "}}}\n";
    if !c.result {
        out += &format!("impl From<{n}> for core::{n} {{fn from(value:{n})->Self {{match value {{");
        for v in &c.variants {
            out += &format!("{n}::{v}=>Self::{v},");
        }
        out += "}}}\n";
    }
    out
}
fn python_quantity(q: &Quantity) -> String {
    let n = &q.name;
    let multi = q.parameters.len() > 1;
    let bytes = q.parameters.len() == 1 && q.parameters[0].input == "bytes";
    let mut parameters = Vec::new();
    let mut calls = Vec::new();
    for parameter in &q.parameters {
        let name = parameter.name.as_str();
        let ty = parameter.input.as_str();
        let py_type = match ty {
            "bytes" => "Vec<u8>",
            "Address" => "u32",
            other => other,
        };
        parameters.push(format!("{name}:{py_type}"));
        calls.push(match ty {
            "bytes" => format!("&{name}"),
            "Address" => format!("fujimod_usb_app80_types::Address({name})"),
            _ => name.to_owned(),
        });
    }
    let parameters = parameters.join(",");
    let calls = calls.join(",");
    let getter = if bytes {
        "#[getter] fn data<'py>(&self,py:Python<'py>)->Bound<'py,PyBytes>{PyBytes::new(py,self.inner.data())}".into()
    } else {
        let ty = if q.getter_type == "Address" {
            "u32"
        } else {
            &q.getter_type
        };
        let get = if q.getter_type == "Address" {
            format!("self.inner.{}().0", q.getter)
        } else {
            format!("self.inner.{}()", q.getter)
        };
        format!("#[getter] fn value(&self)->{ty}{{{get}}}")
    };
    let constructor = if q.fallible {
        format!(
            "#[new] fn new(py:Python<'_>,{parameters})->PyResult<Self>{{core::{n}::new({calls}).map(|inner|Self{{inner}}).map_err(|e|command_error(py,e))}}"
        )
    } else {
        format!("#[new] fn new({parameters})->Self{{Self{{inner:core::{n}::new({calls})}}}}")
    };
    let allow_non_snake_case = if multi {
        "#[allow(non_snake_case)]"
    } else {
        ""
    };
    format!(
        "#[pyclass(frozen,from_py_object,module=\"fujimod.usb.app80\")] #[derive(Clone)] pub(super) struct {n} {{pub(super) inner:core::{n}}}\nimpl From<core::{n}> for {n}{{fn from(inner:core::{n})->Self{{Self{{inner}}}}}}\n#[pymethods] impl {n} {{{allow_non_snake_case}{constructor} {getter} }}\n"
    )
}
fn javascript_quantity(q: &Quantity) -> String {
    let n = &q.name;
    let multi = q.parameters.len() > 1;
    let bytes = q.parameters.len() == 1 && q.parameters[0].input == "bytes";
    let mut parameters = Vec::new();
    let mut calls = Vec::new();
    for parameter in &q.parameters {
        let ty = parameter.input.as_str();
        let name = if multi {
            camel_parameter(&parameter.name)
        } else {
            "data".to_owned()
        };
        let js_type = if ty == "bytes" {
            "Uint8Array"
        } else {
            "number"
        };
        let checked = match ty {
            "bytes" => format!("&error::buffer({name})?"),
            "Address" => format!(
                "fujimod_usb_app80_types::Address({})",
                checked_integer(&name, "u32")
            ),
            _ => checked_integer(&name, ty),
        };
        parameters.push(format!(
            "#[wasm_bindgen(unchecked_param_type=\"{js_type}\")] {name}:JsValue"
        ));
        calls.push(checked);
    }
    let parameters = parameters.join(",");
    let calls = calls.join(",");
    let body = if q.fallible {
        format!("core::{n}::new({calls}).map(|inner|Self{{inner}}).map_err(invalid)")
    } else {
        format!("Ok(Self{{inner:core::{n}::new({calls})}})")
    };
    let getter = if bytes {
        "#[wasm_bindgen(getter,unchecked_return_type=\"Uint8Array\")] pub fn data(&self)->JsValue{error::bytes(self.inner.data())}".into()
    } else {
        let ty = if q.getter_type == "Address" {
            "u32"
        } else {
            &q.getter_type
        };
        let get = if q.getter_type == "Address" {
            format!("self.inner.{}().0", q.getter)
        } else {
            format!("self.inner.{}()", q.getter)
        };
        format!("#[wasm_bindgen(getter)] pub fn value(&self)->{ty}{{{get}}}")
    };
    let allow_non_snake_case = if multi {
        "#[allow(non_snake_case)]"
    } else {
        ""
    };
    format!(
        "#[wasm_bindgen] pub struct {n}{{pub(super) inner:core::{n}}}\n#[wasm_bindgen] impl {n}{{#[wasm_bindgen(constructor)] {allow_non_snake_case} pub fn new({parameters})->Result<Self,JsValue>{{{body}}} {getter} }}\n"
    )
}
fn checked_integer(name: &str, ty: &str) -> String {
    if ty.starts_with('i') {
        format!("error::signed_integer({name},{ty}::MIN.into(),{ty}::MAX.into())? as {ty}")
    } else if ty == "u32" {
        format!("integer({name},u32::MAX)?")
    } else {
        format!("integer({name},{ty}::MAX.into())? as {ty}")
    }
}
fn camel_parameter(name: &str) -> String {
    let mut parts = name.split('_');
    let mut camel = parts.next().unwrap_or_default().to_owned();
    for part in parts {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            camel.extend(first.to_uppercase());
            camel.extend(chars);
        }
    }
    camel
}
fn javascript_codec(c: &Choice) -> String {
    let mut out = format!(
        "#[wasm_bindgen] #[derive(Clone,Copy)] pub enum ServiceCodec{{{}}}\nimpl From<core::Codec> for ServiceCodec{{fn from(value:core::Codec)->Self{{match value{{",
        c.variants.join(",")
    );
    for v in &c.variants {
        out += &format!("core::Codec::{v}=>Self::{v},");
    }
    out += "}}}\n";
    out
}

mod typing;

#[cfg(test)]
mod tests;
