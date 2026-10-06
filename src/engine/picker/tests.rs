use super::*;

fn validate_picker(source: &str) -> Result<()> {
    let view: View = toml::from_str(source).unwrap();
    validate_config(EngineValidationContext {
        view_ref: "sample:main",
        view: &view,
        script_root: None,
    })
}

#[test]
fn input_placeholder_must_be_a_string() {
    for field in STRING_FIELDS {
        for value in ["true", "0", "[]", "{}"] {
            let error = validate_picker(&format!("engine = 'picker'\n[picker]\n{field} = {value}"))
                .unwrap_err()
                .to_string();
            assert!(error.contains("sample:main"), "{error}");
            assert!(
                error.contains(&format!("{field} must be a string")),
                "{error}"
            );
        }
        validate_picker(&format!("engine = 'picker'\n[picker]\n{field} = 'Search'")).unwrap();
    }
}

#[test]
fn display_options_must_be_boolean() {
    for field in BOOLEAN_FIELDS {
        for value in ["true", "false", "'false'", "0", "[]", "{}"] {
            let result =
                validate_picker(&format!("engine = 'picker'\n[picker]\n{field} = {value}"));
            if matches!(value, "true" | "false") {
                result.unwrap();
            } else {
                let error = result.unwrap_err().to_string();
                assert!(error.contains("sample:main"), "{error}");
                assert!(
                    error.contains(&format!("{field} must be a boolean")),
                    "{error}"
                );
            }
        }
    }
}

#[test]
fn static_item_shapes_are_validated_during_engine_validation() {
    validate_picker(
        "engine = 'picker'\n[picker]\nitems = [{ display = 'Example item', value = 'example-value' }]",
    )
    .unwrap();
    assert!(
        validate_picker("engine = 'picker'\n[picker]\nitems = [{ value = 'missing-display' }]")
            .is_err()
    );
}
