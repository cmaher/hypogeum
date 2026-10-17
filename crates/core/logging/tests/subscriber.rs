use hypgm_logging::{
    LogFormat, LoggingError, TestWriter, create_subscriber, create_test_subscriber,
};

#[test]
fn test_valid_text_subscriber_construction() {
    let writer = TestWriter::new();
    let subscriber = create_subscriber("info", LogFormat::Text, writer.clone());
    assert!(subscriber.is_ok());
}

#[test]
fn test_valid_json_subscriber_construction() {
    let writer = TestWriter::new();
    let subscriber = create_subscriber("debug", LogFormat::Json, writer.clone());
    assert!(subscriber.is_ok());
}

#[test]
fn test_invalid_filter_fails_cleanly() {
    let writer = TestWriter::new();
    let res = create_subscriber("invalid[[[filter", LogFormat::Text, writer);
    assert!(res.is_err());
    match res {
        Err(LoggingError::InvalidFilter(_)) => {}
        Err(other) => panic!("expected InvalidFilter error, got: {other:?}"),
        Ok(_) => panic!("expected error, got Ok"),
    }
}

#[test]
fn test_log_format_from_str_and_serde() {
    assert_eq!("text".parse::<LogFormat>().unwrap(), LogFormat::Text);
    assert_eq!("TEXT".parse::<LogFormat>().unwrap(), LogFormat::Text);
    assert_eq!("json".parse::<LogFormat>().unwrap(), LogFormat::Json);
    assert_eq!("JSON".parse::<LogFormat>().unwrap(), LogFormat::Json);
    assert!("xml".parse::<LogFormat>().is_err());

    let text_json: LogFormat = serde_json::from_str("\"text\"").unwrap();
    assert_eq!(text_json, LogFormat::Text);
    let json_json: LogFormat = serde_json::from_str("\"json\"").unwrap();
    assert_eq!(json_json, LogFormat::Json);
}

#[test]
fn test_captured_json_output_parses_and_includes_named_fields() {
    let (subscriber, writer) = create_test_subscriber("info", LogFormat::Json).unwrap();

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(
            request_id = "req-12345",
            user_id = 99,
            action = "test_event",
            "operation completed successfully"
        );
    });

    let output = writer.output_string();
    assert!(!output.is_empty(), "expected captured output");

    let parsed: serde_json::Value =
        serde_json::from_str(output.trim()).expect("captured output must be valid json");

    assert_eq!(parsed["level"], "INFO");
    let fields = &parsed["fields"];
    assert_eq!(fields["message"], "operation completed successfully");
    assert_eq!(fields["request_id"], "req-12345");
    assert_eq!(fields["user_id"], 99);
    assert_eq!(fields["action"], "test_event");
}

#[test]
fn test_captured_text_output() {
    let (subscriber, writer) = create_test_subscriber("info", LogFormat::Text).unwrap();

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(test_key = "test_value", "hello text format");
    });

    let output = writer.output_string();
    assert!(output.contains("hello text format"));
    assert!(output.contains("test_key"));
}

#[test]
fn test_filter_level_respects_threshold() {
    let (subscriber, writer) = create_test_subscriber("warn", LogFormat::Text).unwrap();

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!("this should be filtered out");
        tracing::warn!("this should be logged");
    });

    let output = writer.output_string();
    assert!(!output.contains("this should be filtered out"));
    assert!(output.contains("this should be logged"));
}
