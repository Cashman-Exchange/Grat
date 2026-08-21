use crate::decode::host_error::classify_error;
use crate::decode::report::build_report;
use crate::error::GratResult;
use crate::network::config::NetworkConfig;
use crate::rpc::SorobanRpcClient;
use crate::types::report::DiagnosticReport;
use crate::xdr::codec::XdrCodec;
use stellar_xdr::curr::{
    ContractEvent, ContractEventBody, DiagnosticEvent, Operation, OperationBody, OperationResult,
    OperationResultTr, SorobanTransactionMeta, SorobanTransactionMetaExt, TransactionEnvelope,
    TransactionMeta, TransactionMetaV3, TransactionResult, TransactionResultResult,
    TransactionV1Envelope,
};
use stellar_xdr::curr::{FeeBumpTransactionInnerTx, ScVal};

struct OperationResultInfo {
    function_name: Option<String>,
    arguments: Vec<String>,
    return_value: Option<String>,
    is_success: bool,
    error_category: Option<String>,
    error_name: Option<String>,
}

pub struct MultiOpDecoder;

impl MultiOpDecoder {
    pub fn new() -> Self {
        Self
    }

    pub fn decode_transaction(
        &self,
        tx_data: &serde_json::Value,
    ) -> GratResult<Vec<DiagnosticReport>> {
        let envelope_xdr = tx_data
            .get("envelopeXdr")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                crate::error::GratError::Internal(
                    "Missing envelopeXdr in transaction data".to_string(),
                )
            })?;

        let envelope =
            <TransactionEnvelope as XdrCodec>::from_xdr_base64(envelope_xdr).map_err(|e| {
                crate::error::GratError::Internal(format!("Failed to decode envelope XDR: {}", e))
            })?;

        let num_ops = match &envelope {
            TransactionEnvelope::Tx(TransactionV1Envelope { tx, .. }) => tx.operations.len(),
            TransactionEnvelope::TxFeeBump(fb) => match &fb.tx.inner_tx {
                FeeBumpTransactionInnerTx::Tx(TransactionV1Envelope { tx, .. }) => {
                    tx.operations.len()
                }
            },
            TransactionEnvelope::TxV0(_) => 1,
        };

        let result_xdr = tx_data
            .get("resultXdr")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                crate::error::GratError::Internal(
                    "Missing resultXdr in transaction data".to_string(),
                )
            })?;

        let tx_result =
            <TransactionResult as XdrCodec>::from_xdr_base64(result_xdr).map_err(|e| {
                crate::error::GratError::Internal(format!("Failed to decode result XDR: {}", e))
            })?;

        let op_results = match tx_result.result {
            TransactionResultResult::TxSuccess(ops) => ops,
            TransactionResultResult::TxFailed(ops) => ops,
            TransactionResultResult::TxFeeBumpInnerSuccess(_) => {
                return Ok(vec![build_report(&classify_error(tx_data)?).map_err(
                    |e| crate::error::GratError::Internal(format!("{}", e)),
                )?])
            }
            _ => {
                return Err(crate::error::GratError::NotSorobanTransaction.into());
            }
        };

        let meta_xdr = tx_data
            .get("resultMetaXdr")
            .and_then(|v| v.as_str())
            .map(|xdr| {
                <TransactionMeta as XdrCodec>::from_xdr_base64(xdr).map_err(|e| {
                    crate::error::GratError::Internal(format!("Failed to decode meta XDR: {}", e))
                })
            })
            .transpose()?;

        let soroban_meta = meta_xdr.and_then(|meta| match meta {
            TransactionMeta::V3(v3) => v3.soroban_meta,
            TransactionMeta::V0(_) => None,
            TransactionMeta::V1(_) => None,
            TransactionMeta::V2(_) => None,
        });

        let all_diagnostic_events = soroban_meta
            .as_ref()
            .map(|sm| sm.diagnostic_events.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();

        let all_contract_events = soroban_meta
            .as_ref()
            .map(|sm| sm.events.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();

        let overall_resources =
            crate::decode::resource_analyzer::TransactionResultMeta::from_tx_data(tx_data);
        let overall_resource_summary = crate::types::report::ResourceSummary {
            cpu_instructions_used: overall_resources.resources_consumed.cpu_instructions,
            cpu_instructions_limit: overall_resources.resources_allocated.cpu_instructions,
            memory_bytes_used: overall_resources.resources_consumed.memory_bytes,
            memory_bytes_limit: overall_resources.resources_allocated.memory_bytes,
            read_bytes: overall_resources.resources_consumed.read_bytes,
            read_bytes_limit: overall_resources.resources_allocated.read_bytes,
            write_bytes: overall_resources.resources_consumed.write_bytes,
        };

        let overall_fee = crate::decode::fee_analyzer::analyze_fee_breakdown(tx_data);

        let operation_results = decode_operation_results(&envelope, &op_results, num_ops);

        let operation_event_partitions =
            partition_events_by_operation(&all_diagnostic_events, num_ops);

        let operation_contract_partitions =
            partition_contract_events_by_operation(&all_contract_events, num_ops);

        let mut reports = Vec::new();

        for i in 0..num_ops {
            let op_info = &operation_results[i];
            let op_events = operation_event_partitions
                .get(i)
                .cloned()
                .unwrap_or_default();
            let op_contract_events = operation_contract_partitions
                .get(i)
                .cloned()
                .unwrap_or_default();

            let error_category = op_info
                .error_category
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            let error_name = op_info
                .error_name
                .clone()
                .unwrap_or_else(|| "Unknown".to_string());

            let mut report = if op_info.is_success {
                DiagnosticReport::new(
                    &error_category,
                    0,
                    &error_name,
                    &format!("Operation {} succeeded", i + 1),
                )
            } else {
                DiagnosticReport::new(
                    &error_category,
                    0,
                    &error_name,
                    &format!("Operation {} failed", i + 1),
                )
            };

            report.transaction_context = Some(crate::types::report::TransactionContext {
                tx_hash: tx_data
                    .get("hash")
                    .and_then(|h| h.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                ledger_sequence: tx_data.get("ledger").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                function_name: op_info.function_name.clone(),
                arguments: op_info.arguments.clone(),
                return_value: op_info.return_value.clone(),
                fee: overall_fee.clone(),
                resources: overall_resource_summary.clone(),
                operation_index: Some(i),
                operation_count: Some(num_ops),
            });

            if !op_events.is_empty() {
                let op_events_xdr: Vec<String> = op_events
                    .iter()
                    .filter_map(|e| XdrCodec::to_xdr_base64(e).ok())
                    .collect();

                let enriched = serde_json::json!({
                    "diagnosticEventsXdr": op_events_xdr,
                    "hash": tx_data.get("hash"),
                    "ledger": tx_data.get("ledger"),
                });
                enrich_diagnostic_report(&mut report, &enriched).ok();
            }

            let resource_json = serde_json::json!({
                "diagnosticEvents": overall_resources.is_budget_error.then(|| {
                    serde_json::json!({"type":"budget","data":{"category":"cpu","used":overall_resources.resources_consumed.cpu_instructions,"limit":overall_resources.resources_allocated.cpu_instructions}})
                }).into_iter().collect::<Vec<_>>(),
                "resourcesAllocated": serde_json::json!({
                    "cpuInstructions": overall_resources.resources_allocated.cpu_instructions,
                    "memoryBytes": overall_resources.resources_allocated.memory_bytes,
                    "readBytes": overall_resources.resources_allocated.read_bytes,
                    "writeBytes": overall_resources.resources_allocated.write_bytes,
                }),
                "resourcesConsumed": serde_json::json!({
                    "cpuInstructions": overall_resources.resources_consumed.cpu_instructions,
                    "memoryBytes": overall_resources.resources_consumed.memory_bytes,
                    "readBytes": overall_resources.resources_consumed.read_bytes,
                    "writeBytes": overall_resources.resources_consumed.write_bytes,
                }),
                "status": if op_info.is_success { "SUCCESS" } else { "FAILED" },
            });
            enrich_resource_report(&mut report, &resource_json).ok();

            report.cross_contract_attribution = if !op_contract_events.is_empty() {
                Some(crate::types::report::FailureAttribution {
                    contract_address: op_contract_events
                        .iter()
                        .filter_map(|e| e.contract_id.as_ref().map(|h| hex::encode(&h.0)))
                        .next()
                        .unwrap_or_default(),
                    function_name: op_info.function_name.clone(),
                    call_depth: 0,
                    origin_description: format!("Operation {}", i + 1),
                })
            } else {
                None
            };

            reports.push(report);
        }

        Ok(reports)
    }
}

fn decode_operation_results(
    envelope: &TransactionEnvelope,
    op_results: &[OperationResult],
    num_ops: usize,
) -> Vec<OperationResultInfo> {
    let mut results = Vec::with_capacity(num_ops);

    for i in 0..num_ops {
        let op = get_operation(envelope, i);
        let op_result = op_results.get(i).cloned().unwrap_or_else(|| {
            OperationResult::OpInner(OperationResultTr::InvokeHostFunction(
                stellar_xdr::curr::InvokeHostFunctionResult::Success(stellar_xdr::curr::Hash(
                    [0; 32],
                )),
            ))
        });

        let info = match &op_result {
            OperationResult::OpInner(tr) => match tr {
                OperationResultTr::InvokeHostFunction(inv_result) => match inv_result {
                    stellar_xdr::curr::InvokeHostFunctionResult::Success(hash) => {
                        let (fname, args, ret_val) = op
                            .as_ref()
                            .and_then(|o| {
                                if let OperationBody::InvokeHostFunction(invoke) = &o.body {
                                    match &invoke.host_function {
                                        stellar_xdr::curr::HostFunction::InvokeContract(args) => {
                                            let fname = args.function_name.to_string();
                                            let arguments = args
                                                .args
                                                .iter()
                                                .map(|a| format!("{a:?}"))
                                                .collect();
                                            Some((Some(fname), arguments, None))
                                        }
                                        _ => Some((None, vec![], None)),
                                    }
                                } else {
                                    None
                                }
                            })
                            .unwrap_or((None, vec![], None));

                        OperationResultInfo {
                            function_name: fname,
                            arguments: args,
                            return_value: ret_val,
                            is_success: true,
                            error_category: None,
                            error_name: None,
                        }
                    }
                    stellar_xdr::curr::InvokeHostFunctionResult::Trapped => {
                        let fname = op.as_ref().and_then(|o| {
                            if let OperationBody::InvokeHostFunction(invoke) = &o.body {
                                match &invoke.host_function {
                                    stellar_xdr::curr::HostFunction::InvokeContract(args) => {
                                        Some(args.function_name.to_string())
                                    }
                                    _ => None,
                                }
                            } else {
                                None
                            }
                        });

                        OperationResultInfo {
                            function_name: fname,
                            arguments: vec![],
                            return_value: None,
                            is_success: false,
                            error_category: Some("Contract".to_string()),
                            error_name: Some("HostError".to_string()),
                        }
                    }
                    stellar_xdr::curr::InvokeHostFunctionResult::ResourceLimitExceeded => {
                        let fname = op.as_ref().and_then(|o| {
                            if let OperationBody::InvokeHostFunction(invoke) = &o.body {
                                match &invoke.host_function {
                                    stellar_xdr::curr::HostFunction::InvokeContract(args) => {
                                        Some(args.function_name.to_string())
                                    }
                                    _ => None,
                                }
                            } else {
                                None
                            }
                        });

                        OperationResultInfo {
                            function_name: fname,
                            arguments: vec![],
                            return_value: None,
                            is_success: false,
                            error_category: Some("Budget".to_string()),
                            error_name: Some("HostError".to_string()),
                        }
                    }
                    stellar_xdr::curr::InvokeHostFunctionResult::EntryArchived => {
                        let fname = op.as_ref().and_then(|o| {
                            if let OperationBody::InvokeHostFunction(invoke) = &o.body {
                                match &invoke.host_function {
                                    stellar_xdr::curr::HostFunction::InvokeContract(args) => {
                                        Some(args.function_name.to_string())
                                    }
                                    _ => None,
                                }
                            } else {
                                None
                            }
                        });

                        OperationResultInfo {
                            function_name: fname,
                            arguments: vec![],
                            return_value: None,
                            is_success: false,
                            error_category: Some("Storage".to_string()),
                            error_name: Some("HostError".to_string()),
                        }
                    }
                    stellar_xdr::curr::InvokeHostFunctionResult::Malformed
                    | stellar_xdr::curr::InvokeHostFunctionResult::InsufficientRefundableFee => {
                        let fname = op.as_ref().and_then(|o| {
                            if let OperationBody::InvokeHostFunction(invoke) = &o.body {
                                match &invoke.host_function {
                                    stellar_xdr::curr::HostFunction::InvokeContract(args) => {
                                        Some(args.function_name.to_string())
                                    }
                                    _ => None,
                                }
                            } else {
                                None
                            }
                        });

                        OperationResultInfo {
                            function_name: fname,
                            arguments: vec![],
                            return_value: None,
                            is_success: false,
                            error_category: Some("Context".to_string()),
                            error_name: Some("HostError".to_string()),
                        }
                    }
                },
                _ => {
                    let fname = op
                        .as_ref()
                        .and_then(|o| {
                            if let OperationBody::InvokeHostFunction(invoke) = &o.body {
                                match &invoke.host_function {
                                    stellar_xdr::curr::HostFunction::InvokeContract(args) => {
                                        Some(args.function_name.to_string())
                                    }
                                    _ => None,
                                }
                            } else {
                                None
                            }
                        })
                        .unwrap_or_default();

                    OperationResultInfo {
                        function_name: if fname.is_empty() { None } else { Some(fname) },
                        arguments: vec![],
                        return_value: None,
                        is_success: false,
                        error_category: Some("Unknown".to_string()),
                        error_name: Some("NonInvokeHostFunctionOperation".to_string()),
                    }
                }
            },
            _ => {
                let fname = op
                    .as_ref()
                    .and_then(|o| {
                        if let OperationBody::InvokeHostFunction(invoke) = &o.body {
                            match &invoke.host_function {
                                stellar_xdr::curr::HostFunction::InvokeContract(args) => {
                                    Some(args.function_name.to_string())
                                }
                                _ => None,
                            }
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default();

                OperationResultInfo {
                    function_name: if fname.is_empty() { None } else { Some(fname) },
                    arguments: vec![],
                    return_value: None,
                    is_success: false,
                    error_category: Some("Unknown".to_string()),
                    error_name: Some("NonInvokeHostFunctionOperation".to_string()),
                }
            }
        };

        results.push(info);
    }

    results
}

fn get_operation(envelope: &TransactionEnvelope, index: usize) -> Option<Operation> {
    match envelope {
        TransactionEnvelope::Tx(TransactionV1Envelope { tx, .. }) => {
            tx.operations.get(index).cloned()
        }
        TransactionEnvelope::TxFeeBump(fb) => match &fb.tx.inner_tx {
            FeeBumpTransactionInnerTx::Tx(TransactionV1Envelope { tx, .. }) => {
                tx.operations.get(index).cloned()
            }
        },
        TransactionEnvelope::TxV0(_) => None,
    }
}

fn partition_events_by_operation(
    diagnostic_events: &[DiagnosticEvent],
    num_operations: usize,
) -> Vec<Vec<DiagnosticEvent>> {
    if num_operations <= 1 || diagnostic_events.is_empty() {
        return vec![diagnostic_events.to_vec()];
    }

    let mut partitions: Vec<Vec<DiagnosticEvent>> = vec![Vec::new(); num_operations];
    let mut depth: usize = 0;
    let mut current_op: usize = 0;

    for event in diagnostic_events {
        if let ContractEventBody::V0(v0) = &event.event.body {
            let is_call = v0.topics.iter().any(|t| {
                if let ScVal::Symbol(s) = t {
                    let s_low = s.to_string().to_lowercase();
                    s_low == "fn_call" || s_low == "function_call" || s_low == "call"
                } else {
                    false
                }
            });

            if is_call {
                if depth == 0 && current_op < num_operations {
                    current_op += 1;
                }
                depth += 1;
            }

            if current_op > 0 && current_op <= num_operations {
                partitions[current_op - 1].push(event.clone());
            }

            let is_return = v0.topics.iter().any(|t| {
                if let ScVal::Symbol(s) = t {
                    let s_low = s.to_string().to_lowercase();
                    s_low == "fn_return" || s_low == "function_return" || s_low == "return"
                } else {
                    false
                }
            });

            if is_return {
                depth = depth.saturating_sub(1);
            }
        }
    }

    partitions
}

fn partition_contract_events_by_operation(
    contract_events: &[ContractEvent],
    num_operations: usize,
) -> Vec<Vec<ContractEvent>> {
    if num_operations <= 1 || contract_events.is_empty() {
        return vec![contract_events.to_vec()];
    }

    let mut partitions: Vec<Vec<ContractEvent>> = vec![Vec::new(); num_operations];
    let mut depth: usize = 0;
    let mut current_op: usize = 0;

    for event in contract_events {
        if let ContractEventBody::V0(v0) = &event.body {
            let is_call = v0.topics.iter().any(|t| {
                if let ScVal::Symbol(s) = t {
                    let s_low = s.to_string().to_lowercase();
                    s_low == "fn_call" || s_low == "function_call" || s_low == "call"
                } else {
                    false
                }
            });

            if is_call {
                if depth == 0 && current_op < num_operations {
                    current_op += 1;
                }
                depth += 1;
            }

            if current_op > 0 && current_op <= num_operations {
                partitions[current_op - 1].push(event.clone());
            }

            let is_return = v0.topics.iter().any(|t| {
                if let ScVal::Symbol(s) = t {
                    let s_low = s.to_string().to_lowercase();
                    s_low == "fn_return" || s_low == "function_return" || s_low == "return"
                } else {
                    false
                }
            });

            if is_return {
                depth = depth.saturating_sub(1);
            }
        }
    }

    partitions
}

fn enrich_diagnostic_report(
    report: &mut DiagnosticReport,
    tx_data: &serde_json::Value,
) -> crate::error::GratResult<()> {
    crate::decode::diagnostic::enrich_report(report, tx_data)
}

fn enrich_resource_report(
    report: &mut DiagnosticReport,
    tx_data: &serde_json::Value,
) -> crate::error::GratResult<()> {
    crate::decode::resource_analyzer::enrich_report(report, tx_data);
    Ok(())
}

pub async fn decode_transaction_with_op_filter(
    tx_hash: &str,
    network: &NetworkConfig,
    op_index: Option<usize>,
) -> GratResult<Vec<DiagnosticReport>> {
    let rpc = SorobanRpcClient::new(network);
    let tx_data = rpc.get_transaction(tx_hash).await?;
    let tx_json = serde_json::to_value(&tx_data)
        .map_err(|e| crate::error::GratError::Internal(e.to_string()))?;
    let decoder = MultiOpDecoder::new();
    let reports = decoder.decode_transaction(&tx_json)?;

    if let Some(idx) = op_index {
        if idx < reports.len() {
            return Ok(vec![reports[idx].clone()]);
        }
    }

    Ok(reports)
}
