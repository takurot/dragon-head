use super::*;

pub fn semantic_state_json_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "SemanticState",
        "type": "object",
        "additionalProperties": false,
        "required": ["metadata", "interactive_elements"],
        "properties": {
            "metadata": {
                "type": "object",
                "additionalProperties": false,
                "required": ["url", "page_instance_id", "state_hash", "load_profile", "timestamp"],
                "properties": {
                    "url": { "type": "string", "minLength": 1 },
                    "page_instance_id": { "type": "string", "minLength": 1 },
                    "state_hash": { "type": "string", "minLength": 1 },
                    "load_profile": { "type": "string", "enum": ["minimal", "visual", "interactive"] },
                    "timestamp": { "type": "integer", "minimum": 0 },
                    "speculative": { "type": "boolean" }
                }
            },
            "interactive_elements": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["id", "stable_key", "alias", "role", "name", "attributes", "bbox", "policy_flags"],
                    "properties": {
                        "id": { "type": "integer" },
                        "stable_key": { "type": "string", "minLength": 1 },
                        "alias": { "type": "string", "minLength": 1 },
                        "role": { "type": "string", "minLength": 1 },
                        "name": { "type": "string" },
                        "attributes": {
                            "type": "object",
                            "additionalProperties": {
                                "oneOf": [
                                    { "type": "string" },
                                    { "type": "number" },
                                    { "type": "integer" },
                                    { "type": "boolean" },
                                    { "type": "null" }
                                ]
                            }
                        },
                        "bbox": {
                            "type": "array",
                            "items": { "type": "number" },
                            "minItems": 4,
                            "maxItems": 4
                        },
                        "policy_flags": {
                            "type": "array",
                            "items": { "type": "string" }
                        },
                        "security_flags": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Prompt-injection security classification flags (omitted when empty)"
                        }
                    }
                }
            }
        }
    })
}

pub(crate) fn get_state_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "format": {
                "type": "string",
                "enum": ["json", "markdown"]
            },
            "force_refresh": {
                "type": "boolean"
            },
            "delivery": {
                "type": "string",
                "enum": ["full", "delta"]
            }
        }
    })
}

pub(crate) fn navigate_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["url"],
        "properties": {
            "url": {
                "type": "string",
                "minLength": 1,
                "maxLength": 8192
            }
        }
    })
}

pub(crate) fn act_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["action"],
        "properties": {
            "target_id": {
                "type": "integer",
                "minimum": i64::MIN,
                "maximum": i64::MAX
            },
            "target_stable_key": { "type": "string" },
            "action": {
                "type": "string",
                "enum": ["click", "type"]
            },
            "value": { "type": "string" }
        }
    })
}

pub(crate) fn verify_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["target_id", "expected"],
        "properties": {
            "target_id": {
                "type": "integer",
                "minimum": i64::MIN,
                "maximum": i64::MAX
            },
            "target_stable_key": { "type": "string" },
            "expected": {
                "type": "object",
                "additionalProperties": false,
                "required": ["text"],
                "properties": {
                    "text": { "type": "string", "minLength": 1 }
                }
            }
        }
    })
}

pub(crate) fn get_visual_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "mode": {
                "type": "string",
                "enum": ["clean", "som"]
            },
            "viewport": {
                "type": "string",
                "enum": ["full"]
            }
        }
    })
}

pub(crate) fn ask_human_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["reason"],
        "properties": {
            "reason": { "type": "string", "minLength": 1 },
            "context": { "type": "boolean" }
        }
    })
}

pub(crate) fn run_skill_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["skill_name"],
        "properties": {
            "skill_name": { "type": "string", "minLength": 1 },
            "params": { "type": "object" }
        }
    })
}

pub(crate) fn get_usage_report_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {}
    })
}

pub(crate) fn extract_input_schema() -> Value {
    let inline_rule_schema = json!({
        "oneOf": [
            {
                "type": "object",
                "additionalProperties": false,
                "required": ["selector"],
                "properties": {
                    "selector": { "type": "string", "minLength": 1 },
                    "attribute": { "type": "string", "minLength": 1 }
                }
            },
            {
                "type": "object",
                "additionalProperties": false,
                "required": ["selector", "fields"],
                "properties": {
                    "selector": { "type": "string", "minLength": 1 },
                    "fields": {
                        "type": "object",
                        "minProperties": 1,
                        "additionalProperties": { "type": "string", "minLength": 1 }
                    }
                }
            },
            {
                "type": "object",
                "additionalProperties": false,
                "required": ["items"],
                "properties": {
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["selector", "fields"],
                        "properties": {
                            "selector": { "type": "string", "minLength": 1 },
                            "fields": {
                                "type": "object",
                                "minProperties": 1,
                                "additionalProperties": { "type": "string", "minLength": 1 }
                            }
                        }
                    }
                }
            }
        ],
        "description": "Inline Deep Lens DSL rule"
    });

    json!({
        "type": "object",
        "additionalProperties": false,
        "oneOf": [
            {
                "required": ["rule_name"],
                "properties": {
                    "rule_name": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Name of a pre-registered SchemaRegistry rule"
                    }
                }
            },
            {
                "required": ["inline"],
                "properties": {
                    "inline": inline_rule_schema.clone()
                }
            }
        ],
        "properties": {
            "rule_name": {
                "type": "string",
                "minLength": 1,
                "description": "Name of a pre-registered SchemaRegistry rule"
            },
            "inline": inline_rule_schema,
            "debug": {
                "type": "boolean",
                "description": "Also return the generated JavaScript as `script` for debugging a rule"
            }
        }
    })
}
