use crate::command::{NavigationMode, NavigationRequest, ViewEffect, ViewReturn};
use crate::engine::{EngineDecision, EngineNotice, RuntimeUpdate};
use crate::runtime::RuntimeStore;
use anyhow::{Result, bail, ensure};

pub(super) fn decision_effect(
    decision: &EngineDecision,
    source_view: &str,
) -> Result<Option<ViewEffect>> {
    let effect = match decision {
        EngineDecision::Continue
        | EngineDecision::Invalidate
        | EngineDecision::Report(_)
        | EngineDecision::RuntimeUpdate(_) => None,
        EngineDecision::Return(output) => Some(ViewEffect::Return(ViewReturn {
            source_view: source_view.to_string(),
            output: output.clone(),
            adapter: None,
        })),
        EngineDecision::Navigate(request) => Some(ViewEffect::Navigate {
            request: NavigationRequest::with_defaults(request.target.clone()).with_parameters(
                request
                    .parameters
                    .clone()
                    .unwrap_or(serde_json::Value::Null),
            ),
            mode: if request.replace {
                NavigationMode::Replace
            } else {
                NavigationMode::Push
            },
            parent_edit: None,
        }),
        EngineDecision::Edit(edit) => Some(ViewEffect::EditInput(edit.clone())),
        EngineDecision::DispatchCommand(execution) => {
            Some(ViewEffect::DispatchCommand((**execution).clone()))
        }
        EngineDecision::Execute(crate::engine::EffectRequest::CopyToClipboard(value)) => {
            Some(ViewEffect::CopyToClipboard(value.clone()))
        }
        EngineDecision::Close => Some(ViewEffect::Back(None)),
        EngineDecision::Exit => Some(ViewEffect::Exit),
        EngineDecision::ParameterPatch(_) => {
            bail!("ParameterPatch requires the active Session parameter path")
        }
        EngineDecision::Batch(decisions) => {
            ensure!(
                batch_effect_is_last(decisions),
                "an effectful Engine decision must be the final item in a Batch"
            );
            let mut effect = None;
            for decision in decisions {
                if let Some(next) = decision_effect(decision, source_view)? {
                    ensure!(
                        effect.is_none(),
                        "an Engine Batch may produce only one effect"
                    );
                    effect = Some(next);
                }
            }
            effect
        }
    };
    Ok(effect)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DecisionPolicy {
    Normal,
    Initial,
    Lifecycle,
}

#[derive(Default)]
pub(super) struct DecisionPreflight {
    pub(super) reports: Vec<EngineNotice>,
    pub(super) runtime_updates: Vec<RuntimeUpdate>,
}

pub(super) fn staged_runtime(runtime: &RuntimeStore) -> RuntimeStore {
    let mut staged = RuntimeStore::new();
    staged.replace(runtime.snapshot().clone());
    staged
}

pub(super) fn preflight_engine_decision(
    decision: &EngineDecision,
    runtime: &mut RuntimeStore,
    policy: DecisionPolicy,
) -> Result<DecisionPreflight> {
    let mut preflight = DecisionPreflight::default();
    visit_decision(decision, policy, false, &mut preflight)?;
    if !preflight.runtime_updates.is_empty() {
        runtime.set_many(
            preflight
                .runtime_updates
                .iter()
                .map(|update| (update.path.as_str(), update.value.clone())),
        )?;
    }
    Ok(preflight)
}

fn visit_decision(
    decision: &EngineDecision,
    policy: DecisionPolicy,
    in_batch: bool,
    preflight: &mut DecisionPreflight,
) -> Result<()> {
    match decision {
        EngineDecision::Batch(decisions) => {
            if policy == DecisionPolicy::Normal {
                ensure!(
                    batch_effect_is_last(decisions),
                    "an effectful Engine decision must be the final item in a Batch"
                );
            }
            for decision in decisions {
                visit_decision(decision, policy, true, preflight)?;
            }
        }
        EngineDecision::Continue | EngineDecision::Invalidate => {
            validate_leaf(decision, policy)?;
        }
        EngineDecision::Report(notice) => {
            validate_leaf(decision, policy)?;
            preflight.reports.push(notice.clone());
        }
        EngineDecision::RuntimeUpdate(update) => {
            validate_leaf(decision, policy)?;
            preflight.runtime_updates.push(update.clone());
        }
        EngineDecision::ParameterPatch(_) => {
            if in_batch && policy == DecisionPolicy::Normal {
                bail!("ParameterPatch cannot be used in a Batch because it mutates Session state");
            }
            validate_leaf(decision, policy)?;
        }
        EngineDecision::Return(_)
        | EngineDecision::Navigate(_)
        | EngineDecision::Edit(_)
        | EngineDecision::DispatchCommand(_)
        | EngineDecision::Execute(_)
        | EngineDecision::Close
        | EngineDecision::Exit => {
            validate_leaf(decision, policy)?;
        }
    }
    Ok(())
}

fn validate_leaf(decision: &EngineDecision, policy: DecisionPolicy) -> Result<()> {
    let allowed = match policy {
        DecisionPolicy::Normal => true,
        DecisionPolicy::Initial => matches!(
            decision,
            EngineDecision::Continue
                | EngineDecision::Invalidate
                | EngineDecision::Report(_)
                | EngineDecision::RuntimeUpdate(_)
        ),
        DecisionPolicy::Lifecycle => matches!(
            decision,
            EngineDecision::Continue
                | EngineDecision::Invalidate
                | EngineDecision::Report(_)
                | EngineDecision::RuntimeUpdate(_)
        ),
    };
    if allowed {
        return Ok(());
    }

    match policy {
        DecisionPolicy::Normal => unreachable!("normal Engine decisions are unrestricted"),
        DecisionPolicy::Initial => {
            bail!("initial Engine parameters may only return Report or RuntimeUpdate decisions");
        }
        DecisionPolicy::Lifecycle => {
            bail!("lifecycle phase cannot return a transition decision");
        }
    }
}

fn batch_effect_is_last(decisions: &[EngineDecision]) -> bool {
    let mut effect_seen = false;
    for decision in decisions {
        if effect_seen {
            return false;
        }
        if decision_may_effect(decision) {
            effect_seen = true;
        }
    }
    true
}

fn decision_may_effect(decision: &EngineDecision) -> bool {
    match decision {
        EngineDecision::Continue
        | EngineDecision::Invalidate
        | EngineDecision::Report(_)
        | EngineDecision::RuntimeUpdate(_) => false,
        EngineDecision::Batch(decisions) => decisions.iter().any(decision_may_effect),
        EngineDecision::Return(_)
        | EngineDecision::Navigate(_)
        | EngineDecision::Edit(_)
        | EngineDecision::ParameterPatch(_)
        | EngineDecision::DispatchCommand(_)
        | EngineDecision::Execute(_)
        | EngineDecision::Close
        | EngineDecision::Exit => true,
    }
}

#[cfg(test)]
mod tests {
    use super::decision_effect;
    use crate::command::{NavigationMode, ViewEffect, ViewOutput};
    use crate::engine::{EngineDecision, EngineNavigationRequest};
    use serde_json::json;

    #[test]
    fn return_effect_records_the_source_view() {
        let output = ViewOutput::Value { value: json!(42) };
        let effect = decision_effect(&EngineDecision::Return(output.clone()), "apps:source")
            .unwrap()
            .expect("Return must produce an effect");

        match effect {
            ViewEffect::Return(returned) => {
                assert_eq!(returned.source_view, "apps:source");
                assert_eq!(returned.output, output);
                assert!(returned.adapter.is_none());
            }
            _ => panic!("Return produced the wrong effect"),
        }
    }

    #[test]
    fn navigate_effect_preserves_replace_and_normalizes_parameters() {
        let explicit = decision_effect(
            &EngineDecision::Navigate(EngineNavigationRequest {
                target: "apps:target".to_string(),
                parameters: Some(json!({"query": "ready"})),
                replace: true,
            }),
            "apps:source",
        )
        .unwrap()
        .expect("Navigate must produce an effect");
        match explicit {
            ViewEffect::Navigate {
                request,
                mode,
                parent_edit,
            } => {
                assert_eq!(request.view_ref, "apps:target");
                assert_eq!(request.parameters, Some(json!({"query": "ready"})));
                assert_eq!(mode, NavigationMode::Replace);
                assert!(parent_edit.is_none());
            }
            _ => panic!("Navigate produced the wrong effect"),
        }

        let defaults = decision_effect(
            &EngineDecision::Navigate(EngineNavigationRequest {
                target: "apps:default".to_string(),
                parameters: None,
                replace: false,
            }),
            "apps:source",
        )
        .unwrap()
        .expect("Navigate must produce an effect");
        match defaults {
            ViewEffect::Navigate { request, mode, .. } => {
                assert_eq!(request.parameters, Some(serde_json::Value::Null));
                assert_eq!(mode, NavigationMode::Push);
            }
            _ => panic!("Navigate produced the wrong effect"),
        }
    }

    #[test]
    fn batch_allows_one_final_effect() {
        let effect = decision_effect(
            &EngineDecision::Batch(vec![
                EngineDecision::Continue,
                EngineDecision::RuntimeUpdate(crate::engine::RuntimeUpdate {
                    path: "/marker".to_string(),
                    value: json!(true),
                }),
                EngineDecision::Exit,
            ]),
            "apps:source",
        )
        .unwrap();

        assert!(matches!(effect, Some(ViewEffect::Exit)));
    }

    #[test]
    fn batch_rejects_non_effect_after_effect() {
        let error = match decision_effect(
            &EngineDecision::Batch(vec![EngineDecision::Close, EngineDecision::Continue]),
            "apps:source",
        ) {
            Ok(_) => panic!("an effect followed by a non-effect must be rejected"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("must be the final item"));
    }

    #[test]
    fn parameter_patch_requires_the_active_session_path() {
        let error = match decision_effect(
            &EngineDecision::ParameterPatch(crate::parameter::ParameterPatchRequest::new(
                crate::input::ViewMountId(71),
                json!({"query": "next"}),
                Some(3),
                crate::parameter::ParameterInputPolicy::Preserve,
            )),
            "apps:source",
        ) {
            Ok(_) => panic!("the effect converter must reject ParameterPatch"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("active Session parameter path"));
    }
}
