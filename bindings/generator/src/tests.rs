use super::*;
#[test]
fn new_rust_values_reach_every_language_without_copied_bounds() {
    let source = r#"
 pub enum NewChoice {First,Second}
 pub enum NewResult {Done,Unknown(u8)}
 pub struct NewLevel(u8);
 impl NewLevel {
  pub fn new(value:u8)->Result<Self,CommandError>{if value<42 {Ok(Self(value))}else{panic!()}}
  pub fn value(self)->u8 {self.0}
 }
 enum PrivateChoice {Hidden}
 "#;
    let api = Api::from_sources([source]).unwrap();
    let python = api.python();
    let javascript = api.javascript();
    let exports = api.js_exports(false);
    for code in [&python, &javascript, &exports] {
        assert!(code.contains("NewChoice"));
        assert!(code.contains("NewResult"));
        assert!(code.contains("NewLevel"));
        assert!(!code.contains("PrivateChoice"));
        assert!(!code.contains("42"));
    }
    assert!(python.contains("core::NewLevel::new"));
    assert!(javascript.contains("core::NewLevel::new"));
    assert!(!exports.contains("raw_status")); // Python's runtime supplies the common variant field.
    syn::parse_file(&python).unwrap();
    syn::parse_file(&javascript).unwrap();
    assert_eq!(api.service.quantities[0].parameters[0].input, "u8");
}
#[test]
fn source_order_does_not_change_generated_files() {
    let first = "pub enum Zebra {One,Two}";
    let second = "pub enum Alpha {Three,Four}";
    let a = Api::from_sources([first, second]).unwrap();
    let b = Api::from_sources([second, first]).unwrap();
    assert_eq!(a.python(), b.python());
    assert_eq!(a.javascript(), b.javascript());
    assert_eq!(a.js_exports(false), b.js_exports(false));
}
#[test]
fn checked_values_require_a_public_observer_and_byte_input_is_distinct() {
    assert!(
        Api::from_sources([
            "pub struct Level(u8);impl Level {pub fn new(value:u8)->Self {Self(value)}}"
        ])
        .unwrap_err()
        .contains("getter")
    );
    let api=Api::from_sources(["pub struct Packet([u8;6]);impl Packet {pub fn new(data:&[u8])->Result<Self,Error>{todo!()} pub fn data(&self)->&[u8;6]{&self.0}}"]).unwrap();
    assert_eq!(api.service.quantities[0].parameters[0].input, "bytes");
    assert!(api.python().contains("PyBytes::new"));
    assert!(api.javascript().contains("error::buffer"));
}

#[test]
fn payload_graph_generates_nested_factories_and_structured_replies() {
    let api = Api::from_sources([r#"
        pub enum Choice { One, Two }
        pub enum NewAction { Select { choice: Choice, count: u16 }, Reset }
        pub enum Command { NewFeature { action: NewAction, value: u32 }, Query }
        pub enum Phase { Idle, Done }
        pub enum NewResult { Data { raw_status: u8, values: [u16; 3], phase: Phase }, Unknown(u8) }
        pub enum ReplyValue { NewFeature { result: NewResult, data: Vec<u8> }, Empty }
    "#])
    .unwrap();
    let python = api.python();
    let javascript = api.javascript();
    syn::parse_file(&python).unwrap();
    syn::parse_file(&javascript).unwrap();
    for code in [&python, &javascript] {
        assert!(code.contains("core::Command::NewFeature"));
        assert!(code.contains("core::NewAction::Select"));
        assert!(code.contains("core::ReplyValue::NewFeature"));
        assert!(code.contains("core::NewResult::Data"));
    }
    assert!(
        javascript.contains("export interface App80Command {readonly [__serviceType]:\"Command\"}")
    );
    assert!(
        javascript.contains("export interface NewAction {readonly [__serviceType]:\"NewAction\"}")
    );
    assert!(javascript.contains("integer(count,u16::MAX.into())?"));
    assert!(javascript.contains("Choice::parse(choice)?"));
    assert!(!javascript.contains("impl Phase {pub(super) fn parse"));
    assert!(javascript.contains("readonly [number,number,number]"));
    assert!(javascript.contains("rawStatus:number"));
    assert!(
        api.js_exports(false)
            .contains("newFeature:native.App80Command.newFeature")
    );
    assert!(
        api.js_exports(true)
            .contains("newFeature:typeof native.App80Command.newFeature")
    );
    let schema = serde_json::to_value(&api.service).unwrap();
    let action = schema["unions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["name"] == "NewAction")
        .unwrap();
    assert_eq!(action["input"], true);
    assert_eq!(action["variants"][0]["fields"][1]["ty"]["bits"], 16);
    assert!(
        api.service
            .unions
            .iter()
            .find(|u| u.name == "NewResult")
            .is_some_and(|u| !u.input)
    );
}

#[test]
fn payload_graph_rejects_unclassified_unknown_and_unnamed_fields() {
    for (source, reason) in [
        ("pub enum Lost { Value { byte: u8 } }", "reachable"),
        (
            "pub enum Command { New { unknown: Missing } }",
            "unknown field type",
        ),
        ("pub enum Command { New(u8) }", "named Rust fields"),
        (
            "pub enum Command { New { bytes: Vec<u8> } }",
            "checked Rust value",
        ),
        (
            "pub enum Both { New { value: u8 } } pub enum Command { New { value: Both } } pub enum ReplyValue { New { value: Both } }",
            "exactly one",
        ),
    ] {
        let error = Api::from_sources([source]).unwrap_err();
        assert!(error.contains(reason), "{error}");
    }
}

#[test]
fn split_impls_opaque_results_and_unit_roots_generate_valid_views() {
    let api = Api::from_sources([r#"
        pub struct Level(u8);
        impl Level { pub fn new(value:u8)->Result<Self,Error>{todo!()} }
        impl Level { pub fn value(self)->u8 {self.0} }
        pub enum Status { Unknown(u8) }
        pub enum Command { Query }
        pub enum ReplyValue { Data { level:Level, status:Status, empty:[u16;0] } }
    "#])
    .unwrap();
    for code in [api.python(), api.javascript()] {
        syn::parse_file(&code).unwrap();
    }
    assert!(api.javascript().contains("Level{inner:*level}.into()"));
    assert!(api.python().contains("impl From<core::Level> for Level"));
    assert!(
        api.service
            .unions
            .iter()
            .any(|u| u.name == "Command" && u.input)
    );
    assert!(
        Api::from_sources(["pub enum Repeated { A }", "pub enum Repeated { B }"])
            .unwrap_err()
            .contains("duplicate public type")
    );
}

#[test]
fn infallible_value_constructors_preserve_core_signatures() {
    let api=Api::from_sources(["pub struct FullWord(u32);impl FullWord {pub const fn new(value:u32)->Self{Self(value)} pub fn value(self)->u32{self.0}}"] ).unwrap();
    assert!(!api.service.quantities[0].fallible);
    assert!(api.python().contains("fn new(value:u32)->Self"));
    assert!(
        api.javascript()
            .contains("Ok(Self{inner:core::FullWord::new(integer(data,u32::MAX)?)})")
    );
    syn::parse_file(&api.python()).unwrap();
    syn::parse_file(&api.javascript()).unwrap();
}

#[test]
fn signed_numbers_generate_checked_factories_and_numeric_reply_arrays() {
    let api = Api::from_sources([r#"
        pub struct SignedLevel(i16);
        impl SignedLevel {
            pub fn new(value:i16)->Self {Self(value)}
            pub fn value(self)->i16 {self.0}
        }
        pub enum Command { Set { a:i8, b:i16, c:i32, level:SignedLevel } }
        pub enum ReplyValue { Data { a:i8, b:i16, c:i32, samples:[i8;3] } }
    "#])
    .unwrap();
    let python = api.python();
    let javascript = api.javascript();
    syn::parse_file(&python).unwrap();
    syn::parse_file(&javascript).unwrap();
    for bits in [8, 16, 32] {
        assert!(javascript.contains(&format!("i{bits}::MIN.into(),i{bits}::MAX.into()")));
    }
    assert!(python.contains("samples:(i8,i8,i8,)"));
    assert!(javascript.contains("readonly [number,number,number]"));
    let schema = serde_json::to_value(&api.service).unwrap();
    assert_eq!(
        schema["unions"][0]["variants"][0]["fields"][0]["ty"]["signed"],
        true
    );
}

#[test]
fn multiple_integer_quantity_constructors_keep_names_and_validate_each_argument() {
    let api = Api::from_sources([r#"
        pub struct ScanSelection { index:u8, observed_count:u8 }
        impl ScanSelection {
            pub fn new(index:u8, observed_count:u8)->Result<Self,CommandError> {
                Ok(Self { index, observed_count })
            }
            pub fn value(&self)->u8 { self.index }
        }
    "#])
    .unwrap();
    let python = api.python();
    let javascript = api.javascript();
    syn::parse_file(&python).unwrap();
    syn::parse_file(&javascript).unwrap();
    assert!(python.contains("fn new(py:Python<'_>,index:u8,observed_count:u8)->PyResult<Self>"));
    assert!(javascript.contains(
        "pub fn new(#[wasm_bindgen(unchecked_param_type=\"number\")] index:JsValue,#[wasm_bindgen(unchecked_param_type=\"number\")] observedCount:JsValue)"
    ));
    assert!(javascript.contains(
        "integer(index,u8::MAX.into())? as u8,integer(observedCount,u8::MAX.into())? as u8"
    ));
    assert!(javascript.contains("pub fn value(&self)->u8{self.inner.value()}"));
    let schema = serde_json::to_value(&api.service).unwrap();
    let quantity = &schema["quantities"][0];
    assert_eq!(quantity["name"], "ScanSelection");
    assert_eq!(quantity["parameters"][0]["name"], "index");
    assert_eq!(quantity["parameters"][0]["input"], "u8");
    assert_eq!(quantity["parameters"][1]["name"], "observed_count");
    assert_eq!(quantity["parameters"][1]["input"], "u8");
}

#[test]
fn python_types_include_checked_inputs_and_nested_result_fields() {
    let api = Api::from_sources([r#"
        pub enum NewChoice {First,Second}
        pub enum NewResult {Done,Unknown(u8)}
        pub struct NewBytes(Vec<u8>);
        impl NewBytes {
            pub fn new(data:&[u8])->Result<Self,CommandError>{panic!()}
            pub fn data(&self)->&[u8] { &self.0 }
        }
    "#])
    .unwrap();
    let out = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/generator-tests")
        .join(format!("python-{}", std::process::id()));
    api.write_python_types(&out).unwrap();
    let path = out.join("_fujimod_service_values.pyi");
    let first = fs::read_to_string(&path).unwrap();
    assert!(first.contains("class NewChoice(Enum):"));
    assert!(first.contains("class Unknown(NewResult):\n        raw_status: int"));
    assert!(first.contains("def __init__(self, data: Buffer) -> None"));
    assert!(first.contains("def data(self) -> bytes"));
    api.write_python_types(&out).unwrap();
    assert_eq!(first, fs::read_to_string(&path).unwrap());
    fs::remove_dir_all(&out).unwrap();
}
