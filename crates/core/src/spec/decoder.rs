use crate::error::{GratError, GratResult};
use serde::{Deserialize, Serialize};
use stellar_xdr::curr::{Limited, Limits, ReadXdr, ScSpecEntry, ScSpecTypeDef, ScSpecUdtStructV0};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractErrorEntry {
    pub code: u32,

    pub name: String,

    pub doc: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractFunction {
    pub name: String,

    pub params: Vec<(String, String)>,

    pub return_type: String,

    pub doc: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub return_type_def: Option<ScSpecTypeDef>,

    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub param_defs: Vec<(String, ScSpecTypeDef)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractStructField {
    pub name: String,

    pub type_name: String,

    pub doc: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub type_def: Option<ScSpecTypeDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractStructDef {
    pub name: String,

    pub fields: Vec<ContractStructField>,

    pub doc: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractEnumCase {
    pub name: String,

    pub value: u32,

    pub doc: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractEnumDef {
    pub name: String,

    pub cases: Vec<ContractEnumCase>,

    pub doc: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractUnionCase {
    pub name: String,

    pub doc: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub value_types: Option<Vec<ScSpecTypeDef>>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub fields: Option<Vec<ContractStructField>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractUnionDef {
    pub name: String,

    pub cases: Vec<ContractUnionCase>,

    pub doc: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractSpec {
    pub errors: Vec<ContractErrorEntry>,

    pub functions: Vec<ContractFunction>,

    pub structs: Vec<ContractStructDef>,

    pub name: Option<String>,

    pub version: Option<String>,

    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub enums: Vec<ContractEnumDef>,

    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub unions: Vec<ContractUnionDef>,
}

pub fn decode_contract_spec(wasm_bytes: &[u8]) -> GratResult<ContractSpec> {
    let raw_spec = match SpecParser::extract_spec(wasm_bytes) {
        Ok(bytes) => bytes,
        Err(_) => {
            return Ok(ContractSpec {
                errors: Vec::new(),
                functions: Vec::new(),
                structs: Vec::new(),
                enums: Vec::new(),
                unions: Vec::new(),
                name: None,
                version: None,
            });
        }
    };

    let mut errors = Vec::new();
    let mut functions = Vec::new();
    let mut structs = Vec::new();
    let mut enums = Vec::new();
    let mut unions = Vec::new();

    let cursor = std::io::Cursor::new(&raw_spec);
    let mut limited = Limited::new(cursor, Limits::none());
    loop {
        match ScSpecEntry::read_xdr(&mut limited) {
            Ok(entry) => match entry {
                ScSpecEntry::FunctionV0(func) => {
                    let func_name = func.name.to_string();
                    let doc = if func.doc.is_empty() {
                        None
                    } else {
                        Some(func.doc.to_string())
                    };

                    let mut params = Vec::new();
                    let mut param_defs = Vec::new();
                    for input in func.inputs.iter() {
                        let param_name = input.name.to_string();
                        let param_type = format_type_def(&input.type_);
                        params.push((param_name.clone(), param_type));
                        param_defs.push((param_name, input.type_.clone()));
                    }

                    let return_type_def = if func.outputs.is_empty() {
                        Some(ScSpecTypeDef::Void)
                    } else {
                        Some(func.outputs[0].clone())
                    };

                    let return_type = if func.outputs.is_empty() {
                        "Void".to_string()
                    } else {
                        format_type_def(&func.outputs[0])
                    };

                    functions.push(ContractFunction {
                        name: func_name,
                        params,
                        return_type,
                        doc,
                        return_type_def,
                        param_defs,
                    });
                }
                ScSpecEntry::UdtErrorEnumV0(err_enum) => {
                    let enum_name = err_enum.name.to_string();
                    let doc = if err_enum.doc.is_empty() {
                        None
                    } else {
                        Some(err_enum.doc.to_string())
                    };
                    for case in err_enum.cases.iter() {
                        let case_doc = if case.doc.is_empty() {
                            doc.clone()
                        } else {
                            Some(case.doc.to_string())
                        };
                        errors.push(ContractErrorEntry {
                            code: case.value,
                            name: format!("{}::{}", enum_name, case.name),
                            doc: case_doc,
                        });
                    }
                }
                ScSpecEntry::UdtEnumV0(enum_spec) => {
                    let enum_name = enum_spec.name.to_string();
                    let doc = if enum_spec.doc.is_empty() {
                        None
                    } else {
                        Some(enum_spec.doc.to_string())
                    };
                    let mut cases = Vec::new();
                    for case in enum_spec.cases.iter() {
                        let case_doc = if case.doc.is_empty() {
                            None
                        } else {
                            Some(case.doc.to_string())
                        };
                        cases.push(ContractEnumCase {
                            name: case.name.to_string(),
                            value: case.value,
                            doc: case_doc,
                        });
                    }
                    enums.push(ContractEnumDef {
                        name: enum_name,
                        cases,
                        doc,
                    });
                }
                ScSpecEntry::UdtUnionV0(union_spec) => {
                    let union_name = union_spec.name.to_string();
                    let doc = if union_spec.doc.is_empty() {
                        None
                    } else {
                        Some(union_spec.doc.to_string())
                    };
                    let mut cases = Vec::new();
                    for case in union_spec.cases.iter() {
                        match case {
                            stellar_xdr::curr::ScSpecUdtUnionCaseV0::VoidV0(c) => {
                                let case_doc = if c.doc.is_empty() {
                                    None
                                } else {
                                    Some(c.doc.to_string())
                                };
                                cases.push(ContractUnionCase {
                                    name: c.name.to_string(),
                                    doc: case_doc,
                                    value_types: None,
                                    fields: None,
                                });
                            }
                            stellar_xdr::curr::ScSpecUdtUnionCaseV0::TupleV0(c) => {
                                let case_doc = if c.doc.is_empty() {
                                    None
                                } else {
                                    Some(c.doc.to_string())
                                };
                                let value_types: Vec<ScSpecTypeDef> =
                                    c.type_.iter().cloned().collect();
                                cases.push(ContractUnionCase {
                                    name: c.name.to_string(),
                                    doc: case_doc,
                                    value_types: Some(value_types),
                                    fields: None,
                                });
                            }
                        }
                    }
                    unions.push(ContractUnionDef {
                        name: union_name,
                        cases,
                        doc,
                    });
                }
                ScSpecEntry::UdtStructV0(struct_spec) => {
                    let struct_name = struct_spec.name.to_string();
                    let doc = if struct_spec.doc.is_empty() {
                        None
                    } else {
                        Some(struct_spec.doc.to_string())
                    };

                    let mut fields = Vec::new();
                    for field in struct_spec.fields.iter() {
                        let field_name = field.name.to_string();
                        let field_type = format_type_def(&field.type_);
                        let field_doc = if field.doc.is_empty() {
                            None
                        } else {
                            Some(field.doc.to_string())
                        };
                        fields.push(ContractStructField {
                            name: field_name,
                            type_name: field_type,
                            doc: field_doc,
                            type_def: Some(field.type_.clone()),
                        });
                    }
                    structs.push(ContractStructDef {
                        name: struct_name,
                        fields,
                        doc,
                    });
                }
            },
            Err(_) => break,
        }
    }

    Ok(ContractSpec {
        errors,
        functions,
        structs,
        enums,
        unions,
        name: None,
        version: None,
    })
}

fn format_type_def(type_def: &ScSpecTypeDef) -> String {
    match type_def {
        ScSpecTypeDef::Val => "Val".to_string(),
        ScSpecTypeDef::Bool => "Bool".to_string(),
        ScSpecTypeDef::Void => "Void".to_string(),
        ScSpecTypeDef::Error => "Error".to_string(),
        ScSpecTypeDef::U32 => "U32".to_string(),
        ScSpecTypeDef::I32 => "I32".to_string(),
        ScSpecTypeDef::U64 => "U64".to_string(),
        ScSpecTypeDef::I64 => "I64".to_string(),
        ScSpecTypeDef::Timepoint => "Timepoint".to_string(),
        ScSpecTypeDef::Duration => "Duration".to_string(),
        ScSpecTypeDef::U128 => "U128".to_string(),
        ScSpecTypeDef::I128 => "I128".to_string(),
        ScSpecTypeDef::U256 => "U256".to_string(),
        ScSpecTypeDef::I256 => "I256".to_string(),
        ScSpecTypeDef::Bytes => "Bytes".to_string(),
        ScSpecTypeDef::BytesN(b) => format!("BytesN<{}>", b.n),
        ScSpecTypeDef::String => "String".to_string(),
        ScSpecTypeDef::Symbol => "Symbol".to_string(),
        ScSpecTypeDef::Address => "Address".to_string(),
        ScSpecTypeDef::Option(opt) => format!("Option<{}>", format_type_def(&opt.value_type)),
        ScSpecTypeDef::Result(res) => format!(
            "Result<{}, {}>",
            format_type_def(&res.ok_type),
            format_type_def(&res.error_type)
        ),
        ScSpecTypeDef::Vec(vec) => format!("Vec<{}>", format_type_def(&vec.element_type)),
        ScSpecTypeDef::Map(map) => format!(
            "Map<{}, {}>",
            format_type_def(&map.key_type),
            format_type_def(&map.value_type)
        ),
        ScSpecTypeDef::Tuple(tuple) => {
            let elements: Vec<String> = tuple.value_types.iter().map(format_type_def).collect();
            format!("({})", elements.join(", "))
        }
        ScSpecTypeDef::Udt(udt) => udt.name.to_string(),
    }
}

pub struct SpecParser;

impl SpecParser {
    pub fn extract_spec(wasm_bytes: &[u8]) -> GratResult<Vec<u8>> {
        Self::extract_raw_section(wasm_bytes, "contractspecv0")
    }

    pub fn extract_raw_section(wasm_bytes: &[u8], section_name: &str) -> GratResult<Vec<u8>> {
        let parser = wasmparser::Parser::new(0);
        for payload in parser.parse_all(wasm_bytes) {
            let payload = match payload {
                Ok(p) => p,
                Err(_) => {
                    continue;
                }
            };

            if let wasmparser::Payload::CustomSection(section) = payload {
                if section.name() == section_name {
                    return Ok(section.data().to_vec());
                }
            }
        }

        Err(GratError::SpecError(format!(
            "{section_name} custom section not found"
        )))
    }

    pub fn extract_structs(wasm_bytes: &[u8]) -> GratResult<Vec<ContractStructDef>> {
        let raw_spec = match Self::extract_spec(wasm_bytes) {
            Ok(bytes) => bytes,
            Err(_) => return Ok(Vec::new()),
        };

        let mut structs = Vec::new();
        let cursor = std::io::Cursor::new(&raw_spec);
        let mut limited = Limited::new(cursor, Limits::none());

        loop {
            match ScSpecEntry::read_xdr(&mut limited) {
                Ok(entry) => {
                    if let ScSpecEntry::UdtStructV0(struct_spec) = entry {
                        let struct_name = struct_spec.name.to_string();
                        let doc = if struct_spec.doc.is_empty() {
                            None
                        } else {
                            Some(struct_spec.doc.to_string())
                        };

                        let mut fields = Vec::new();
                        for field in struct_spec.fields.iter() {
                            let field_name = field.name.to_string();
                            let field_type = format_type_def(&field.type_);
                            let field_doc = if field.doc.is_empty() {
                                None
                            } else {
                                Some(field.doc.to_string())
                            };
                            fields.push(ContractStructField {
                                name: field_name,
                                type_name: field_type,
                                doc: field_doc,
                                type_def: Some(field.type_.clone()),
                            });
                        }

                        structs.push(ContractStructDef {
                            name: struct_name,
                            fields,
                            doc,
                        });
                    }
                }
                Err(_) => break,
            }
        }

        Ok(structs)
    }

    pub fn extract_raw_structs(wasm_bytes: &[u8]) -> GratResult<Vec<ScSpecUdtStructV0>> {
        let raw_spec = match Self::extract_spec(wasm_bytes) {
            Ok(bytes) => bytes,
            Err(_) => return Ok(Vec::new()),
        };

        let mut structs = Vec::new();
        let cursor = std::io::Cursor::new(&raw_spec);
        let mut limited = Limited::new(cursor, Limits::none());

        loop {
            match ScSpecEntry::read_xdr(&mut limited) {
                Ok(entry) => {
                    if let ScSpecEntry::UdtStructV0(struct_spec) = entry {
                        structs.push(struct_spec);
                    }
                }
                Err(_) => break,
            }
        }

        Ok(structs)
    }
}

pub fn resolve_error_code(spec: &ContractSpec, error_code: u32) -> Option<&ContractErrorEntry> {
    spec.errors.iter().find(|e| e.code == error_code)
}

#[cfg(test)]
fn build_wasm_with_custom_section(section_name: &str, section_data: &[u8]) -> Vec<u8> {
    let mut wasm = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];
    let mut custom_payload = Vec::new();
    custom_payload.push(section_name.len() as u8);
    custom_payload.extend_from_slice(section_name.as_bytes());
    custom_payload.extend_from_slice(section_data);
    wasm.push(0);
    wasm.push(custom_payload.len() as u8);
    wasm.extend(custom_payload);
    wasm
}

#[cfg(test)]
fn make_struct_spec_entry(
    name: &str,
    doc: &str,
    fields: Vec<(&str, &str, ScSpecTypeDef)>,
) -> ScSpecEntry {
    use stellar_xdr::curr::{ScSpecUdtStructFieldV0, ScSpecUdtStructV0};

    let struct_fields: Vec<ScSpecUdtStructFieldV0> = fields
        .into_iter()
        .map(|(fname, fdoc, ftype)| ScSpecUdtStructFieldV0 {
            doc: fdoc.try_into().unwrap(),
            name: fname.try_into().unwrap(),
            type_: ftype,
        })
        .collect();

    ScSpecEntry::UdtStructV0(ScSpecUdtStructV0 {
        doc: doc.try_into().unwrap(),
        lib: "".try_into().unwrap(),
        name: name.try_into().unwrap(),
        fields: struct_fields.try_into().unwrap(),
    })
}

#[cfg(test)]
fn make_wasm_with_structs(structs: Vec<ScSpecEntry>) -> Vec<u8> {
    use stellar_xdr::curr::{Limits, WriteXdr};
    let mut section_data = Vec::new();
    for entry in structs {
        let bytes = entry.to_xdr(Limits::none()).unwrap();
        section_data.extend_from_slice(&bytes);
    }
    build_wasm_with_custom_section("contractspecv0", &section_data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_error_code_not_found() {
        let spec = ContractSpec {
            errors: vec![ContractErrorEntry {
                code: 1,
                name: "NotFound".to_string(),
                doc: None,
            }],
            functions: Vec::new(),
            structs: Vec::new(),
            enums: Vec::new(),
            unions: Vec::new(),
            name: None,
            version: None,
        };
        assert!(resolve_error_code(&spec, 99).is_none());
        assert!(resolve_error_code(&spec, 1).is_some());
    }

    #[test]
    fn test_extract_spec_success() {
        let section_data = vec![1, 2, 3, 4];
        let wasm = build_wasm_with_custom_section("contractspecv0", &section_data);
        let result = SpecParser::extract_spec(&wasm).expect("Should find section");
        assert_eq!(result, section_data);
    }

    #[test]
    fn test_extract_spec_not_found() {
        let wasm = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];
        let result = SpecParser::extract_spec(&wasm);
        assert!(result.is_err());
        match result {
            Err(GratError::SpecError(msg)) => assert!(msg.contains("not found")),
            _ => panic!("Expected SpecError"),
        }
    }

    #[test]
    fn test_extract_raw_section_custom_name() {
        let section_data = vec![10, 20, 30];
        let wasm = build_wasm_with_custom_section("contractenvmetav0", &section_data);
        let result = SpecParser::extract_raw_section(&wasm, "contractenvmetav0")
            .expect("Should find section");
        assert_eq!(result, section_data);
    }

    #[test]
    fn test_extract_raw_section_not_found() {
        let wasm = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];
        let result = SpecParser::extract_raw_section(&wasm, "nonexistent");
        assert!(result.is_err());
        match result {
            Err(GratError::SpecError(msg)) => assert!(msg.contains("nonexistent")),
            _ => panic!("Expected SpecError"),
        }
    }

    #[test]
    fn test_extract_structs_returns_empty_on_missing_section() {
        let wasm = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];
        let result = SpecParser::extract_structs(&wasm).expect("Should not error");
        assert!(result.is_empty());
    }

    #[test]
    fn test_extract_structs_handles_empty_section() {
        let wasm = build_wasm_with_custom_section("contractspecv0", &[]);
        let result = SpecParser::extract_structs(&wasm).expect("Should not error on empty");
        assert!(result.is_empty());
    }

    #[test]
    fn test_extract_structs_gracefully_handles_malformed_xdr() {
        let wasm = build_wasm_with_custom_section("contractspecv0", &[0xFF, 0xFE, 0xFD, 0xFC]);
        let result = SpecParser::extract_structs(&wasm).expect("Should handle malformed XDR");
        assert!(result.is_empty());
    }

    #[test]
    fn test_extract_structs_parses_single_struct() {
        let entry = make_struct_spec_entry(
            "Balance",
            "A user balance",
            vec![
                ("amount", "The amount", ScSpecTypeDef::I128),
                ("asset", "The asset code", ScSpecTypeDef::Symbol),
            ],
        );
        let wasm = make_wasm_with_structs(vec![entry]);
        let result = SpecParser::extract_structs(&wasm).expect("Should parse struct");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "Balance");
        assert_eq!(result[0].doc.as_deref(), Some("A user balance"));
        assert_eq!(result[0].fields.len(), 2);
        assert_eq!(result[0].fields[0].name, "amount");
        assert_eq!(result[0].fields[0].type_name, "I128");
        assert_eq!(result[0].fields[1].name, "asset");
        assert_eq!(result[0].fields[1].type_name, "Symbol");
    }

    #[test]
    fn test_extract_structs_skips_non_struct_entries() {
        let entry = ScSpecEntry::FunctionV0(stellar_xdr::curr::ScSpecFunctionV0 {
            name: "hello".try_into().unwrap(),
            doc: "".try_into().unwrap(),
            inputs: vec![].try_into().unwrap(),
            outputs: vec![].try_into().unwrap(),
        });
        let wasm = make_wasm_with_structs(vec![entry]);
        let result = SpecParser::extract_structs(&wasm).expect("Should skip non-struct");
        assert!(result.is_empty());
    }

    #[test]
    fn test_extract_raw_structs_returns_sc_spec_udt_struct_v0() {
        let entry = make_struct_spec_entry("Voter", "", vec![("name", "", ScSpecTypeDef::String)]);
        let wasm = make_wasm_with_structs(vec![entry]);
        let result = SpecParser::extract_raw_structs(&wasm).expect("Should extract raw structs");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name.to_string(), "Voter");
    }

    #[test]
    fn test_extract_raw_structs_returns_empty_on_missing_section() {
        let wasm = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];
        let result = SpecParser::extract_raw_structs(&wasm).expect("Should not error");
        assert!(result.is_empty());
    }

    #[test]
    fn test_decode_contract_spec_includes_structs() {
        let struct_entry = make_struct_spec_entry(
            "Config",
            "Contract configuration",
            vec![("admin", "", ScSpecTypeDef::Address)],
        );
        let wasm = make_wasm_with_structs(vec![struct_entry]);
        let spec = decode_contract_spec(&wasm).expect("Should decode spec");
        assert_eq!(spec.structs.len(), 1);
        assert_eq!(spec.structs[0].name, "Config");
    }

    #[test]
    fn test_decode_contract_spec_returns_empty_on_missing_section() {
        let wasm = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];
        let spec = decode_contract_spec(&wasm).expect("Should not error");
        assert!(spec.structs.is_empty());
        assert!(spec.functions.is_empty());
        assert!(spec.errors.is_empty());
    }

    #[test]
    fn test_decode_contract_spec_handles_malformed_xdr_gracefully() {
        let wasm = build_wasm_with_custom_section("contractspecv0", &[0xFF; 32]);
        let spec = decode_contract_spec(&wasm).expect("Should handle malformed XDR");
        assert!(spec.structs.is_empty());
    }
}
