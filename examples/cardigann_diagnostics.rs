//! Offline corpus compilation report. No service configuration or network access.
use rackett::{
    cardigann::{load, Limits},
    cardigann_exec::Plan,
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "vendor/jackett/definitions".into());
    let mut paths = std::fs::read_dir(directory)?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "yml" || e == "yaml"))
        .collect::<Vec<_>>();
    paths.sort();
    let mut results = Vec::new();
    let mut compiled = 0;
    let mut blocked_sessions = 0;
    let mut runtime_executable = 0;
    for path in paths {
        let runtime_supported = std::fs::read_to_string(&path).ok().is_some_and(|raw| {
            rackett::builtin::SOURCES.iter().any(|(id, source)| {
                *source == raw
                    && rackett::builtin::catalog()
                        .iter()
                        .any(|entry| entry["id"] == *id && entry["ready"] == true)
            })
        });
        let entry = match load(&path, Limits::default()) {
            Err(e) => serde_json::json!({"file":path,"model_error":e}),
            Ok(node) => match Plan::compile(&node) {
                Ok(plan) => {
                    compiled += 1;
                    runtime_executable += usize::from(runtime_supported);
                    if !plan.blockers().is_empty() {
                        blocked_sessions += 1;
                    }
                    serde_json::json!({"file":path,"offline_plan_compiles":true,"runtime_execution_supported":runtime_supported,"integration_blockers":plan.blockers().iter().map(|b|serde_json::json!({"path":b.path,"feature":b.feature})).collect::<Vec<_>>()})
                }
                Err(e) => {
                    serde_json::json!({"file":path,"offline_plan_compiles":false,"runtime_execution_supported":false,"diagnostic":{"path":e.path,"message":e.message}})
                }
            },
        };
        results.push(entry);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &serde_json::json!({"summary":{"definitions":results.len(),"offline_plans":compiled,"plans_with_integration_blockers":blocked_sessions,"runtime_executable":runtime_executable},"results":results})
        )?
    );
    Ok(())
}
